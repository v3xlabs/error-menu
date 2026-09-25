use sqlx::Row;

use crate::forge::ForgeKind;
use crate::forge::reader::Repository;
use crate::prelude::*;

/// One installation of the GitHub App, as far as it covers one repository. Its id is what
/// a token is asked for; GitHub gives it out in every webhook the installation causes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GithubInstallation {
    pub id: i64,
}

/// A repository an installation covers, as a webhook names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveredRepository {
    pub id: i64,
    pub full_name: String,
}

impl GithubInstallation {
    /// The installation that covers a project's repository. Only a repository on github.com
    /// can have one: the app is registered there, and an Enterprise server has its own apps.
    pub async fn for_project(
        database: &Database,
        project: &Project,
    ) -> Result<Option<Self>, DatabaseError> {
        let Some(full_name) = full_name(&project.remote, project.forge_kind) else {
            return Ok(None);
        };
        let row = sqlx::query(
            "SELECT installation_id FROM github_installation_repositories WHERE full_name = ? \
             ORDER BY repository_id LIMIT 1",
        )
        .bind(&full_name)
        .fetch_optional(&database.pool)
        .await?;

        row.map(|row| {
            Ok(GithubInstallation {
                id: row.try_get("installation_id")?,
            })
        })
        .transpose()
    }

    /// Records that this installation covers these repositories. A repository renamed on
    /// GitHub keeps its id, so the row follows the rename the next time a webhook names it.
    pub async fn cover(
        self,
        database: &Database,
        repositories: &[CoveredRepository],
    ) -> Result<(), DatabaseError> {
        let mut transaction = database.write().await?;
        for repository in repositories {
            sqlx::query(
                "INSERT INTO github_installation_repositories \
                 (repository_id, installation_id, full_name) VALUES (?, ?, ?) \
                 ON CONFLICT(repository_id) DO UPDATE SET \
                 installation_id = excluded.installation_id, full_name = excluded.full_name",
            )
            .bind(repository.id)
            .bind(self.id)
            .bind(repository.full_name.to_ascii_lowercase())
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;

        Ok(())
    }

    pub async fn uncover(
        self,
        database: &Database,
        repository_ids: &[i64],
    ) -> Result<(), DatabaseError> {
        let mut transaction = database.write().await?;
        for repository_id in repository_ids {
            sqlx::query(
                "DELETE FROM github_installation_repositories \
                 WHERE repository_id = ? AND installation_id = ?",
            )
            .bind(repository_id)
            .bind(self.id)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;

        Ok(())
    }

    /// An installation that was removed or suspended covers nothing any more.
    pub async fn remove(self, database: &Database) -> Result<(), DatabaseError> {
        sqlx::query("DELETE FROM github_installation_repositories WHERE installation_id = ?")
            .bind(self.id)
            .execute(&database.pool)
            .await?;

        Ok(())
    }
}

/// The lowercased `owner/name` of a repository on github.com, the spelling a webhook's
/// `full_name` is matched against. `None` for any other forge or host.
pub fn full_name(remote: &RemoteUrl, kind: ForgeKind) -> Option<String> {
    let repository = Repository::from_remote(remote, kind).ok()?;

    repository
        .on_public_github()
        .then(|| repository.project_path().to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_github_com_repository_has_a_full_name() {
        let named = |remote: &str, kind| full_name(&RemoteUrl::new(remote).unwrap(), kind);

        assert_eq!(
            named(
                "https://github.com/Open-Lavatory/Open-Lavatory.git",
                ForgeKind::Auto
            ),
            Some("open-lavatory/open-lavatory".to_owned())
        );
        assert_eq!(
            named(
                "https://git.example.invalid/team/service",
                ForgeKind::Github
            ),
            None
        );
        assert_eq!(
            named("https://gitlab.com/group/service", ForgeKind::Auto),
            None
        );
    }
}
