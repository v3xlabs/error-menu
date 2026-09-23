use error_menu::analysis::NewRun;
use error_menu::analysis::finding::fingerprint::{Components, Fingerprint};
use error_menu::forge::{ChangeState, ForgeMetadata};
use error_menu::prelude::*;
use error_menu::registry::AUDIT;

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

    async fn lock(&self, snapshot: &Snapshot, packages: &[(&str, PackageOrigin)]) {
        let findings: Vec<NewFinding> = packages
            .iter()
            .map(|(name, origin)| {
                let location = Location::Package {
                    path: RepoPath::new("Cargo.lock").expect("valid path"),
                    ecosystem: Ecosystem::Cargo,
                    name: (*name).to_owned(),
                    version: "1.0.0".to_owned(),
                    origin: Some(origin.clone()),
                    integrity: Some("claimed".to_owned()),
                };
                let title = format!("{name} 1.0.0 was added to the lockfile");

                NewFinding {
                    movement: Some(VersionMovement::Added),
                    fingerprint: Fingerprint::compute(&Components {
                        analyzer: "lockfile-delta",
                        rule: "package-added",
                        location: &location,
                        title: &title,
                        occurrence: 0,
                    }),
                    location,
                    severity: Severity::Info,
                    confidence: Confidence::new(1.0).expect("in range"),
                    attribution: Attribution::Introduced,
                    title,
                    detail: String::new(),
                }
            })
            .collect();

        Run::record(
            &self.database,
            NewRun {
                snapshot_id: snapshot.id,
                analyzer: "lockfile-delta",
                status: RunStatus::Succeeded,
                compared_against: None,
                findings: &findings,
                signals: &[],
            },
        )
        .await
        .expect("a lockfile run");
    }

    async fn answer(&self, name: &str, status: &str) {
        sqlx::query(
            "INSERT OR REPLACE INTO package_facts \
             (ecosystem, name, version, status, fetched_at, expires_at, checksum) \
             VALUES ('cargo', ?, '1.0.0', ?, '2026-09-22T00:00:00Z', '2026-09-22T00:15:00Z', 'published')",
        )
        .bind(name)
        .bind(status)
        .execute(&self.database.pool)
        .await
        .expect("a facts row");
    }

    async fn awaiting_audit(&self) -> Vec<Id<Snapshot>> {
        Snapshot::awaiting_audit(&self.database, self.subject.project_id, AUDIT)
            .await
            .expect("reads")
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

#[tokio::test]
async fn a_snapshot_waits_for_every_registry_answer_before_its_audit() {
    let fixture = Fixture::build().await;
    let snapshot = fixture.analyse().await;
    fixture
        .lock(
            &snapshot,
            &[
                ("serde", PackageOrigin::PublicRegistry),
                ("anyhow", PackageOrigin::PublicRegistry),
            ],
        )
        .await;

    fixture.answer("serde", "known").await;
    assert!(
        fixture.awaiting_audit().await.is_empty(),
        "audited while anyhow had no answer"
    );

    fixture.answer("anyhow", "failed").await;
    assert!(
        fixture.awaiting_audit().await.is_empty(),
        "audited while the anyhow read had failed"
    );

    fixture.answer("anyhow", "absent").await;
    assert_eq!(fixture.awaiting_audit().await, [snapshot.id]);
}

#[tokio::test]
async fn a_package_from_outside_the_public_registry_is_not_audited() {
    let fixture = Fixture::build().await;
    let snapshot = fixture.analyse().await;
    fixture
        .lock(
            &snapshot,
            &[
                (
                    "inner",
                    PackageOrigin::Registry {
                        url: "https://packages.example.invalid/index".to_owned(),
                    },
                ),
                ("utils", PackageOrigin::Local),
            ],
        )
        .await;

    assert!(
        fixture.awaiting_audit().await.is_empty(),
        "a snapshot with no public-registry package waits for an audit"
    );
}
