//! When each project last received new code: the newest time any of its branches or
//! changes moved to a head it had not held before. Discovery re-reads every subject on
//! every pass, so writing this onto the project row would put a write on each of those
//! reads. It is held in memory instead, rebuilt from the snapshots at boot and moved
//! forward as heads change.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use jiff::Timestamp;
use sqlx::Row;

use crate::database::codec::FromStored;
use crate::prelude::*;

#[derive(Default)]
struct Heads {
    by_subject: HashMap<Id<Subject>, CommitSha>,
    latest: HashMap<Id<Project>, Timestamp>,
}

pub struct ProjectActivity {
    heads: Mutex<Heads>,
}

impl ProjectActivity {
    pub async fn load(database: &Database) -> Result<Self, DatabaseError> {
        let rows = sqlx::query(
            "SELECT s.project_id, n.subject_id, n.head, n.observed_at \
             FROM snapshots n JOIN subjects s ON s.id = n.subject_id ORDER BY n.id",
        )
        .fetch_all(&database.pool)
        .await?;

        let mut first_seen: HashMap<(Id<Subject>, CommitSha), (Id<Project>, Timestamp)> =
            HashMap::new();
        let mut heads = Heads::default();
        for row in &rows {
            let project_id: Id<Project> = Id::from_raw(row.try_get("project_id")?);
            let subject_id: Id<Subject> = Id::from_raw(row.try_get("subject_id")?);
            let head = CommitSha::from_stored(row.try_get("head")?, "head")?;
            let observed_at = Timestamp::read(row, "observed_at")?;
            first_seen
                .entry((subject_id, head.clone()))
                .and_modify(|(_, seen)| *seen = (*seen).min(observed_at))
                .or_insert((project_id, observed_at));
            heads.by_subject.insert(subject_id, head);
        }
        for (project_id, seen) in first_seen.into_values() {
            heads
                .latest
                .entry(project_id)
                .and_modify(|latest| *latest = (*latest).max(seen))
                .or_insert(seen);
        }

        Ok(Self {
            heads: Mutex::new(heads),
        })
    }

    /// A rescan records the head its subject already holds and leaves the time alone.
    pub fn observe(&self, project_id: Id<Project>, snapshot: &Snapshot) {
        let mut heads = self.heads.lock().unwrap_or_else(PoisonError::into_inner);
        if heads.by_subject.get(&snapshot.subject_id) == Some(&snapshot.head) {
            return;
        }
        heads
            .by_subject
            .insert(snapshot.subject_id, snapshot.head.clone());
        heads
            .latest
            .entry(project_id)
            .and_modify(|latest| *latest = (*latest).max(snapshot.observed_at))
            .or_insert(snapshot.observed_at);
    }

    pub fn latest(&self, project_id: Id<Project>) -> Option<Timestamp> {
        self.heads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .latest
            .get(&project_id)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use jiff::{SignedDuration, Timestamp};

    use super::*;
    use crate::forge::ForgeMetadata;

    fn snapshot(subject_id: Id<Subject>, head: char, observed_at: Timestamp) -> Snapshot {
        Snapshot {
            id: Id::from_raw(1),
            subject_id,
            head: CommitSha::new(&head.to_string().repeat(40)).expect("valid sha"),
            base: None,
            merge_base: None,
            forge: ForgeMetadata::default(),
            observed_at,
        }
    }

    #[test]
    fn only_a_new_head_moves_the_project_forward() {
        let activity = ProjectActivity {
            heads: Mutex::new(Heads::default()),
        };
        let project_id = Id::from_raw(7);
        let subject_id = Id::from_raw(9);
        let first = Timestamp::UNIX_EPOCH;
        let later = first + SignedDuration::from_hours(1);

        activity.observe(project_id, &snapshot(subject_id, 'a', first));
        activity.observe(project_id, &snapshot(subject_id, 'a', later));
        assert_eq!(activity.latest(project_id), Some(first));

        activity.observe(project_id, &snapshot(subject_id, 'b', later));
        assert_eq!(activity.latest(project_id), Some(later));
    }
}
