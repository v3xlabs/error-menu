use jiff::Timestamp;
use sqlx::Row;

use crate::forge::{ChangeState, ForgeMetadata};
use crate::id::Id;
use crate::person::{Person, PersonRole, Signature};
use crate::vcs::CommitSha;
use crate::watch::{Project, Snapshot, Subject, SubjectKind};

use super::{Store, StoreError};

impl Store {
    pub async fn upsert_subject(
        &self,
        project_id: Id<Project>,
        kind: SubjectKind,
    ) -> Result<Subject, StoreError> {
        let (kind_text, key) = encode_subject_kind(&kind);
        let id: Id<Subject> = self.ids.next();

        let row = sqlx::query(
            "INSERT INTO subjects (id, project_id, kind, subject_key) VALUES (?, ?, ?, ?) \
             ON CONFLICT(project_id, kind, subject_key) DO NOTHING RETURNING id",
        )
        .bind(id.raw())
        .bind(project_id.raw())
        .bind(kind_text)
        .bind(&key)
        .fetch_optional(&self.pool)
        .await?;
        let id = match row {
            Some(row) => Id::from_raw(row.try_get("id")?),
            None => {
                let row = sqlx::query(
                    "SELECT id FROM subjects WHERE project_id = ? AND kind = ? AND subject_key = ?",
                )
                .bind(project_id.raw())
                .bind(kind_text)
                .bind(&key)
                .fetch_one(&self.pool)
                .await?;
                Id::from_raw(row.try_get("id")?)
            }
        };

        Ok(Subject {
            id,
            project_id,
            kind,
        })
    }

    pub async fn record_snapshot(
        &self,
        subject_id: Id<Subject>,
        head: CommitSha,
        base: Option<CommitSha>,
        merge_base: Option<CommitSha>,
        forge: ForgeMetadata,
    ) -> Result<Snapshot, StoreError> {
        let id: Id<Snapshot> = self.ids.next();
        let observed_at = Timestamp::now();
        let mut transaction = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO snapshots \
             (id, subject_id, head, base, merge_base, forge_title, forge_body, forge_author, forge_url, forge_base_ref, forge_head_ref, forge_state, forge_merge_commit, signature_present, signature_verified, signature_signer, signature_reason, observed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(forge.state.map(encode_change_state))
        .bind(forge.merge_commit.as_ref().map(CommitSha::as_str))
        .bind(i64::from(forge.signature.present))
        .bind(forge.signature.verified.map(i64::from))
        .bind(forge.signature.signer.as_deref())
        .bind(forge.signature.reason.as_deref())
        .bind(observed_at.to_string())
        .execute(&mut *transaction)
        .await?;

        for person in &forge.people {
            sqlx::query(
                "INSERT INTO snapshot_people \
                 (id, snapshot_id, role, person_name, email, login, avatar_url, identity) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(self.ids.next::<Person>().raw())
            .bind(id.raw())
            .bind(encode_person_role(person.role))
            .bind(person.name.as_deref())
            .bind(person.email.as_deref())
            .bind(person.login.as_deref())
            .bind(person.avatar_url.as_deref())
            .bind(person.identity())
            .execute(&mut *transaction)
            .await?;
        }

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

    /// The newest reading of one change, with the people it named. Re-scanning a change
    /// that discovery only recorded needs the range and the metadata it was recorded with.
    pub async fn latest_snapshot_for_change(
        &self,
        project_id: Id<Project>,
        number: u64,
    ) -> Result<Option<Snapshot>, StoreError> {
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
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let mut snapshot = decode_snapshot(row)?;
        snapshot.forge.people = self.people_for_snapshot(snapshot.id).await?;

        Ok(Some(snapshot))
    }

    pub async fn people_for_snapshot(
        &self,
        snapshot_id: Id<Snapshot>,
    ) -> Result<Vec<Person>, StoreError> {
        let rows = sqlx::query(
            "SELECT role, person_name, email, login, avatar_url \
             FROM snapshot_people WHERE snapshot_id = ? ORDER BY id",
        )
        .bind(snapshot_id.raw())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(Person {
                    role: decode_person_role(row.try_get("role")?)?,
                    name: row.try_get("person_name")?,
                    email: row.try_get("email")?,
                    login: row.try_get("login")?,
                    avatar_url: row.try_get("avatar_url")?,
                })
            })
            .collect()
    }

    /// The avatar a forge last gave for this person. One human reaches us under several
    /// identities: the forge knows them by login, while their commits are keyed by the
    /// address they committed with. When the address has no picture, the same login does,
    /// so a bot that opened a change is not drawn twice in two different ways.
    pub async fn avatar_url_for_identity(
        &self,
        identity: &str,
    ) -> Result<Option<String>, StoreError> {
        let own = sqlx::query(
            "SELECT avatar_url FROM snapshot_people \
             WHERE identity = ? AND avatar_url IS NOT NULL ORDER BY id DESC LIMIT 1",
        )
        .bind(identity)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = own {
            return Ok(Some(row.try_get("avatar_url")?));
        }

        let row = sqlx::query(
            "SELECT other.avatar_url FROM snapshot_people mine \
             JOIN snapshot_people other \
               ON other.login IS NOT NULL \
              AND other.avatar_url IS NOT NULL \
              AND lower(other.login) IN (lower(mine.login), lower(mine.person_name)) \
             WHERE mine.identity = ? ORDER BY other.id DESC LIMIT 1",
        )
        .bind(identity)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| row.try_get("avatar_url")).transpose()?)
    }

    /// The newest head recorded for a branch subject of this project. Reading a file out
    /// of the repository needs a revision, and the default branch is the one a project's
    /// own mark should follow.
    pub async fn default_branch_head(
        &self,
        project_id: Id<Project>,
    ) -> Result<Option<CommitSha>, StoreError> {
        let row = sqlx::query(
            "SELECT n.head FROM snapshots n JOIN subjects s ON s.id = n.subject_id \
             WHERE s.project_id = ? AND s.kind = 'branch' ORDER BY n.id DESC LIMIT 1",
        )
        .bind(project_id.raw())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| decode_sha(row.try_get("head")?, "head"))
            .transpose()
    }
}

pub(crate) fn decode_snapshot(row: sqlx::sqlite::SqliteRow) -> Result<Snapshot, StoreError> {
    Ok(Snapshot {
        id: Id::from_raw(row.try_get("id")?),
        subject_id: Id::from_raw(row.try_get("subject_id")?),
        head: decode_sha(row.try_get("head")?, "head")?,
        base: row
            .try_get::<Option<String>, _>("base")?
            .map(|sha| decode_sha(sha, "base"))
            .transpose()?,
        merge_base: row
            .try_get::<Option<String>, _>("merge_base")?
            .map(|sha| decode_sha(sha, "merge_base"))
            .transpose()?,
        forge: ForgeMetadata {
            title: row.try_get("forge_title")?,
            body: row.try_get("forge_body")?,
            author: row.try_get("forge_author")?,
            url: row.try_get("forge_url")?,
            base_ref: row.try_get("forge_base_ref")?,
            head_ref: row.try_get("forge_head_ref")?,
            state: row
                .try_get::<Option<String>, _>("forge_state")?
                .map(decode_change_state)
                .transpose()?,
            merge_commit: row
                .try_get::<Option<String>, _>("forge_merge_commit")?
                .map(|sha| decode_sha(sha, "forge_merge_commit"))
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
        observed_at: decode_timestamp(row.try_get("observed_at")?)?,
    })
}

fn decode_sha(sha: String, field: &'static str) -> Result<CommitSha, StoreError> {
    CommitSha::new(&sha).map_err(|_| StoreError::Unreadable { field, value: sha })
}

pub(crate) fn decode_subject(row: &sqlx::sqlite::SqliteRow) -> Result<Subject, StoreError> {
    Ok(Subject {
        id: Id::from_raw(row.try_get("subject_id")?),
        project_id: Id::from_raw(row.try_get("project_id")?),
        kind: decode_subject_kind(row.try_get("subject_kind")?, row.try_get("subject_key")?)?,
    })
}

fn decode_subject_kind(kind: String, key: String) -> Result<SubjectKind, StoreError> {
    match kind.as_str() {
        "change" => key
            .parse()
            .map(|number| SubjectKind::Change { number })
            .map_err(|_| StoreError::Unreadable {
                field: "subject_key",
                value: key,
            }),
        "branch" => Ok(SubjectKind::Branch { name: key }),
        "commit" => decode_sha(key, "subject_key").map(|sha| SubjectKind::Commit { sha }),
        _ => Err(StoreError::Unreadable {
            field: "subject_kind",
            value: kind,
        }),
    }
}

fn encode_subject_kind(kind: &SubjectKind) -> (&'static str, String) {
    match kind {
        SubjectKind::Change { number } => ("change", number.to_string()),
        SubjectKind::Branch { name } => ("branch", name.clone()),
        SubjectKind::Commit { sha } => ("commit", sha.to_string()),
    }
}

fn encode_change_state(state: ChangeState) -> &'static str {
    match state {
        ChangeState::Open => "open",
        ChangeState::Closed => "closed",
        ChangeState::Merged => "merged",
    }
}

fn decode_change_state(state: String) -> Result<ChangeState, StoreError> {
    match state.as_str() {
        "open" => Ok(ChangeState::Open),
        "closed" => Ok(ChangeState::Closed),
        "merged" => Ok(ChangeState::Merged),
        _ => Err(StoreError::Unreadable {
            field: "forge_state",
            value: state,
        }),
    }
}

fn decode_timestamp(timestamp: String) -> Result<Timestamp, StoreError> {
    timestamp.parse().map_err(|_| StoreError::Unreadable {
        field: "timestamp",
        value: timestamp,
    })
}

fn encode_person_role(role: PersonRole) -> &'static str {
    match role {
        PersonRole::Author => "author",
        PersonRole::Committer => "committer",
        PersonRole::CoAuthor => "co_author",
        PersonRole::SignedOffBy => "signed_off_by",
        PersonRole::Submitter => "submitter",
        PersonRole::Reviewer => "reviewer",
    }
}

fn decode_person_role(role: String) -> Result<PersonRole, StoreError> {
    match role.as_str() {
        "author" => Ok(PersonRole::Author),
        "committer" => Ok(PersonRole::Committer),
        "co_author" => Ok(PersonRole::CoAuthor),
        "signed_off_by" => Ok(PersonRole::SignedOffBy),
        "submitter" => Ok(PersonRole::Submitter),
        "reviewer" => Ok(PersonRole::Reviewer),
        _ => Err(StoreError::Unreadable {
            field: "role",
            value: role,
        }),
    }
}
