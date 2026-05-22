//! Admin HTTP API surface (P05).
//!
//! Mounted on its own port (default `127.0.0.1:8081`). All endpoints require
//! `Authorization: Bearer <admin-token>`. The bootstrap token is created on
//! first boot and written to `admin.bootstrap_token_path`; from there
//! operators rotate via `POST /admin/auth/tokens` and revoke via
//! `DELETE /admin/auth/tokens/{id}`.

pub mod auth;
pub mod console;
pub mod error;
pub mod handlers;
pub mod openapi;
pub mod router;
pub mod server;

pub use router::build_router;
pub use server::serve_admin;
