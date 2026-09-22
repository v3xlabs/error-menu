use error_menu::database::Database;
use error_menu::forge::ForgeKind;
use error_menu::organization::member::{
    OrganizationMember, OrganizationMemberChange, OrganizationRole,
};
use error_menu::organization::{Organization, OrganizationDeletion};
use error_menu::project::member::{ProjectMember, ProjectRole};
use error_menu::project::transfer::ProjectTransfer;
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

    assert_eq!(
        names(&fixture.visible_to(&fixture.coworker).await),
        ["shared"]
    );
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
        .transfer(&fixture.database, &personal, &company, fixture.owner.id)
        .await
        .expect("moves");

    assert_eq!(
        names(&fixture.visible_to(&fixture.coworker).await),
        ["dotfiles"]
    );
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

/// Moving or deleting a project is gated on an owner grant on the organization holding it,
/// which rests on a project grant never reaching the organization. A project owner who was
/// given that one repository resolves to no organization role at all.
#[tokio::test]
async fn a_project_owner_is_not_an_owner_of_its_organization() {
    let fixture = Fixture::build().await;
    let personal = fixture.organization("Personal").await;
    let project = fixture.project(&personal, "dotfiles").await;
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
            .expect("a role"),
        Some(ProjectRole::Owner)
    );
    assert_eq!(
        OrganizationRole::for_user(&fixture.database, &fixture.coworker, personal.id)
            .await
            .expect("a role"),
        None
    );
}

/// The existing move test proves the destination gains sight. This is the half nobody
/// asserted: the organization a project leaves stops reaching it, on the listing and on
/// the project itself.
#[tokio::test]
async fn a_transfer_takes_a_project_out_of_reach_of_its_old_organization() {
    let fixture = Fixture::build().await;
    let personal = fixture.organization("Personal").await;
    let company = fixture.organization("Company A").await;
    let project = fixture.project(&personal, "dotfiles").await;
    OrganizationMember::set(
        &fixture.database,
        personal.id,
        fixture.coworker.id,
        OrganizationRole::Operator,
    )
    .await
    .expect("a grant");

    assert_eq!(
        names(&fixture.visible_to(&fixture.coworker).await),
        ["dotfiles"]
    );

    project
        .transfer(&fixture.database, &personal, &company, fixture.owner.id)
        .await
        .expect("moves");

    assert!(fixture.visible_to(&fixture.coworker).await.is_empty());
    assert_eq!(
        ProjectRole::for_user(&fixture.database, &fixture.coworker, project.id)
            .await
            .expect("a role"),
        None
    );
}

/// A project grant was given out about the project, so it travels with it. The reader who
/// holds one keeps it inside the destination, where nobody granted it.
#[tokio::test]
async fn a_direct_project_grant_survives_a_transfer() {
    let fixture = Fixture::build().await;
    let personal = fixture.organization("Personal").await;
    let company = fixture.organization("Company A").await;
    let project = fixture.project(&personal, "dotfiles").await;
    ProjectMember::set(
        &fixture.database,
        project.id,
        fixture.coworker.id,
        ProjectRole::Viewer,
    )
    .await
    .expect("a grant");

    project
        .transfer(&fixture.database, &personal, &company, fixture.owner.id)
        .await
        .expect("moves");

    assert_eq!(
        ProjectRole::for_user(&fixture.database, &fixture.coworker, project.id)
            .await
            .expect("a role"),
        Some(ProjectRole::Viewer)
    );
}

/// The organization a project came from is overwritten by the move, so the record is the
/// only thing that still holds it. It names organizations rather than referencing them,
/// which is what lets an emptied organization be deleted afterwards.
#[tokio::test]
async fn a_transfer_records_where_the_project_came_from() {
    let fixture = Fixture::build().await;
    let personal = fixture.organization("Personal").await;
    let company = fixture.organization("Company A").await;
    let project = fixture.project(&personal, "dotfiles").await;

    project
        .transfer(&fixture.database, &personal, &company, fixture.owner.id)
        .await
        .expect("moves");

    let transfers = ProjectTransfer::for_project(&fixture.database, project.id)
        .await
        .expect("transfers");

    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].from_organization_name, "Personal");
    assert_eq!(transfers[0].to_organization_name, "Company A");
    assert_eq!(transfers[0].moved_by, fixture.owner.id);
    assert_eq!(transfers[0].moved_by_name, "owner");

    assert_eq!(
        personal.delete(&fixture.database).await.expect("answers"),
        OrganizationDeletion::Deleted
    );
    assert_eq!(
        ProjectTransfer::for_project(&fixture.database, project.id)
            .await
            .expect("transfers")
            .len(),
        1
    );
}

/// Every listing and every access check reads `projects.organization_id`, so an
/// organization that still holds projects cannot go: the projects would be unreachable.
#[tokio::test]
async fn an_organization_holding_projects_is_not_deleted() {
    let fixture = Fixture::build().await;
    let personal = fixture.organization("Personal").await;
    let company = fixture.organization("Company A").await;
    let project = fixture.project(&personal, "dotfiles").await;

    assert_eq!(
        personal.delete(&fixture.database).await.expect("answers"),
        OrganizationDeletion::HoldsProjects(1)
    );

    project
        .transfer(&fixture.database, &personal, &company, fixture.owner.id)
        .await
        .expect("moves");

    assert_eq!(
        personal.delete(&fixture.database).await.expect("answers"),
        OrganizationDeletion::Deleted
    );
}

/// Deleting a project takes its grants and its history with it. A member who reached it
/// through a direct grant is left with nothing to reach.
#[tokio::test]
async fn deleting_a_project_takes_its_grants_with_it() {
    let fixture = Fixture::build().await;
    let personal = fixture.organization("Personal").await;
    let project = fixture.project(&personal, "dotfiles").await;
    ProjectMember::set(
        &fixture.database,
        project.id,
        fixture.coworker.id,
        ProjectRole::Operator,
    )
    .await
    .expect("a grant");

    project.delete(&fixture.database).await.expect("deletes");

    assert!(fixture.visible_to(&fixture.owner).await.is_empty());
    assert!(
        Project::load(&fixture.database, project.id)
            .await
            .expect("answers")
            .is_none()
    );
    assert_eq!(
        ProjectMember::load(&fixture.database, project.id, fixture.coworker.id)
            .await
            .expect("answers"),
        None
    );
}
