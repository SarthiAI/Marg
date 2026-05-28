//! `marg init` subcommand.
//!
//! Resolves a config prefix (`/etc/marg/` for root, `~/.marg/` otherwise),
//! writes a sensible-default `marg.toml` and `policy.toml`, opens the SQLite
//! backend (creating the file and running migrations), mints the bootstrap
//! admin token (idempotent), optionally installs the bundled systemd unit,
//! and prints a single post-install summary box.
//!
//! Used by the one-line installer (`installer/install.sh`) and by the
//! container entrypoint. Also auto-invoked by `marg start` when no config
//! file exists at the requested path.

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use marg_core::{BudgetSpec, Config, MargToken, NewAdminToken, NewKey, PrincipalKind};
use marg_storage::{SqliteStorage, Storage};

pub struct InitOptions {
    pub prefix: Option<PathBuf>,
    /// Override the marg.toml location. Defaults to `<prefix>/marg.toml`.
    /// `marg start --config <path>` uses this so its auto-init writes the
    /// config to exactly where the start command expects to read it.
    pub config_path: Option<PathBuf>,
    pub force: bool,
    pub systemd: bool,
    /// Surfaced as the `--auto` CLI flag and passed through by the
    /// installer and the container entrypoint. Today the init path is
    /// fully non-interactive; this field is the contract that keeps it
    /// that way as features land.
    #[allow(dead_code)]
    pub auto: bool,
    pub seed_key: Option<String>,
    pub quiet: bool,
}

pub struct InitOutcome {
    pub config_path: PathBuf,
    pub policy_path: PathBuf,
    pub data_dir: PathBuf,
    pub admin_token_path: PathBuf,
    pub admin_token_plain: Option<String>,
    pub admin_url: String,
    pub proxy_url: String,
    pub seed_key_plain: Option<String>,
    pub seed_key_id: Option<String>,
    pub systemd_installed: bool,
}

/// Front-end called by both `marg init` and `marg start` (when config is
/// missing). Side effects: writes files to disk, opens SQLite, mints tokens.
pub async fn run(opts: InitOptions) -> Result<()> {
    let outcome = perform(&opts).await?;
    if !opts.quiet {
        print_summary(&outcome);
    }
    Ok(())
}

/// Same as `run`, but returns the outcome instead of printing. Used by
/// `marg start --config <path>` so it can hand the resolved path to the
/// server loader after init lands.
pub async fn perform(opts: &InitOptions) -> Result<InitOutcome> {
    let prefix = resolve_prefix(opts.prefix.as_deref())?;
    std::fs::create_dir_all(&prefix)
        .with_context(|| format!("creating config prefix at {}", prefix.display()))?;
    let prefix = canonicalize_safe(&prefix);

    let config_path = opts
        .config_path
        .clone()
        .unwrap_or_else(|| prefix.join("marg.toml"));
    let policy_path = prefix.join("policy.toml");
    let db_path = prefix.join("marg.db");
    let key_path = prefix.join("marg.key");
    let audit_dir = prefix.join("audit");
    let admin_token_path = prefix.join("marg-admin.token");

    write_config_files(
        &config_path,
        &policy_path,
        &db_path,
        &key_path,
        &audit_dir,
        &admin_token_path,
        opts.force,
    )?;

    let storage = open_storage(&config_path).await?;
    let (admin_plain, _admin_id) = mint_bootstrap_admin_token(storage.as_ref()).await?;

    if let Some(plain) = admin_plain.as_deref() {
        if let Err(e) = write_secret_file(&admin_token_path, plain) {
            tracing::warn!(?e, path = %admin_token_path.display(), "failed to write admin token file");
        }
    }

    let (seed_plain, seed_id) = if let Some(principal) = opts.seed_key.as_deref() {
        let (plain, id) = mint_seed_api_key(storage.as_ref(), principal).await?;
        (Some(plain), Some(id))
    } else {
        (None, None)
    };

    let systemd_installed = if opts.systemd {
        install_systemd_unit(&config_path, &prefix)?
    } else {
        false
    };

    let cfg = Config::load(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;

    Ok(InitOutcome {
        config_path,
        policy_path,
        data_dir: prefix,
        admin_token_path,
        admin_token_plain: admin_plain,
        admin_url: format_url(&cfg.admin.bind),
        proxy_url: format_url(&cfg.server.bind),
        seed_key_plain: seed_plain,
        seed_key_id: seed_id,
        systemd_installed,
    })
}

fn resolve_prefix(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if is_root() {
        return Ok(PathBuf::from("/etc/marg"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("$HOME is not set; pass --config-dir explicitly"))?;
    Ok(home.join(".marg"))
}

fn is_root() -> bool {
    if !cfg!(unix) {
        return false;
    }
    // Safe replacement for libc getuid(): shell out to `id -u`. The
    // workspace forbids unsafe code, so we cannot call the FFI directly.
    // `id` is in POSIX and present on every supported target.
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .as_deref()
        == Some("0")
}

fn canonicalize_safe(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

fn format_url(bind: &str) -> String {
    let display_host = match bind.rsplit_once(':') {
        Some(("0.0.0.0", port)) => format!("127.0.0.1:{}", port),
        Some(("[::]", port)) => format!("127.0.0.1:{}", port),
        _ => bind.to_string(),
    };
    format!("http://{}", display_host)
}

fn write_config_files(
    config_path: &Path,
    policy_path: &Path,
    db_path: &Path,
    key_path: &Path,
    audit_dir: &Path,
    admin_token_path: &Path,
    force: bool,
) -> Result<()> {
    if !config_path.exists() || force {
        let body = render_marg_toml(db_path, key_path, audit_dir, admin_token_path, policy_path);
        std::fs::write(config_path, body)
            .with_context(|| format!("writing {}", config_path.display()))?;
        // marg.toml may carry plaintext credentials (a `plain:` provider key,
        // a database DSN with a password, etc.), so lock it to the owner even
        // when the install starts with no providers configured.
        set_mode(config_path, 0o600);
    }
    if !policy_path.exists() || force {
        std::fs::write(policy_path, DEFAULT_POLICY_TOML)
            .with_context(|| format!("writing {}", policy_path.display()))?;
        // policy.toml is non-secret and security / compliance teams need read
        // access, so it stays world-readable.
        set_mode(policy_path, 0o644);
    }
    std::fs::create_dir_all(audit_dir)
        .with_context(|| format!("creating audit dir at {}", audit_dir.display()))?;
    Ok(())
}

fn render_marg_toml(
    db_path: &Path,
    key_path: &Path,
    audit_dir: &Path,
    admin_token_path: &Path,
    policy_path: &Path,
) -> String {
    let db = path_display(db_path);
    let key = path_display(key_path);
    let audit = path_display_with_trailing_slash(audit_dir);
    let token = path_display(admin_token_path);
    let policy = path_display(policy_path);
    format!(
        r#"# marg.toml written by `marg init`. Safe to edit. See
# https://github.com/SarthiAI/Marg/blob/main/marg.toml.example for
# every option, default, and provider block.

[server]
bind = "0.0.0.0:8080"
max_body_bytes = 1048576

[storage]
backend = "sqlite"
path = "{db}"

[security]
log_prompts = false
log_responses = false

[admin]
enabled = true
bind = "127.0.0.1:8081"
bootstrap_token_path = "{token}"

[rate_limits]
default_rpm = 0
strict_mode = false

# No providers configured. Add one before pointing apps at this gateway.
# Pick one of openai / anthropic / google / bedrock and uncomment the
# matching block. Example for OpenAI:
#
# [providers.openai]
# api_key = "env:OPENAI_API_KEY"
# base_url = "https://api.openai.com"

# ---------------------------------------------------------------------------
# Kavach. First boot defaults to observe so policy can be tuned before the
# flip to enforce. Default policy below permits every request but enforces
# a 32k max_tokens ceiling as an architectural guard rail.
# ---------------------------------------------------------------------------

[kavach]
mode = "observe"
policy_path = "{policy}"
keypair_path = "{key}"
audit_export_path = "{audit}"
audit_flush_seconds = 60
audit_hybrid = true
include_prompts = false
expose_permit_to_caller = false
forward_permit_to_provider = false
permit_ttl_seconds = 30
"#
    )
}

fn path_display(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn path_display_with_trailing_slash(p: &Path) -> String {
    let mut s = path_display(p);
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

const DEFAULT_POLICY_TOML: &str = r#"# Kavach policy file written by `marg init`.
#
# Permit-all default. Replace with real rules before flipping
# [kavach].mode to "enforce" in marg.toml.

[[policy]]
name = "permit_all"
effect = "permit"
priority = 100
conditions = []

# Hard architectural ceiling. Refuses any single request asking for more
# than 32k output tokens, regardless of policy.
[[invariant]]
kind = "param_max"
name = "no_huge_single_request"
field = "max_tokens"
max = 32000
"#;

fn set_mode(_path: &Path, _mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(_path) {
            let mut perms = meta.permissions();
            perms.set_mode(_mode);
            let _ = std::fs::set_permissions(_path, perms);
        }
    }
}

async fn open_storage(config_path: &Path) -> Result<Arc<dyn Storage>> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;
    if cfg.storage.backend != "sqlite" {
        return Err(anyhow!(
            "marg init only supports the sqlite backend; switch [storage].backend later via marg.toml"
        ));
    }
    let storage = SqliteStorage::open(&cfg.storage.path)
        .await
        .with_context(|| format!("opening sqlite at {}", cfg.storage.path))?;
    storage
        .migrate()
        .await
        .context("running sqlite migrations")?;
    Ok(Arc::new(storage) as Arc<dyn Storage>)
}

async fn mint_bootstrap_admin_token(storage: &dyn Storage) -> Result<(Option<String>, Option<String>)> {
    let count = storage
        .count_active_admin_tokens()
        .await
        .context("counting admin tokens")?;
    if count > 0 {
        return Ok((None, None));
    }

    let token = MargToken::generate();
    let plain = token.expose().to_string();
    let new = NewAdminToken {
        id: uuid::Uuid::new_v4().to_string(),
        token_hash: token.hash(),
        token_prefix: token.display_prefix(),
        label: "bootstrap".to_string(),
        created_at: Utc::now(),
    };
    let saved = storage
        .create_admin_token(new)
        .await
        .context("inserting bootstrap admin token")?;
    Ok((Some(plain), Some(saved.id)))
}

async fn mint_seed_api_key(storage: &dyn Storage, principal: &str) -> Result<(String, String)> {
    let token = MargToken::generate();
    let new = NewKey::build(principal.to_string(), PrincipalKind::User, &token);
    let key_id = new.id.clone();
    let saved = storage
        .create_key(new)
        .await
        .with_context(|| format!("creating seed key for {}", principal))?;
    storage
        .upsert_budget(BudgetSpec {
            key_id: key_id.clone(),
            daily_usd: 0.0,
            rpm: 0,
        })
        .await
        .context("setting seed key budget")?;
    Ok((token.expose().to_string(), saved.id))
}

fn write_secret_file(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(contents.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

/// Drop the bundled systemd unit into /etc/systemd/system/ and run
/// daemon-reload + enable. Only attempted on Linux as root with
/// systemctl present. Idempotent: re-running is safe.
fn install_systemd_unit(config_path: &Path, data_dir: &Path) -> Result<bool> {
    if !cfg!(target_os = "linux") {
        tracing::warn!("--systemd is only supported on linux, skipping");
        return Ok(false);
    }
    if !is_root() {
        return Err(anyhow!(
            "--systemd requires running as root (re-run with sudo)"
        ));
    }
    if !which("systemctl") {
        return Err(anyhow!("--systemd was passed but systemctl is not on PATH"));
    }

    let data = path_display(data_dir);
    let unit = include_str!("../../dist/systemd/marg.service")
        .replace("/etc/marg/marg.toml", &config_path.to_string_lossy())
        .replace("/var/lib/marg", &data);
    std::fs::write("/etc/systemd/system/marg.service", unit)
        .context("writing /etc/systemd/system/marg.service")?;

    let run = |args: &[&str]| -> Result<()> {
        let status = std::process::Command::new("systemctl")
            .args(args)
            .status()
            .with_context(|| format!("running systemctl {:?}", args))?;
        if !status.success() {
            return Err(anyhow!("systemctl {:?} exited with {}", args, status));
        }
        Ok(())
    };
    run(&["daemon-reload"])?;
    run(&["enable", "marg.service"])?;
    Ok(true)
}

fn which(bin: &str) -> bool {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.join(bin).is_file() {
                return true;
            }
        }
    }
    false
}

fn print_summary(o: &InitOutcome) {
    let bar = "=".repeat(72);
    println!();
    println!("{}", bar);
    println!("Marg is installed.");
    println!("{}", bar);
    println!();
    println!("  Config file       : {}", o.config_path.display());
    println!("  Policy file       : {}", o.policy_path.display());
    println!("  Data directory    : {}", o.data_dir.display());
    println!("  Admin token file  : {}", o.admin_token_path.display());
    println!();
    println!("  Proxy URL         : {}", o.proxy_url);
    println!("  Admin / Console   : {}", o.admin_url);
    println!();
    if let Some(plain) = o.admin_token_plain.as_deref() {
        println!("  Admin bearer token (shown once, also written to the file above):");
        println!("    {}", plain);
        println!();
    } else {
        println!("  Admin token already minted; see the file above.");
        println!();
    }
    if let Some(plain) = o.seed_key_plain.as_deref() {
        println!("  Seed API key (use with the OpenAI SDK):");
        println!("    id    : {}", o.seed_key_id.as_deref().unwrap_or("-"));
        println!("    token : {}", plain);
        println!();
    }
    if o.systemd_installed {
        println!("  systemd unit installed: /etc/systemd/system/marg.service");
        println!("  Start with            : sudo systemctl start marg");
        println!("  Tail logs with        : journalctl -u marg -f");
    } else {
        println!("  Start the gateway with:");
        println!(
            "    marg start --config {}",
            o.config_path.display()
        );
    }
    println!();
    println!("  Next steps:");
    println!("    1. Edit {} and add a provider", o.config_path.display());
    println!("       block under [providers.openai] / .anthropic / .google / .bedrock.");
    println!("    2. Open the console at {} to create application API keys.", o.admin_url);
    println!("    3. Point your OpenAI SDK base_url at {}.", o.proxy_url);
    println!();
    println!("{}", bar);
    println!();
}
