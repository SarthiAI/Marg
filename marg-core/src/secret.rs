use std::path::Path;

use crate::error::ConfigError;

/// Resolves a secret reference into a concrete value. References are short
/// strings written in config files and have one of these shapes:
///
///   * `plain:<value>`      literal value (default if no scheme prefix)
///   * `env:<NAME>`         read from the `NAME` environment variable
///   * `file:<absolute or relative path>` read the trimmed file contents
///
/// A bare value with no scheme prefix is treated as `plain:`. This keeps the
/// existing config files (which pass api keys inline) working unchanged while
/// letting operators move secrets out of the file when they need to.
///
/// All schemes return a `String`; callers wrap with `secrecy::SecretString` for
/// in-memory protection.
pub fn resolve(reference: &str) -> Result<String, ConfigError> {
    let trimmed = reference.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::Validation(
            "empty secret reference (expected plain:..., env:..., or file:...)".into(),
        ));
    }
    if let Some(rest) = trimmed.strip_prefix("env:") {
        let name = rest.trim();
        if name.is_empty() {
            return Err(ConfigError::Validation(
                "env: secret reference missing variable name".into(),
            ));
        }
        return std::env::var(name).map_err(|e| {
            ConfigError::Validation(format!(
                "env variable '{}' is not set ({})",
                name, e
            ))
        });
    }
    if let Some(rest) = trimmed.strip_prefix("file:") {
        let path = rest.trim();
        if path.is_empty() {
            return Err(ConfigError::Validation(
                "file: secret reference missing path".into(),
            ));
        }
        let value = std::fs::read_to_string(Path::new(path)).map_err(|e| {
            ConfigError::Validation(format!("failed to read secret file '{}': {}", path, e))
        })?;
        return Ok(value.trim_end_matches(['\r', '\n']).to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("plain:") {
        return Ok(rest.to_string());
    }
    Ok(trimmed.to_string())
}

/// Same as `resolve` but only returns Some(value) when the reference is set
/// (i.e. non-empty plain or successfully resolved env/file). Useful for
/// optional secrets such as session tokens.
pub fn resolve_optional(reference: Option<&str>) -> Result<Option<String>, ConfigError> {
    match reference {
        None => Ok(None),
        Some(r) if r.trim().is_empty() => Ok(None),
        Some(r) => Ok(Some(resolve(r)?)),
    }
}
