//! Types backing the admin HTTP API surface (P05). Lives in marg-core so the
//! storage trait and the server handlers share one definition.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::routing::{RouteSpec, SplitEntry};

/// Admin token row as it lives in storage. The plain-text token itself is
/// never persisted; only the SHA-256 hash and a redacted prefix used for
/// listing in the admin UI.
#[derive(Debug, Clone, Serialize)]
pub struct AdminToken {
    pub id: String,
    pub token_hash: String,
    pub token_prefix: String,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Insert payload for a fresh admin token.
#[derive(Debug, Clone)]
pub struct NewAdminToken {
    pub id: String,
    pub token_hash: String,
    pub token_prefix: String,
    pub label: String,
    pub created_at: DateTime<Utc>,
}

/// One persisted routing rule. The shape mirrors [`RouteSpec`] from
/// the static config file, but it carries an id, an explicit ordering
/// position, and serialised JSON for fallback / split lists so it can be
/// stored in one row.
#[derive(Debug, Clone, Serialize)]
pub struct PersistedRoute {
    pub id: String,
    pub position: i32,
    pub match_model: Option<String>,
    pub match_team: Option<String>,
    pub primary: Option<String>,
    pub primary_model: Option<String>,
    pub fallbacks: Vec<String>,
    pub split: Vec<SplitEntry>,
    pub created_at: DateTime<Utc>,
}

impl PersistedRoute {
    /// Translate the persisted row back into the `RouteSpec` shape the
    /// `RoutingEngine` compiles from.
    pub fn to_route_spec(&self) -> RouteSpec {
        let primary = match (&self.primary, &self.primary_model) {
            (Some(p), Some(m)) => Some(format!("{}:{}", p, m)),
            (Some(p), None) => Some(p.clone()),
            (None, _) => None,
        };
        RouteSpec {
            r#match: crate::routing::MatchSpec {
                model: self.match_model.clone(),
                team: self.match_team.clone(),
            },
            primary,
            fallback: self.fallbacks.clone(),
            split: self.split.clone(),
        }
    }
}

/// Payload accepted by `POST /admin/routes`. `position` may be omitted, in
/// which case the route is appended at the end. The handler turns this into
/// a [`PersistedRoute`] before passing it to storage.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewRouteRequest {
    #[serde(default)]
    pub position: Option<i32>,
    #[serde(default)]
    pub match_model: Option<String>,
    #[serde(default)]
    pub match_team: Option<String>,
    #[serde(default)]
    pub primary: Option<String>,
    #[serde(default)]
    pub primary_model: Option<String>,
    #[serde(default)]
    pub fallbacks: Vec<String>,
    #[serde(default)]
    pub split: Vec<SplitEntry>,
}
