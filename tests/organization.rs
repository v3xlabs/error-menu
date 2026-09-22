use error_menu::database::Database;
use error_menu::forge::ForgeKind;
use error_menu::organization::Organization;
use error_menu::organization::member::{
    OrganizationMember, OrganizationMemberChange, OrganizationRole,
};
use error_menu::project::member::{ProjectMember, ProjectRole};
use error_menu::project::{NewProject, Project, ProjectSummary};
use error_menu::user::User;
use error_menu::vcs::RemoteUrl;

struct Fixture {
    database: Database,
    owner: User,
    coworker: User,
    stranger: User,
}

impl Fixture {
    /// The first account an instance registers becomes its administrator, and an
    /// administrator reaches every project by instance role alone. Absorbing that role
    /// with an unused account keeps these tests about grants.
    async fn build() -> Self {
        let database = Database::open("sqlite::memory:", 0).await.expect("opens");
        let _administrator = register(&database, "administrator").await;

        Self {
            owner: register(&database, "owner").await,
            coworker: register(&database, "coworker").await,
            stranger: register(&database, "stranger").await,
            database,
        }
    }

    async fn organization(&self, name: &str) -> Organization {
        Organization::create(&self.database, self.owner.id, name, None)
            .await
            .expect("an organization")
    }

    async fn project(&self, organization: &Organization, name: &str) -> Project {
        Project::create(
            &self.database,
            NewProject {
                organization_id: organization.id,
                owner_id: self.owner.id,
                name,
                remote: RemoteUrl::new(&format!("https://github.com/owner/{name}")).expect("a url"),
                forge_kind: ForgeKind::Auto,
                uses_default_analyzers: true,
                analyzers: &[],
            },
        )
        .await
        .expect("a project")
    }

    async fn visible_to(&self, user: &User) -> Vec<ProjectSummary> {
        Project::summaries_for(&self.database, user)
            .await
            .expect("summaries")
    }
}

async fn register(database: &Database, name: &str) -> User {
    User::register(database, "urn:test", name, name, true)
        .await
        .expect("registers")
        .expect("a user")
}

fn names(summaries: &[ProjectSummary]) -> Vec<&str> {
    summaries
        .iter()
        .map(|summary| summary.project.name.as_str())
        .collect()
}

#[tokio::test]
async fn an_organization_grant_reaches_every_project_inside_it() {
    let fixture = Fixture::build().await;
    let company = fixture.organization("Company A").await;
    fixture.project(&company, "api").await;
    fixture.project(&company, "web").await;

    OrganizationMember::set(
        &fixture.database,
        company.id,
        fixture.coworker.id,
        OrganizationRole::Viewer,
    )
    .await
    .expect("a grant");

    let visible = fixture.visible_to(&fixture.coworker).await;
    assert_eq!(names(&visible), ["web", "api"]);
    assert!(
        visible
            .iter()
            .all(|summary| summary.viewer_role == ProjectRole::Viewer)
    );
    assert!(
        visible
            .iter()
            .all(|summary| summary.organization_name == "Company A")
    );
}

/// A guest holds no instance role worth anything. The grant is what grants.
#[tokio::test]
async fn a_grant_needs_no_instance_promotion_first() {
    let fixture = Fixture::build().await;
    let company = fixture.organization("Company B").await;
    let project = fixture.project(&company, "service").await;

    assert_eq!(
        ProjectRole::for_user(&fixture.database, &fixture.coworker, project.id)
            .await
            .expect("resolves"),
        None
    );

    OrganizationMember::set(
        &fixture.database,
        company.id,
        fixture.coworker.id,
        OrganizationRole::Operator,
    )
    .await
    .expect("a grant");

    assert_eq!(
        ProjectRole::for_user(&fixture.database, &fixture.coworker, project.id)
            .await
            .expect("resolves"),
        Some(ProjectRole::Operator)
    );
}

#[tokio::test]
async fn the_higher_of_the_two_grants_wins() {
    let fixture = Fixture::build().await;
    let company = fixture.organization("Company A").await;
    let project = fixture.project(&company, "api").await;
    OrganizationMember::set(
        &fixture.database,
        company.id,
        fixture.coworker.id,
        OrganizationRole::Viewer,
    )
    .await
    .expect("a grant");
    ProjectMember::set(
        &fixture.database,
        project.id,
        fixture.coworker.id,
        ProjectRole::Owner,
    )
    .await
    .expect("a grant");

    assert_eq!(
        ProjectRole::for_user(&fixture.database, &fixture.coworker, project.id)
            .await
            .expect("resolves"),
        Some(ProjectRole::Owner)
    );

    ProjectMember::set(
        &fixture.database,
        project.id,
        fixture.coworker.id,
        ProjectRole::Viewer,
    )
    .await
    .expect("a grant");
    OrganizationMember::set(
        &fixture.database,
        company.id,
        fixture.coworker.id,
        OrganizationRole::Operator,
    )
    .await
    .expect("a grant");

    assert_eq!(
        ProjectRole::for_user(&fixture.database, &fixture.coworker, project.id)
            .await
            .expect("resolves"),
        Some(ProjectRole::Operator)
    );
}

#[tokio::test]
async fn a_grant_stops_at_the_edge_of_its_organization() {
    let fixture = Fixture::build().await;
    let company = fixture.organization("Company A").await;
    let personal = fixture.organization("Personal").await;
    fixture.project(&company, "shared").await;
    let private = fixture.project(&personal, "dotfiles").await;

    OrganizationMember::set(
        &fixture.database,
        company.id,
        fixture.coworker.id,
        OrganizationRole::Owner,
    )
    .await
    .expect("a grant");

    assert_eq!(names(&fixture.visible_to(&fixture.coworker).await), [
        "shared"
    ]);
    assert_eq!(
        ProjectRole::for_user(&fixture.database, &fixture.coworker, private.id)
            .await
            .expect("resolves"),
        None
    );
    assert!(fixture.visible_to(&fixture.stranger).await.is_empty());
}

#[tokio::test]
async fn moving_a_project_moves_who_can_see_it() {
    let fixture = Fixture::build().await;
    let company = fixture.organization("Company A").await;
    let personal = fixture.organization("Personal").await;
    let project = fixture.project(&personal, "dotfiles").await;
    OrganizationMember::set(
        &fixture.database,
        company.id,
        fixture.coworker.id,
        OrganizationRole::Viewer,
    )
    .await
    .expect("a grant");

    assert!(fixture.visible_to(&fixture.coworker).await.is_empty());

    project
        .move_to(&fixture.database, company.id)
        .await
        .expect("moves");

    assert_eq!(names(&fixture.visible_to(&fixture.coworker).await), [
        "dotfiles"
    ]);
}

#[tokio::test]
async fn an_organization_keeps_an_owner() {
    let fixture = Fixture::build().await;
    let company = fixture.organization("Company A").await;

    assert_eq!(
        OrganizationMember::remove(&fixture.database, company.id, fixture.owner.id)
            .await
            .expect("answers"),
        OrganizationMemberChange::FinalOwner
    );

    OrganizationMember::set(
        &fixture.database,
        company.id,
        fixture.coworker.id,
        OrganizationRole::Owner,
    )
    .await
    .expect("a grant");

    assert_eq!(
        OrganizationMember::remove(&fixture.database, company.id, fixture.owner.id)
            .await
            .expect("answers"),
        OrganizationMemberChange::Removed
    );
}
