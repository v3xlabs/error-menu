use error_menu::database::Database;
use error_menu::forge::{ChangeState, ForgeMetadata};
use error_menu::id::Id;
use error_menu::project::Project;
use error_menu::project::snapshot::Snapshot;
use error_menu::project::subject::{Subject, SubjectKind};
use error_menu::vcs::CommitSha;

const HEAD: &str = "8de08b979d673013e8a08a539e653d05a4087472";
const BASE: &str = "560b6c2530fe1e9d0c29a3bfc23ffcfe1b788bd5";
const MERGE_BASE: &str = "3814b368ee6a03aa46ea3fde76b6ebb2196ba324";

struct Fixture {
    database: Database,
    subject: Subject,
}

impl Fixture {
    async fn build() -> Self {
        let database = Database::open("sqlite::memory:", 0).await.expect("opens");
        let project_id: Id<Project> = database.ids.next();
        sqlx::query(
            "INSERT INTO projects (id, name, remote_url, forge_kind, uses_default_analyzers) \
             VALUES (?, 'subject', 'https://github.com/owner/repo', 'auto', 1)",
        )
        .bind(project_id.raw())
        .execute(&database.pool)
        .await
        .expect("a project");
        let subject = Subject::upsert(&database, project_id, SubjectKind::Change { number: 3 })
            .await
            .expect("a subject");

        Self { database, subject }
    }

    async fn observe(&self) {
        Snapshot::observe(
            &self.database,
            self.subject.id,
            sha(HEAD),
            Some(sha(BASE)),
            metadata(),
            None,
        )
        .await
        .expect("observes");
    }

    async fn analyse(&self) -> Snapshot {
        Snapshot::record(
            &self.database,
            self.subject.id,
            sha(HEAD),
            Some(sha(BASE)),
            Some(sha(MERGE_BASE)),
            metadata(),
        )
        .await
        .expect("records")
    }

    async fn readings(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM snapshots WHERE subject_id = ?")
            .bind(self.subject.id.raw())
            .fetch_one(&self.database.pool)
            .await
            .expect("counts")
    }

    async fn attach_run(&self, snapshot: &Snapshot) {
        let id: Id<Snapshot> = self.database.ids.next();
        sqlx::query(
            "INSERT INTO runs (id, snapshot_id, analyzer, status, started_at, finished_at) \
             VALUES (?, ?, 'secret-scan', 'succeeded', '2026-09-22T00:00:00Z', '2026-09-22T00:00:00Z')",
        )
        .bind(id.raw())
        .bind(snapshot.id.raw())
        .execute(&self.database.pool)
        .await
        .expect("a run");
    }
}

fn sha(value: &str) -> CommitSha {
    CommitSha::new(value).expect("a sha")
}

fn metadata() -> ForgeMetadata {
    ForgeMetadata {
        title: Some("a change".to_owned()),
        state: Some(ChangeState::Open),
        ..ForgeMetadata::default()
    }
}

#[tokio::test]
async fn polling_one_head_keeps_one_reading() {
    let fixture = Fixture::build().await;

    for _ in 0..4 {
        fixture.observe().await;
    }

    let readings = fixture.readings().await;

    assert_eq!(readings, 1, "four polls wrote {readings} readings");
}

#[tokio::test]
async fn analysing_a_recorded_head_keeps_one_reading() {
    let fixture = Fixture::build().await;
    fixture.observe().await;

    fixture.analyse().await;

    let readings = fixture.readings().await;

    assert_eq!(
        readings, 1,
        "a head discovery recorded and analysis scanned wrote {readings} readings"
    );
}

#[tokio::test]
async fn re_scanning_an_analysed_head_is_a_new_reading() {
    let fixture = Fixture::build().await;
    let scanned = fixture.analyse().await;
    fixture.attach_run(&scanned).await;

    let rescanned = fixture.analyse().await;

    assert_ne!(
        rescanned.id, scanned.id,
        "a re-scan took over the reading that holds the earlier runs"
    );
    assert_eq!(fixture.readings().await, 2);
}
