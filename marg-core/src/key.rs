use chrono::{DateTime, Utc};
use data_encoding::BASE32_NOPAD;
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;

use crate::principal::{Principal, PrincipalKind};

pub const TOKEN_PREFIX: &str = "marg_live_";
const TOKEN_RANDOM_BYTES: usize = 32;

pub struct MargToken(SecretString);

impl MargToken {
    pub fn generate() -> Self {
        let mut bytes = [0u8; TOKEN_RANDOM_BYTES];
        rand::thread_rng().fill_bytes(&mut bytes);
        let encoded = BASE32_NOPAD.encode(&bytes).to_lowercase();
        let token = format!("{}{}", TOKEN_PREFIX, encoded);
        Self(SecretString::new(token.into_boxed_str()))
    }

    pub fn from_str(s: &str) -> Self {
        Self(SecretString::new(s.to_string().into_boxed_str()))
    }

    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }

    pub fn hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.expose().as_bytes());
        let digest = hasher.finalize();
        data_encoding::HEXLOWER.encode(&digest)
    }

    pub fn display_prefix(&self) -> String {
        let raw = self.expose();
        if raw.len() <= TOKEN_PREFIX.len() + 8 {
            return raw.to_string();
        }
        let head = &raw[..TOKEN_PREFIX.len() + 4];
        let tail = &raw[raw.len() - 4..];
        format!("{}{}...{}", &raw[..TOKEN_PREFIX.len()], &head[TOKEN_PREFIX.len()..], tail)
    }
}

impl fmt::Debug for MargToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("MargToken").field(&"REDACTED").finish()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum KeyStatus {
    Active,
    Revoked,
}

impl fmt::Display for KeyStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyStatus::Active => write!(f, "active"),
            KeyStatus::Revoked => write!(f, "revoked"),
        }
    }
}

impl FromStr for KeyStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(KeyStatus::Active),
            "revoked" => Ok(KeyStatus::Revoked),
            other => Err(format!("unknown key status '{}'", other)),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MargKey {
    pub id: String,
    pub token_hash: String,
    pub token_prefix: String,
    pub principal: Principal,
    pub status: KeyStatus,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewKey {
    pub id: String,
    pub token_hash: String,
    pub token_prefix: String,
    pub principal_id: String,
    pub principal_kind: PrincipalKind,
    pub created_at: DateTime<Utc>,
}

impl NewKey {
    pub fn build(principal_id: String, kind: PrincipalKind, token: &MargToken) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            token_hash: token.hash(),
            token_prefix: token.display_prefix(),
            principal_id,
            principal_kind: kind,
            created_at: Utc::now(),
        }
    }
}
