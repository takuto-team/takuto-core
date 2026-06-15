// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Domain models for the multi-user authentication system.

use serde::{Deserialize, Serialize};

/// User role in the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    User,
}

impl UserRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
        }
    }
}

impl std::str::FromStr for UserRole {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Self::Admin),
            "user" => Ok(Self::User),
            _ => Err(format!("unknown user role: {s}")),
        }
    }
}

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A Takuto user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub role: UserRole,
    pub suspended: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Credential kind discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialKind {
    Password,
    Passkey,
}

impl CredentialKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Passkey => "passkey",
        }
    }
}

impl std::str::FromStr for CredentialKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "password" => Ok(Self::Password),
            "passkey" => Ok(Self::Passkey),
            _ => Err(format!("unknown credential kind: {s}")),
        }
    }
}

/// A stored credential (password hash or passkey data).
#[derive(Debug, Clone)]
pub struct Credential {
    pub id: String,
    pub user_id: String,
    pub kind: CredentialKind,
    pub data: Vec<u8>,
    pub label: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// A one-time recovery code (hashed).
#[derive(Debug, Clone)]
pub struct RecoveryCode {
    pub id: String,
    pub user_id: String,
    pub code_hash: Vec<u8>,
    pub used: bool,
    pub created_at: String,
}

/// Per-user repository access record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRepository {
    pub id: String,
    pub user_id: String,
    pub repo_url: String,
    pub local_path: String,
    pub added_at: String,
}

/// Maps a Takuto user to an ephemeral OS user inside a container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerUser {
    pub id: String,
    pub user_id: String,
    pub container_id: String,
    pub container_type: String,
    pub os_username: String,
    pub created_at: String,
    pub destroyed_at: Option<String>,
}

/// Request to create a new user.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: Option<String>,
    pub role: Option<UserRole>,
}

/// Request to update a user.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateUserRequest {
    pub username: Option<String>,
    pub role: Option<UserRole>,
}

/// Export format for a user (no credentials, no repos).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserExport {
    pub username: String,
    pub role: UserRole,
    pub suspended: bool,
    pub created_at: String,
}

/// Summary of an import operation.
#[derive(Debug, Serialize)]
pub struct ImportSummary {
    pub created: Vec<String>,
    pub skipped: Vec<SkippedUser>,
}

/// A user that was skipped during import, with the reason.
#[derive(Debug, Serialize)]
pub struct SkippedUser {
    pub username: String,
    pub reason: String,
}
