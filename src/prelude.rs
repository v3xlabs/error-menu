//! What nearly every module needs: the database it reads, the things error.menu watches,
//! the things it produces, and the people it names. Subsystems are imported by name, so a
//! module's imports still say which of them it reaches for.

pub use crate::analysis::confidence::Confidence;
pub use crate::analysis::finding::{
    Attribution, Ecosystem, Finding, LineSpan, Location, NewFinding, Severity, VersionMovement,
};
pub use crate::analysis::signal::{NewSignal, Score, Signal, SignalKey, SignalValue};
pub use crate::analysis::{Run, RunStatus};
pub use crate::database::{Database, DatabaseError};
pub use crate::id::Id;
pub use crate::organization::Organization;
pub use crate::organization::member::{OrganizationMember, OrganizationRole};
pub use crate::project::member::{ProjectMember, ProjectRole};
pub use crate::project::person::{Person, PersonRole, Signature};
pub use crate::project::snapshot::Snapshot;
pub use crate::project::subject::{Subject, SubjectKind};
pub use crate::project::transfer::ProjectTransfer;
pub use crate::project::{Project, ProjectIcon};
pub use crate::user::{User, UserRole};
pub use crate::vcs::{CommitSha, RemoteUrl, RepoPath};
