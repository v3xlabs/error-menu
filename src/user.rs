use jiff::Timestamp;

use crate::id::Id;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRole {
    Guest,
    Admin,
    Member,
}

impl UserRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Guest => "guest",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }

    pub const fn legacy_storage_role(self) -> &'static str {
        match self {
            Self::Guest => "member",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "guest" => Some(Self::Guest),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectRole {
    Viewer,
    Operator,
    Owner,
}

impl ProjectRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Owner => "owner",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "viewer" => Some(Self::Viewer),
            "operator" => Some(Self::Operator),
            "owner" => Some(Self::Owner),
            _ => None,
        }
    }

    pub const fn can_scan(self) -> bool {
        matches!(self, Self::Operator | Self::Owner)
    }

    pub const fn can_manage(self) -> bool {
        matches!(self, Self::Owner)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMember {
    pub user_id: Id<User>,
    pub display_name: String,
    pub role: ProjectRole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: Id<User>,
    pub oidc_issuer: String,
    pub oidc_subject: String,
    pub display_name: String,
    pub role: UserRole,
    pub created_at: Timestamp,
    pub last_signed_in_at: Timestamp,
}
