use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PrincipalKind {
    User,
    Service,
    Agent,
}

impl fmt::Display for PrincipalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrincipalKind::User => write!(f, "user"),
            PrincipalKind::Service => write!(f, "service"),
            PrincipalKind::Agent => write!(f, "agent"),
        }
    }
}

impl FromStr for PrincipalKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(PrincipalKind::User),
            "service" => Ok(PrincipalKind::Service),
            "agent" => Ok(PrincipalKind::Agent),
            other => Err(format!("unknown principal kind '{}', expected user|service|agent", other)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Principal {
    pub id: String,
    pub kind: PrincipalKind,
}
