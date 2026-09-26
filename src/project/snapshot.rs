use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection};

use crate::database::codec::{DecodeRow, FromStored, StoredAs};
use crate::database::{Database, DatabaseError};
use crate::forge::{ChangeState, ForgeMetadata};
use crate::id::{Id, IdGenerator};
use crate::project::Project;
use crate::project::person::{Person, PersonRole, Signature};
use crate::project::subject::Subject;
use crate::vcs::CommitSha;

/// An immutable reading of a subject at one moment. A new head sha makes a new snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: Id<Snapshot>,
    pub subject_id: Id<Subject>,
    pub head: CommitSha,
    pub base: Option<CommitSha>,
    pub merge_base: Option<CommitSha>,
    pub forge: ForgeMetadata,
    pub observed_at: Timestamp,
}

impl Snapshot {
    /// The reading of one subject at one head. Discovery records a head when it first
    /// sees it and analysis records the same head again when it finally scans it, so this
    /// takes over a reading that has no run behind it rather than leaving it beside the
    /// one that has. A re-scan of a head that was already analysed is a new reading.
    pub async fn record(
        database: &Database,
        subject_id: Id<Subject>,
        head: CommitSha,
        base: Option<CommitSha>,
        merge_base: Option<CommitSha>,
        forge: ForgeMetadata,
    ) -> Result<Snapshot, DatabaseError> {
        let observed_at = Timestamp::now();
        let mut transaction = database.write().await?;
        let unscanned: Option<i64> = sqlx::query_scalar(
            "SELECT n.id FROM snapshots n WHERE n.subject_id = ? AND n.head = ? \
             AND n.analysis_snapshot_id IS NULL \
             AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.snapshot_id = n.id) \
             ORDER BY n.id LIMIT 1",
        )
        .bind(subject_id.raw())
        .bind(head.as_str())
        .fetch_optional(&mut *transaction)
        .await?;
        let id: Id<Snapshot> = unscanned.map_or_else(|| database.ids.next(), Id::from_raw);

        sqlx::query(
            "INSERT INTO snapshots \
             (id, subject_id, head, base, merge_base, forge_title, forge_body, forge_author, forge_url, forge_base_ref, forge_head_ref, forge_state, forge_merge_commit, signature_present, signature_verified, signature_signer, signature_reason, observed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
             base = excluded.base, merge_base = excluded.merge_base, forge_title = excluded.forge_title, \
             forge_body = excluded.forge_body, forge_author = excluded.forge_author, forge_url = excluded.forge_url, \
             forge_base_ref = excluded.forge_base_ref, forge_head_ref = excluded.forge_head_ref, \
             forge_state = excluded.forge_state, forge_merge_commit = excluded.forge_merge_commit, \
             signature_present = excluded.signature_present, signature_verified = excluded.signature_verified, \
             signature_signer = excluded.signature_signer, signature_reason = excluded.signature_reason, \
             observed_at = excluded.observed_at",
        )
        .bind(id.raw())
        .bind(subject_id.raw())
        .bind(head.as_str())
        .bind(base.as_ref().map(CommitSha::as_str))
        .bind(merge_base.as_ref().map(CommitSha::as_str))
        .bind(forge.title.as_deref())
        .bind(forge.body.as_deref())
        .bind(forge.author.as_deref())
        .bind(forge.url.as_deref())
        .bind(forge.base_ref.as_deref())
        .bind(forge.head_ref.as_deref())
        .bind(forge.state.as_ref().map(ChangeState::stored))
        .bind(forge.merge_commit.as_ref().map(CommitSha::as_str))
        .bind(i64::from(forge.signature.present))
        .bind(forge.signature.verified.map(i64::from))
        .bind(forge.signature.signer.as_deref())
        .bind(forge.signature.reason.as_deref())
        .bind(observed_at.to_string())
        .execute(&mut *transaction)
        .await?;

        sqlx::query("DELETE FROM snapshot_people WHERE snapshot_id = ?")
            .bind(id.raw())
            .execute(&mut *transaction)
            .await?;
        insert_people(&mut transaction, &database.ids, id, &forge.people).await?;

        transaction.commit().await?;

        Ok(Snapshot {
            id,
            subject_id,
            head,
            base,
            merge_base,
            forge,
            observed_at,
        })
    }

    pub async fn indexed(
        database: &Database,
        project_id: Id<Project>,
        head: &CommitSha,
    ) -> Result<Option<Snapshot>, DatabaseError> {
        let row = sqlx::query(
            "SELECT n.* FROM commit_analyses a JOIN snapshots n ON n.id = a.snapshot_id \
             WHERE a.project_id = ? AND a.head = ?",
        )
        .bind(project_id.raw())
        .bind(head.as_str())
        .fetch_optional(&database.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut snapshot = Snapshot::decode_row(&row)?;
        snapshot.forge.people = people_of(database, snapshot.id).await?;
        Ok(Some(snapshot))
    }

    /// The pull request a head was last seen as, if it was seen as one. A re-run asked for
    /// on the forge names a commit, and a re-scan is asked of the change.
    pub async fn change_with_head(
        database: &Database,
        project_id: Id<Project>,
        head: &CommitSha,
    ) -> Result<Option<u64>, DatabaseError> {
        let key: Option<String> = sqlx::query_scalar(
            "SELECT s.subject_key FROM snapshots n JOIN subjects s ON s.id = n.subject_id \
             WHERE s.project_id = ? AND s.kind = 'change' AND n.head = ? \
             ORDER BY n.id DESC LIMIT 1",
        )
        .bind(project_id.raw())
        .bind(head.as_str())
        .fetch_optional(&database.pool)
        .await?;

        key.map(|key| {
            key.parse().map_err(|_| DatabaseError::Unreadable {
                field: "subjects.subject_key",
                value: key,
            })
        })
        .transpose()
    }

    pub async fn observe(
        database: &Database,
        subject_id: Id<Subject>,
        head: CommitSha,
        base: Option<CommitSha>,
        mut forge: ForgeMetadata,
        indexed: Option<&Snapshot>,
    ) -> Result<Snapshot, DatabaseError> {
        let row = sqlx::query(
            "SELECT * FROM snapshots WHERE subject_id = ? AND head = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(subject_id.raw())
        .bind(head.as_str())
        .fetch_optional(&database.pool)
        .await?;
        let previous_analysis = row
            .as_ref()
            .map(|row| row.try_get::<Option<i64>, _>("analysis_snapshot_id"))
            .transpose()?
            .flatten();
        let previous = row.as_ref().map(Snapshot::decode_row).transpose()?;
        let mut stored_people = match previous.as_ref() {
            Some(snapshot) => snapshot.people(database).await?,
            None => Vec::new(),
        };
        if let Some(snapshot) = indexed {
            stored_people.extend(snapshot.forge.people.iter().cloned());
        }
        forge.people.extend(
            stored_people.into_iter().filter(|person| {
                !matches!(person.role, PersonRole::Submitter | PersonRole::Reviewer)
            }),
        );
        forge.people = crate::project::person::deduplicate(forge.people);
        if let Some(snapshot) = previous
            .as_ref()
            .filter(|snapshot| snapshot.forge.signature.present)
            .or(indexed)
        {
            forge.signature = snapshot.forge.signature.clone();
        }
        let base = base.or_else(|| {
            previous
                .as_ref()
                .or(indexed)
                .and_then(|snapshot| snapshot.base.clone())
        });
        let merge_base = previous
            .as_ref()
            .or(indexed)
            .and_then(|snapshot| snapshot.merge_base.clone());
        let Some(mut snapshot) = previous else {
            let snapshot =
                Snapshot::record(database, subject_id, head, base, merge_base, forge).await?;
            if let Some(indexed) = indexed {
                sqlx::query("UPDATE snapshots SET analysis_snapshot_id = ? WHERE id = ?")
                    .bind(indexed.id.raw())
                    .bind(snapshot.id.raw())
                    .execute(&database.pool)
                    .await?;
            }
            return Ok(snapshot);
        };
        let analysis_snapshot_id = indexed
            .filter(|indexed| indexed.id != snapshot.id)
            .map(|indexed| indexed.id.raw());
        snapshot.forge.people = people_of(database, snapshot.id).await?;
        if snapshot.forge == forge
            && snapshot.base == base
            && previous_analysis == analysis_snapshot_id
        {
            return Ok(snapshot);
        }
        let mut transaction = database.write().await?;
        sqlx::query(
            "UPDATE snapshots SET base = ?, forge_title = ?, forge_body = ?, forge_author = ?, \
             forge_url = ?, forge_base_ref = ?, forge_head_ref = ?, forge_state = ?, forge_merge_commit = ?, \
             signature_present = ?, signature_verified = ?, signature_signer = ?, signature_reason = ?, \
             analysis_snapshot_id = ? WHERE id = ?",
        )
        .bind(base.as_ref().map(CommitSha::as_str))
        .bind(forge.title.as_deref())
        .bind(forge.body.as_deref())
        .bind(forge.author.as_deref())
        .bind(forge.url.as_deref())
        .bind(forge.base_ref.as_deref())
        .bind(forge.head_ref.as_deref())
        .bind(forge.state.as_ref().map(ChangeState::stored))
        .bind(forge.merge_commit.as_ref().map(CommitSha::as_str))
        .bind(i64::from(forge.signature.present))
        .bind(forge.signature.verified.map(i64::from))
        .bind(forge.signature.signer.as_deref())
        .bind(forge.signature.reason.as_deref())
        .bind(analysis_snapshot_id)
        .bind(snapshot.id.raw())
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM snapshot_people WHERE snapshot_id = ?")
            .bind(snapshot.id.raw())
            .execute(&mut *transaction)
            .await?;
        insert_people(&mut transaction, &database.ids, snapshot.id, &forge.people).await?;
        transaction.commit().await?;
        snapshot.base = base;
        snapshot.forge = forge;
        Ok(snapshot)
    }

    /// The newest reading of one change, with the people it named. Re-scanning a change
    /// that discovery only recorded needs the range and the metadata it was recorded with.
    pub async fn latest_for_change(
        database: &Database,
        project_id: Id<Project>,
        number: u64,
    ) -> Result<Option<Snapshot>, DatabaseError> {
        let row = sqlx::query(
            "SELECT n.id, n.subject_id, n.head, n.base, n.merge_base, n.forge_title, n.forge_body, \
                    n.forge_author, n.forge_url, n.forge_base_ref, n.forge_head_ref, n.forge_state, \
                    n.forge_merge_commit, n.signature_present, n.signature_verified, \
                    n.signature_signer, n.signature_reason, n.observed_at \
             FROM snapshots n JOIN subjects s ON s.id = n.subject_id \
             WHERE s.project_id = ? AND s.kind = 'change' AND s.subject_key = ? \
             ORDER BY n.id DESC LIMIT 1",
        )
        .bind(project_id.raw())
        .bind(number.to_string())
        .fetch_optional(&database.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let mut snapshot = Snapshot::decode_row(&row)?;
        snapshot.forge.people = people_of(database, snapshot.id).await?;

        Ok(Some(snapshot))
    }

    /// The newest head recorded for a branch subject of this project. Reading a file out
    /// of the repository needs a revision, and the default branch is the one a project's
    /// own mark should follow.
    pub async fn default_branch_head(
        database: &Database,
        project_id: Id<Project>,
    ) -> Result<Option<CommitSha>, DatabaseError> {
        let row = sqlx::query(
            "SELECT n.head FROM snapshots n JOIN subjects s ON s.id = n.subject_id \
             WHERE s.project_id = ? AND s.kind = 'branch' ORDER BY n.id DESC LIMIT 1",
        )
        .bind(project_id.raw())
        .fetch_optional(&database.pool)
        .await?;

        row.map(|row| CommitSha::from_stored(row.try_get("head")?, "head"))
            .transpose()
    }

    /// Snapshots of this project whose public-registry packages recorded an integrity, have
    /// an answer from the registry for every one of them, and have not been audited yet. The
    /// audit run is its own record that it happened, so it must not be written while a
    /// coordinate is unanswered: that coordinate would never be compared.
    pub async fn awaiting_audit(
        database: &Database,
        project_id: Id<Project>,
        audit: &str,
    ) -> Result<Vec<Id<Snapshot>>, DatabaseError> {
        let rows = sqlx::query(
            "SELECT DISTINCT n.id FROM snapshots n \
             JOIN subjects s ON s.id = n.subject_id \
             JOIN runs r ON r.snapshot_id = n.id \
             JOIN findings f ON f.run_id = r.id \
             WHERE s.project_id = ? AND f.package_integrity IS NOT NULL \
               AND f.origin_kind = 'public_registry' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM runs a WHERE a.snapshot_id = n.id AND a.analyzer = ? \
               ) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM runs ur JOIN findings u ON u.run_id = ur.id \
                   WHERE ur.snapshot_id = n.id AND u.package_integrity IS NOT NULL \
                     AND u.origin_kind = 'public_registry' \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM package_facts p \
                         WHERE p.ecosystem = u.ecosystem AND p.name = u.package_name \
                           AND p.version = u.package_version \
                           AND p.status IN ('known', 'absent') \
                     ) \
               )",
        )
        .bind(project_id.raw())
        .bind(audit)
        .fetch_all(&database.pool)
        .await?;

        rows.into_iter()
            .map(|row| Ok(Id::from_raw(row.try_get("id")?)))
            .collect()
    }

    pub async fn people(&self, database: &Database) -> Result<Vec<Person>, DatabaseError> {
        people_of(database, self.id).await
    }

    pub async fn mark_analysed(&self, database: &Database) -> Result<(), DatabaseError> {
        sqlx::query(
            "INSERT INTO commit_analyses (project_id, head, snapshot_id) \
             SELECT s.project_id, n.head, n.id FROM snapshots n JOIN subjects s ON s.id = n.subject_id \
             WHERE n.id = ? AND n.analysis_snapshot_id IS NULL \
             ON CONFLICT(project_id, head) DO UPDATE SET snapshot_id = excluded.snapshot_id",
        )
        .bind(self.id.raw())
        .execute(&database.pool)
        .await?;
        Ok(())
    }
}

impl DecodeRow for Snapshot {
    fn decode_row(row: &SqliteRow) -> Result<Self, DatabaseError> {
        Ok(Snapshot {
            id: Id::from_raw(row.try_get("id")?),
            subject_id: Id::from_raw(row.try_get("subject_id")?),
            head: CommitSha::from_stored(row.try_get("head")?, "head")?,
            base: row
                .try_get::<Option<&str>, _>("base")?
                .map(|sha| CommitSha::from_stored(sha, "base"))
                .transpose()?,
            merge_base: row
                .try_get::<Option<&str>, _>("merge_base")?
                .map(|sha| CommitSha::from_stored(sha, "merge_base"))
                .transpose()?,
            forge: ForgeMetadata {
                title: row.try_get("forge_title")?,
                body: row.try_get("forge_body")?,
                author: row.try_get("forge_author")?,
                url: row.try_get("forge_url")?,
                base_ref: row.try_get("forge_base_ref")?,
                head_ref: row.try_get("forge_head_ref")?,
                state: row
                    .try_get::<Option<&str>, _>("forge_state")?
                    .map(ChangeState::from_stored)
                    .transpose()?,
                merge_commit: row
                    .try_get::<Option<&str>, _>("forge_merge_commit")?
                    .map(|sha| CommitSha::from_stored(sha, "forge_merge_commit"))
                    .transpose()?,
                people: Vec::new(),
                signature: Signature {
                    present: row.try_get::<i64, _>("signature_present")? != 0,
                    verified: row
                        .try_get::<Option<i64>, _>("signature_verified")?
                        .map(|value| value != 0),
                    signer: row.try_get("signature_signer")?,
                    reason: row.try_get("signature_reason")?,
                },
            },
            observed_at: Timestamp::read(row, "observed_at")?,
        })
    }
}

async fn people_of(
    database: &Database,
    snapshot_id: Id<Snapshot>,
) -> Result<Vec<Person>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT role, person_name, email, login, avatar_url \
         FROM snapshot_people WHERE snapshot_id = ? ORDER BY id",
    )
    .bind(snapshot_id.raw())
    .fetch_all(&database.pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(Person {
                role: PersonRole::read(&row, "role")?,
                name: row.try_get("person_name")?,
                email: row.try_get("email")?,
                login: row.try_get("login")?,
                avatar_url: row.try_get("avatar_url")?,
            })
        })
        .collect()
}

async fn insert_people(
    connection: &mut SqliteConnection,
    ids: &IdGenerator,
    snapshot_id: Id<Snapshot>,
    people: &[Person],
) -> Result<(), DatabaseError> {
    for person in people {
        sqlx::query(
            "INSERT INTO snapshot_people \
             (id, snapshot_id, role, person_name, email, login, avatar_url, identity) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(ids.next::<Person>().raw())
        .bind(snapshot_id.raw())
        .bind(person.role.stored())
        .bind(person.name.as_deref())
        .bind(person.email.as_deref())
        .bind(person.login.as_deref())
        .bind(person.avatar_url.as_deref())
        .bind(person.identity())
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}
