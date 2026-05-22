use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RouteSpec {
    #[serde(default)]
    pub r#match: MatchSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub split: Vec<SplitEntry>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MatchSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SplitEntry {
    pub provider: String,
    pub weight: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProviderTarget {
    pub provider: String,
    pub model_override: Option<String>,
}

impl ProviderTarget {
    pub fn parse(spec: &str) -> Result<Self, ConfigError> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(ConfigError::Validation(
                "provider target string cannot be empty".into(),
            ));
        }
        match spec.split_once(':') {
            Some((provider, model)) => {
                let provider = provider.trim();
                let model = model.trim();
                if provider.is_empty() {
                    return Err(ConfigError::Validation(format!(
                        "provider target '{}' is missing a provider name before ':'",
                        spec
                    )));
                }
                if model.is_empty() {
                    return Err(ConfigError::Validation(format!(
                        "provider target '{}' has empty model after ':'",
                        spec
                    )));
                }
                Ok(Self {
                    provider: provider.to_string(),
                    model_override: Some(model.to_string()),
                })
            }
            None => Ok(Self {
                provider: spec.to_string(),
                model_override: None,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompiledSplit {
    pub provider: String,
    pub model_override: Option<String>,
    pub weight: u32,
}

#[derive(Debug, Clone)]
pub struct CompiledRoute {
    pub model_pattern: Option<String>,
    pub team: Option<String>,
    pub primary: Option<ProviderTarget>,
    pub fallback: Vec<ProviderTarget>,
    pub split: Vec<CompiledSplit>,
    pub total_weight: u32,
}

impl CompiledRoute {
    fn matches(&self, model: &str, team: Option<&str>) -> bool {
        if let Some(pattern) = &self.model_pattern {
            if !glob_match(pattern, model) {
                return false;
            }
        }
        if let Some(required_team) = &self.team {
            match team {
                Some(t) if t == required_team => {}
                _ => return false,
            }
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct RouteResolution {
    pub primary: ResolvedTarget,
    pub fallbacks: Vec<ResolvedTarget>,
}

#[derive(Debug, Clone, Default)]
pub struct RoutingEngine {
    routes: Vec<CompiledRoute>,
    default_provider: Option<String>,
}

impl RoutingEngine {
    pub fn build(
        specs: &[RouteSpec],
        default_provider: Option<String>,
        registered_providers: &[String],
    ) -> Result<Self, ConfigError> {
        let mut compiled = Vec::with_capacity(specs.len());
        for (idx, spec) in specs.iter().enumerate() {
            let route = compile_route(idx, spec, registered_providers)?;
            compiled.push(route);
        }
        if let Some(default) = &default_provider {
            if !registered_providers.iter().any(|p| p == default) {
                return Err(ConfigError::Validation(format!(
                    "providers.default '{}' is not a configured provider (configured: {:?})",
                    default, registered_providers
                )));
            }
        }
        Ok(Self {
            routes: compiled,
            default_provider,
        })
    }

    pub fn resolve(
        &self,
        request_model: &str,
        team: Option<&str>,
        pick_seed: u64,
    ) -> Result<RouteResolution, RoutingError> {
        for route in &self.routes {
            if !route.matches(request_model, team) {
                continue;
            }
            if !route.split.is_empty() {
                let chosen = pick_weighted(&route.split, route.total_weight, pick_seed);
                return Ok(RouteResolution {
                    primary: ResolvedTarget {
                        provider: chosen.provider.clone(),
                        model: chosen
                            .model_override
                            .clone()
                            .unwrap_or_else(|| request_model.to_string()),
                    },
                    fallbacks: route
                        .fallback
                        .iter()
                        .map(|t| ResolvedTarget {
                            provider: t.provider.clone(),
                            model: t
                                .model_override
                                .clone()
                                .unwrap_or_else(|| request_model.to_string()),
                        })
                        .collect(),
                });
            }
            let primary = route.primary.as_ref().ok_or_else(|| {
                RoutingError::MisconfiguredRoute("route has neither primary nor split".into())
            })?;
            return Ok(RouteResolution {
                primary: ResolvedTarget {
                    provider: primary.provider.clone(),
                    model: primary
                        .model_override
                        .clone()
                        .unwrap_or_else(|| request_model.to_string()),
                },
                fallbacks: route
                    .fallback
                    .iter()
                    .map(|t| ResolvedTarget {
                        provider: t.provider.clone(),
                        model: t
                            .model_override
                            .clone()
                            .unwrap_or_else(|| request_model.to_string()),
                    })
                    .collect(),
            });
        }
        match &self.default_provider {
            Some(provider) => Ok(RouteResolution {
                primary: ResolvedTarget {
                    provider: provider.clone(),
                    model: request_model.to_string(),
                },
                fallbacks: Vec::new(),
            }),
            None => Err(RoutingError::NoRouteMatched {
                model: request_model.to_string(),
            }),
        }
    }
}

fn compile_route(
    idx: usize,
    spec: &RouteSpec,
    registered: &[String],
) -> Result<CompiledRoute, ConfigError> {
    let model_pattern = spec
        .r#match
        .model
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let team = spec
        .r#match
        .team
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let has_primary = spec
        .primary
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let has_split = !spec.split.is_empty();
    if has_primary && has_split {
        return Err(ConfigError::Validation(format!(
            "route #{} declares both primary and split. Choose one.",
            idx
        )));
    }
    if !has_primary && !has_split {
        return Err(ConfigError::Validation(format!(
            "route #{} has neither primary nor split. At least one is required.",
            idx
        )));
    }

    let primary = if has_primary {
        Some(ProviderTarget::parse(spec.primary.as_ref().unwrap())?)
    } else {
        None
    };
    if let Some(p) = &primary {
        ensure_registered(&p.provider, registered, idx, "primary")?;
    }

    let mut fallback = Vec::with_capacity(spec.fallback.len());
    for raw in &spec.fallback {
        let target = ProviderTarget::parse(raw)?;
        ensure_registered(&target.provider, registered, idx, "fallback")?;
        fallback.push(target);
    }

    let mut split = Vec::with_capacity(spec.split.len());
    let mut total_weight: u32 = 0;
    for entry in &spec.split {
        if entry.weight == 0 {
            return Err(ConfigError::Validation(format!(
                "route #{} split entry for provider '{}' has weight 0; must be >= 1",
                idx, entry.provider
            )));
        }
        ensure_registered(&entry.provider, registered, idx, "split")?;
        total_weight = total_weight.saturating_add(entry.weight);
        split.push(CompiledSplit {
            provider: entry.provider.clone(),
            model_override: entry.model.clone(),
            weight: entry.weight,
        });
    }

    Ok(CompiledRoute {
        model_pattern,
        team,
        primary,
        fallback,
        split,
        total_weight,
    })
}

fn ensure_registered(
    provider: &str,
    registered: &[String],
    route_idx: usize,
    role: &str,
) -> Result<(), ConfigError> {
    if registered.iter().any(|p| p == provider) {
        Ok(())
    } else {
        Err(ConfigError::Validation(format!(
            "route #{} {} references provider '{}' which is not configured (configured providers: {:?})",
            route_idx, role, provider, registered
        )))
    }
}

fn pick_weighted(entries: &[CompiledSplit], total: u32, seed: u64) -> &CompiledSplit {
    if total == 0 {
        return &entries[0];
    }
    let mut pick = (seed % total as u64) as u32;
    for entry in entries {
        if pick < entry.weight {
            return entry;
        }
        pick -= entry.weight;
    }
    entries.last().expect("split entries non-empty")
}

#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error("no route matched for model '{model}' and no default provider configured")]
    NoRouteMatched { model: String },

    #[error("misconfigured route: {0}")]
    MisconfiguredRoute(String),
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RouteAttemptOutcome;

pub fn glob_match(pattern: &str, input: &str) -> bool {
    let pat = pattern.as_bytes();
    let s = input.as_bytes();
    let (mut i, mut j) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut match_j: usize = 0;
    while j < s.len() {
        if i < pat.len() && pat[i] == b'*' {
            star = Some(i);
            match_j = j;
            i += 1;
        } else if i < pat.len() && (pat[i] == b'?' || pat[i] == s[j]) {
            i += 1;
            j += 1;
        } else if let Some(p_star) = star {
            i = p_star + 1;
            match_j += 1;
            j = match_j;
        } else {
            return false;
        }
    }
    while i < pat.len() && pat[i] == b'*' {
        i += 1;
    }
    i == pat.len()
}
