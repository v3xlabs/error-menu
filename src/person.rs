use serde::{Deserialize, Serialize};

/// How a person is attached to a change. Git writes every one of these from the client,
/// so a role records a claim and not a proven fact: a rebase, a squash, or a hand-edited
/// trailer can each put the wrong name here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonRole {
    Author,
    Committer,
    CoAuthor,
    SignedOffBy,
    Submitter,
    Reviewer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub role: PersonRole,
    pub name: Option<String>,
    pub email: Option<String>,
    pub login: Option<String>,
    pub avatar_url: Option<String>,
}

impl Person {
    /// Groups the same human across snapshots, so their avatar is fetched once and their
    /// other work can be found. A forge login is the strongest claim, because one person
    /// commits under several addresses and the forge holds the mapping. An address comes
    /// next, and a display name last, because a name changes freely.
    pub fn identity(&self) -> String {
        let claim = self
            .login
            .as_deref()
            .or(self.email.as_deref())
            .or(self.name.as_deref())
            .unwrap_or("unknown");

        blake3::hash(claim.to_lowercase().as_bytes()).to_hex()[..32].to_owned()
    }

    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .or(self.login.as_deref())
            .or(self.email.as_deref())
            .unwrap_or("unknown")
    }
}

/// Reads `Name <email>` as git writes it in a trailer. A trailer that carries only a name
/// is kept, because a person named without an address is still worth showing.
pub fn parse_identity(value: &str) -> (Option<String>, Option<String>) {
    let value = value.trim();
    let Some((name, address)) = value.rsplit_once('<') else {
        return (non_empty(value), None);
    };

    (
        non_empty(name.trim()),
        non_empty(address.trim_end().trim_end_matches('>').trim()),
    )
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

/// One entry per identity and role. A commit that names the same person as author and as
/// committer is the ordinary case, and showing them twice in one row reads as noise.
pub fn deduplicate(people: Vec<Person>) -> Vec<Person> {
    let mut seen = std::collections::BTreeSet::new();

    people
        .into_iter()
        .filter(|person| seen.insert((person.role, person.identity())))
        .collect()
}

/// What a commit's own bytes say about a signature, and what the forge says about it.
/// error.menu never checks the cryptography itself, so `verified` repeats the forge's
/// verdict and is absent when no forge was asked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub present: bool,
    pub verified: Option<bool>,
    pub signer: Option<String>,
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailer_carries_a_name_and_an_address() {
        assert_eq!(
            parse_identity("Ada Lovelace <ada@example.invalid>"),
            (
                Some("Ada Lovelace".to_owned()),
                Some("ada@example.invalid".to_owned())
            )
        );
    }

    #[test]
    fn a_trailer_may_carry_only_a_name() {
        assert_eq!(
            parse_identity("Ada Lovelace"),
            (Some("Ada Lovelace".to_owned()), None)
        );
    }

    #[test]
    fn one_person_keeps_one_identity_across_spellings_of_their_address() {
        let lower = Person {
            role: PersonRole::Author,
            name: Some("Ada".to_owned()),
            email: Some("ada@example.invalid".to_owned()),
            login: None,
            avatar_url: None,
        };
        let upper = Person {
            name: Some("A. Lovelace".to_owned()),
            email: Some("Ada@Example.Invalid".to_owned()),
            ..lower.clone()
        };

        assert_eq!(lower.identity(), upper.identity());
    }

    #[test]
    fn a_login_outranks_an_address_so_one_human_is_one_identity() {
        let from_forge = Person {
            role: PersonRole::Submitter,
            name: None,
            email: None,
            login: Some("lucemans".to_owned()),
            avatar_url: Some("https://avatars.example.invalid/lucemans".to_owned()),
        };
        let from_commit = Person {
            role: PersonRole::Author,
            name: Some("Luc".to_owned()),
            email: Some("luc@example.invalid".to_owned()),
            login: Some("lucemans".to_owned()),
            avatar_url: None,
        };

        assert_eq!(from_forge.identity(), from_commit.identity());
    }

    #[test]
    fn the_same_person_in_two_roles_is_kept_twice() {
        let author = Person {
            role: PersonRole::Author,
            name: Some("Ada".to_owned()),
            email: Some("ada@example.invalid".to_owned()),
            login: None,
            avatar_url: None,
        };
        let committer = Person {
            role: PersonRole::Committer,
            ..author.clone()
        };

        let people = deduplicate(vec![author.clone(), author, committer]);

        assert_eq!(people.len(), 2);
    }
}
