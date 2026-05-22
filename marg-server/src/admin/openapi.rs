//! Hand-rolled OpenAPI 3.1 spec served at `GET /admin/openapi.json`.
//!
//! We keep this hand-written instead of pulling a derive macro. It is one
//! flat document, easy to review, easy to diff. Whenever an admin endpoint
//! changes shape, update this file in the same diff.

use serde_json::{json, Value};

pub fn spec() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Marg admin API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Admin HTTP API for managing Marg keys, budgets, routes, policy, providers, and the request log. Every endpoint requires `Authorization: Bearer <admin-token>` (see `/admin/auth/tokens`)."
        },
        "components": {
            "securitySchemes": {
                "BearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "marg_live_<base32>"
                }
            },
            "schemas": {
                "Error": {
                    "type": "object",
                    "properties": {
                        "error": {
                            "type": "object",
                            "properties": {
                                "code": { "type": "string" },
                                "message": { "type": "string" }
                            },
                            "required": ["code", "message"]
                        }
                    }
                },
                "BudgetSpec": {
                    "type": "object",
                    "properties": {
                        "key_id": { "type": "string" },
                        "daily_usd": { "type": "number", "format": "double" },
                        "rpm": { "type": "integer", "minimum": 0 }
                    },
                    "required": ["key_id", "daily_usd", "rpm"]
                },
                "MargKey": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "token_hash": { "type": "string" },
                        "token_prefix": { "type": "string" },
                        "principal": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "kind": { "type": "string", "enum": ["user", "service", "agent"] }
                            },
                            "required": ["id", "kind"]
                        },
                        "team": { "type": ["string", "null"] },
                        "status": { "type": "string", "enum": ["active", "revoked"] },
                        "created_at": { "type": "string", "format": "date-time" },
                        "revoked_at": { "type": ["string", "null"], "format": "date-time" }
                    },
                    "required": ["id", "principal", "status", "created_at"]
                },
                "RouteSpec": {
                    "type": "object",
                    "properties": {
                        "position": { "type": "integer" },
                        "match_model": { "type": ["string", "null"] },
                        "match_team": { "type": ["string", "null"] },
                        "primary": { "type": ["string", "null"] },
                        "primary_model": { "type": ["string", "null"] },
                        "fallbacks": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "split": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "provider": { "type": "string" },
                                    "weight": { "type": "integer", "minimum": 1 },
                                    "model": { "type": ["string", "null"] }
                                },
                                "required": ["provider", "weight"]
                            }
                        }
                    }
                }
            }
        },
        "security": [{ "BearerAuth": [] }],
        "paths": {
            "/admin/keys": {
                "post": {
                    "summary": "Create a Marg API key",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "properties": {
                                        "principal_id": { "type": "string" },
                                        "kind": { "type": "string", "enum": ["user", "service", "agent"], "default": "user" },
                                        "team": { "type": ["string", "null"] },
                                        "daily_budget_usd": { "type": "number", "default": 0 },
                                        "rpm": { "type": "integer", "default": 0 }
                                    },
                                    "required": ["principal_id"]
                                }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Created key with plaintext token (shown once).",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "key": { "$ref": "#/components/schemas/MargKey" },
                                            "token": { "type": "string" },
                                            "budget": { "$ref": "#/components/schemas/BudgetSpec" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                "get": {
                    "summary": "List Marg API keys",
                    "parameters": [
                        { "name": "principal", "in": "query", "schema": { "type": "string" } },
                        { "name": "kind", "in": "query", "schema": { "type": "string" } },
                        { "name": "status", "in": "query", "schema": { "type": "string" } }
                    ],
                    "responses": {
                        "200": {
                            "description": "Keys array.",
                            "content": { "application/json": { "schema": { "type": "object" } } }
                        }
                    }
                }
            },
            "/admin/keys/{id}": {
                "get": {
                    "summary": "Get one key with its budget",
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" } }],
                    "responses": {
                        "200": { "description": "Key + budget." },
                        "404": { "description": "Not found", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } }
                    }
                },
                "delete": {
                    "summary": "Revoke a key (final, irreversible)",
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" } }],
                    "responses": {
                        "200": { "description": "Revoked." },
                        "404": { "description": "Not found" }
                    }
                }
            },
            "/admin/keys/{id}/invalidate": {
                "post": {
                    "summary": "Invalidate the hot-store entry for a key (forces cold-path re-auth)",
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" } }],
                    "responses": { "200": { "description": "Invalidated." } }
                }
            },
            "/admin/budgets": {
                "post": {
                    "summary": "Upsert a budget for a key",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/BudgetSpec" }
                            }
                        }
                    },
                    "responses": { "200": { "description": "Budget upserted." } }
                }
            },
            "/admin/budgets/{key_id}": {
                "get": {
                    "summary": "Get a key's budget + today's spend + remaining",
                    "parameters": [{ "name": "key_id", "in": "path", "required": true, "schema": { "type": "string" } }],
                    "responses": { "200": { "description": "Budget snapshot." } }
                }
            },
            "/admin/routes": {
                "get": {
                    "summary": "List persisted routes (config routes are in /admin/policy)",
                    "responses": { "200": { "description": "Routes." } }
                },
                "post": {
                    "summary": "Create a runtime route. Triggers a policy reload before returning.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/RouteSpec" }
                            }
                        }
                    },
                    "responses": { "200": { "description": "Route stored, policy reloaded." } }
                }
            },
            "/admin/policy": {
                "get": {
                    "summary": "View the effective routing + pricing policy",
                    "responses": { "200": { "description": "Effective policy." } }
                }
            },
            "/admin/policy/reload": {
                "post": {
                    "summary": "Re-read config from disk + DB routes and atomically swap them in",
                    "responses": { "200": { "description": "Reload outcome." } }
                }
            },
            "/admin/providers/health": {
                "get": {
                    "summary": "Per-provider derived health (configured + success/error counters)",
                    "responses": { "200": { "description": "Provider health." } }
                }
            },
            "/admin/requests": {
                "get": {
                    "summary": "Query the Marg-native request log (pre-Kavach; P08 adds audit chain queries)",
                    "parameters": [
                        { "name": "since", "in": "query", "schema": { "type": "string", "format": "date-time" } },
                        { "name": "key_id", "in": "query", "schema": { "type": "string" } },
                        { "name": "model", "in": "query", "schema": { "type": "string" } },
                        { "name": "provider", "in": "query", "schema": { "type": "string" } },
                        { "name": "limit", "in": "query", "schema": { "type": "integer", "minimum": 1, "maximum": 10000, "default": 100 } }
                    ],
                    "responses": { "200": { "description": "Matching request log entries." } }
                }
            },
            "/admin/auth/tokens": {
                "post": {
                    "summary": "Create a new admin token (shown once in plaintext). Old tokens stay valid until revoked.",
                    "requestBody": {
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "properties": { "label": { "type": "string" } }
                                }
                            }
                        }
                    },
                    "responses": { "200": { "description": "Created admin token." } }
                },
                "get": {
                    "summary": "List admin tokens (hash + prefix + status only; never the plain token)",
                    "responses": { "200": { "description": "Tokens." } }
                }
            },
            "/admin/auth/tokens/{id}": {
                "delete": {
                    "summary": "Revoke an admin token. Use this for rotation: create new, then revoke old.",
                    "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" } }],
                    "responses": { "200": { "description": "Revoked." } }
                }
            }
        }
    })
}
