use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use marg_core::{secret, BudgetSpec, Config, MargToken, NewAdminToken, NewKey, PrincipalKind};
use marg_storage::{PostgresStorage, SqliteStorage, Storage};

mod init;

/// Which subcommand we are running. The server runs in JSON-log mode by
/// default so logs flow straight into any modern log pipeline; CLI admin
/// commands keep human-friendly compact output so the operator can read them.
#[derive(Clone, Copy)]
enum LogMode {
    Server,
    Cli,
}

#[derive(Parser)]
#[command(name = "marg", version, about = "Marg: self-hosted AI gateway")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Start {
        #[arg(long, default_value = "./marg.toml")]
        config: String,
    },
    Version {
        #[arg(long)]
        verbose: bool,
    },
    Db {
        #[command(subcommand)]
        action: DbCommand,
    },
    Keys {
        #[command(subcommand)]
        action: KeysCommand,
    },
    Budget {
        #[command(subcommand)]
        action: BudgetCommand,
    },
    Log {
        #[command(subcommand)]
        action: LogCommand,
    },
    Admin {
        #[command(subcommand)]
        action: AdminCommand,
    },
    /// Kavach policy and audit-chain tooling.
    Policy {
        #[command(subcommand)]
        action: PolicyCommand,
    },
    /// One-shot install bootstrap. Writes a default marg.toml and policy.toml
    /// into a chosen prefix, runs the SQLite migrations, mints the first
    /// admin token, and prints a single post-install summary. Idempotent
    /// (existing files are kept unless --force is set). Used by the one-line
    /// installer and the container entrypoint; safe to run by hand.
    Init {
        /// Config prefix. Defaults to /etc/marg when run as root and
        /// $HOME/.marg otherwise. Marg writes the config file, policy file,
        /// SQLite DB, audit dir, signing keypair, and admin token into this
        /// prefix.
        #[arg(long)]
        config_dir: Option<PathBuf>,
        /// Overwrite an existing marg.toml / policy.toml in the prefix.
        #[arg(long, default_value_t = false)]
        force: bool,
        /// Drop the bundled systemd unit into /etc/systemd/system/ and
        /// enable it. Requires root and systemctl on PATH; linux only.
        #[arg(long, default_value_t = false)]
        systemd: bool,
        /// No prompts. Picks defaults for every choice. Used by the
        /// installer script and the container entrypoint.
        #[arg(long, default_value_t = false)]
        auto: bool,
        /// Optional: also mint a Marg API key for this principal id, so
        /// apps can be wired up without opening the console.
        #[arg(long)]
        seed_key: Option<String>,
    },
}

#[derive(Subcommand)]
enum PolicyCommand {
    /// Print would-refuse events from the Kavach signed audit chain (the
    /// per-process JSONL files under [kavach].audit_export_path). Operators
    /// use this in observe mode to tune their policy before flipping to
    /// enforce.
    Audit {
        #[arg(long, default_value = "./marg.toml")]
        config: String,
        /// Only show events newer than this duration ago, e.g. `5m`, `2h`, `24h`.
        #[arg(long)]
        since: Option<String>,
        /// Maximum rows to print.
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Optional explicit path: a JSONL file or a directory of audit
        /// JSONL files. When omitted, [kavach].audit_export_path is used.
        #[arg(long)]
        path: Option<String>,
    },
}

#[derive(Subcommand)]
enum DbCommand {
    Migrate {
        #[arg(long, default_value = "./marg.toml")]
        config: String,
    },
}

#[derive(Subcommand)]
enum KeysCommand {
    Create {
        #[arg(long)]
        principal_id: String,
        #[arg(long, default_value = "user")]
        kind: String,
        #[arg(long, default_value_t = 0.0)]
        daily_budget_usd: f64,
        #[arg(long, default_value_t = 0)]
        rpm: u32,
        #[arg(long)]
        team: Option<String>,
        #[arg(long, default_value = "./marg.toml")]
        config: String,
    },
    List {
        #[arg(long, default_value = "./marg.toml")]
        config: String,
    },
    Revoke {
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "./marg.toml")]
        config: String,
    },
}

#[derive(Subcommand)]
enum BudgetCommand {
    Show {
        #[arg(long)]
        key_id: String,
        #[arg(long, default_value = "./marg.toml")]
        config: String,
    },
    Set {
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        daily_budget_usd: f64,
        #[arg(long, default_value_t = 0)]
        rpm: u32,
        #[arg(long, default_value = "./marg.toml")]
        config: String,
    },
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Mint the first admin bearer token. Writes it to the configured
    /// `[admin].bootstrap_token_path` (default `./marg-admin.token`).
    /// Idempotent: no-op when active admin tokens already exist.
    Bootstrap {
        #[arg(long, default_value = "./marg.toml")]
        config: String,
        #[arg(long, default_value_t = false)]
        force: bool,
        #[arg(long)]
        label: Option<String>,
    },
    /// List admin tokens.
    Tokens {
        #[command(subcommand)]
        action: AdminTokensCommand,
    },
}

#[derive(Subcommand)]
enum AdminTokensCommand {
    List {
        #[arg(long, default_value = "./marg.toml")]
        config: String,
    },
    Revoke {
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "./marg.toml")]
        config: String,
    },
}

#[derive(Subcommand)]
enum LogCommand {
    Tail {
        #[arg(long)]
        key_id: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long, default_value = "./marg.toml")]
        config: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mode = match &cli.command {
        Command::Start { .. } => LogMode::Server,
        _ => LogMode::Cli,
    };
    install_tracing(mode);
    match cli.command {
        Command::Start { config } => {
            ensure_config_present(&config).await?;
            marg_server::run(&config).await
        }
        Command::Version { verbose } => {
            print_version(verbose);
            Ok(())
        }
        Command::Db { action } => match action {
            DbCommand::Migrate { config } => db_migrate(&config).await,
        },
        Command::Keys { action } => match action {
            KeysCommand::Create {
                principal_id,
                kind,
                daily_budget_usd,
                rpm,
                team,
                config,
            } => keys_create(&config, &principal_id, &kind, daily_budget_usd, rpm, team).await,
            KeysCommand::List { config } => keys_list(&config).await,
            KeysCommand::Revoke { id, config } => keys_revoke(&config, &id).await,
        },
        Command::Budget { action } => match action {
            BudgetCommand::Show { key_id, config } => budget_show(&config, &key_id).await,
            BudgetCommand::Set {
                key_id,
                daily_budget_usd,
                rpm,
                config,
            } => budget_set(&config, &key_id, daily_budget_usd, rpm).await,
        },
        Command::Log { action } => match action {
            LogCommand::Tail { key_id, limit, config } => {
                log_tail(&config, key_id.as_deref(), limit).await
            }
        },
        Command::Admin { action } => match action {
            AdminCommand::Bootstrap { config, force, label } => {
                admin_bootstrap(&config, force, label.as_deref()).await
            }
            AdminCommand::Tokens { action } => match action {
                AdminTokensCommand::List { config } => admin_tokens_list(&config).await,
                AdminTokensCommand::Revoke { id, config } => admin_tokens_revoke(&config, &id).await,
            },
        },
        Command::Policy { action } => match action {
            PolicyCommand::Audit {
                config,
                since,
                limit,
                path,
            } => policy_audit(&config, since.as_deref(), limit, path.as_deref()).await,
        },
        Command::Init {
            config_dir,
            force,
            systemd,
            auto,
            seed_key,
        } => {
            let opts = init::InitOptions {
                prefix: config_dir,
                config_path: None,
                force,
                systemd,
                auto,
                seed_key,
                quiet: false,
            };
            init::run(opts).await
        }
    }
}

/// `marg start --config <path>` is the most common entry point and we want it
/// to "just work" on a fresh box. When the config file is missing we run the
/// same init flow the installer uses, scoped to the parent directory of the
/// requested path, then continue with the regular boot. Idempotent in
/// practice: a config-present second run skips the file writes and just
/// validates that admin-token bootstrap is in place.
async fn ensure_config_present(config_path: &str) -> Result<()> {
    let path = std::path::Path::new(config_path);
    if path.exists() {
        return Ok(());
    }
    let prefix = path
        .parent()
        .map(|p| if p.as_os_str().is_empty() { PathBuf::from(".") } else { p.to_path_buf() });
    tracing::info!(
        path = %config_path,
        "no config found at requested path; running marg init to bootstrap defaults"
    );
    let opts = init::InitOptions {
        prefix,
        config_path: Some(path.to_path_buf()),
        force: false,
        systemd: false,
        auto: true,
        seed_key: None,
        quiet: false,
    };
    init::run(opts).await
}

fn install_tracing(mode: LogMode) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let env_format = std::env::var("MARG_LOG_FORMAT").ok();
    let use_json = match env_format.as_deref() {
        Some("json") => true,
        Some("text") | Some("compact") => false,
        _ => matches!(mode, LogMode::Server),
    };

    if use_json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_target(true)
            .with_current_span(true)
            .with_span_list(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .compact()
            .init();
    }
}

fn print_version(verbose: bool) {
    let info = marg_server::version_info();
    if verbose {
        let pretty = serde_json::to_string_pretty(&info).expect("serialize version info");
        println!("{}", pretty);
    } else if let Some(v) = info.get("marg").and_then(|v| v.as_str()) {
        println!("marg {}", v);
    } else {
        println!("marg unknown");
    }
}

async fn open_storage(config_path: &str) -> Result<Arc<dyn Storage>> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("loading config from {}", config_path))?;
    match cfg.storage.backend.as_str() {
        "sqlite" => {
            let storage = SqliteStorage::open(&cfg.storage.path)
                .await
                .with_context(|| format!("opening sqlite at {}", cfg.storage.path))?;
            storage.migrate().await.context("running sqlite migrations")?;
            Ok(Arc::new(storage) as Arc<dyn Storage>)
        }
        "postgres" => {
            let dsn_ref = cfg
                .storage
                .dsn
                .as_deref()
                .context("storage.dsn must be set for postgres backend")?;
            let dsn = secret::resolve(dsn_ref).context("resolving storage.dsn")?;
            let storage = PostgresStorage::connect(
                &dsn,
                cfg.storage.max_connections,
                cfg.storage.min_connections,
            )
            .await
            .with_context(|| "connecting to postgres")?;
            storage.migrate().await.context("running postgres migrations")?;
            Ok(Arc::new(storage) as Arc<dyn Storage>)
        }
        other => anyhow::bail!(
            "storage.backend '{}' is not supported: choose 'sqlite' or 'postgres'",
            other
        ),
    }
}

async fn db_migrate(config_path: &str) -> Result<()> {
    let storage = open_storage(config_path).await?;
    println!("migrations applied to {}", storage.backend_name());
    Ok(())
}

async fn keys_create(
    config_path: &str,
    principal_id: &str,
    kind_str: &str,
    daily_budget_usd: f64,
    rpm: u32,
    team: Option<String>,
) -> Result<()> {
    let storage = open_storage(config_path).await?;
    let kind = PrincipalKind::from_str(kind_str).map_err(|e| anyhow!(e))?;
    let token = MargToken::generate();
    let new = NewKey::build(principal_id.to_string(), kind, &token).with_team(team);
    let key_id = new.id.clone();
    let saved = storage.create_key(new).await.context("inserting key")?;
    storage
        .upsert_budget(BudgetSpec {
            key_id: key_id.clone(),
            daily_usd: daily_budget_usd,
            rpm,
        })
        .await
        .context("setting initial budget")?;

    println!("KEY ID:        {}", saved.id);
    println!("PRINCIPAL:     {} ({})", saved.principal.id, saved.principal.kind);
    println!("TEAM:          {}", saved.team.as_deref().unwrap_or("(none)"));
    println!("CREATED:       {}", saved.created_at.to_rfc3339());
    println!("DAILY BUDGET:  {} USD", daily_budget_usd);
    println!("RPM:           {}", rpm);
    println!();
    println!("MARG API TOKEN (shown once, store it now):");
    println!("  {}", token.expose());
    println!();
    println!("Configure your client:");
    println!("  openai.OpenAI(base_url='http://localhost:8080/v1', api_key='{}')", token.expose());
    Ok(())
}

async fn keys_list(config_path: &str) -> Result<()> {
    let storage = open_storage(config_path).await?;
    let keys = storage.list_keys().await.context("listing keys")?;
    if keys.is_empty() {
        println!("no keys");
        return Ok(());
    }
    println!(
        "{:<36}  {:<24}  {:<8}  {:<8}  {:<14}  {:<20}  {}",
        "id", "principal", "kind", "status", "team", "created_at", "token_prefix"
    );
    for k in keys {
        println!(
            "{:<36}  {:<24}  {:<8}  {:<8}  {:<14}  {:<20}  {}",
            k.id,
            k.principal.id,
            k.principal.kind,
            k.status,
            k.team.as_deref().unwrap_or("-"),
            k.created_at.to_rfc3339(),
            k.token_prefix,
        );
    }
    Ok(())
}

async fn keys_revoke(config_path: &str, id: &str) -> Result<()> {
    let storage = open_storage(config_path).await?;
    storage.revoke_key(id).await.with_context(|| format!("revoking key {}", id))?;
    println!("revoked {}", id);
    Ok(())
}

async fn budget_show(config_path: &str, key_id: &str) -> Result<()> {
    let storage = open_storage(config_path).await?;
    let budget = storage
        .get_budget(key_id)
        .await
        .context("looking up budget")?
        .ok_or_else(|| anyhow!("no budget configured for key {}", key_id))?;
    let day = Utc::now().date_naive();
    let spent = storage
        .current_spend(key_id, day)
        .await
        .context("looking up current spend")?;

    println!("KEY ID:        {}", budget.key_id);
    println!("DAILY BUDGET:  {} USD", budget.daily_usd);
    println!("RPM:           {}", budget.rpm);
    println!("DAY (UTC):     {}", day);
    println!("SPENT TODAY:   {} USD", spent);
    if budget.daily_usd > 0.0 {
        let remaining = (budget.daily_usd - spent).max(0.0);
        println!("REMAINING:     {} USD", remaining);
    } else {
        println!("REMAINING:     unlimited");
    }
    Ok(())
}

async fn budget_set(config_path: &str, key_id: &str, daily_usd: f64, rpm: u32) -> Result<()> {
    let storage = open_storage(config_path).await?;
    storage
        .upsert_budget(BudgetSpec {
            key_id: key_id.to_string(),
            daily_usd,
            rpm,
        })
        .await
        .context("upserting budget")?;
    println!("budget updated for {} (daily {} USD, rpm {})", key_id, daily_usd, rpm);
    Ok(())
}

async fn log_tail(config_path: &str, key_id: Option<&str>, limit: u32) -> Result<()> {
    let storage = open_storage(config_path).await?;
    let entries = storage
        .recent_request_logs(key_id, limit)
        .await
        .context("reading request log")?;
    if entries.is_empty() {
        println!("no request log entries");
        return Ok(());
    }
    println!(
        "{:<24}  {:<36}  {:<10}  {:<22}  {:>6}  {:>10}  {:>10}  {:>4}  {:>4}",
        "timestamp", "key_id", "provider", "model", "in", "out", "cost_usd", "ms", "fovr"
    );
    for e in entries {
        let failovers = e
            .attempts
            .iter()
            .filter(|a| !matches!(a.outcome, marg_core::AttemptOutcome::Success))
            .count();
        println!(
            "{:<24}  {:<36}  {:<10}  {:<22}  {:>6}  {:>10}  {:>10.6}  {:>4}  {:>4}",
            e.timestamp.to_rfc3339(),
            e.key_id,
            e.provider,
            e.model,
            e.input_tokens,
            e.output_tokens,
            e.cost_usd,
            e.latency_ms,
            failovers,
        );
    }
    Ok(())
}

async fn admin_bootstrap(config_path: &str, force: bool, label: Option<&str>) -> Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("loading config from {}", config_path))?;
    let storage = open_storage(config_path).await?;
    let count = storage
        .count_active_admin_tokens()
        .await
        .context("counting admin tokens")?;
    if count > 0 && !force {
        return Err(anyhow!(
            "active admin tokens already exist ({}). Re-run with --force to mint another.",
            count
        ));
    }

    let token = MargToken::generate();
    let plain = token.expose().to_string();
    let new = NewAdminToken {
        id: uuid::Uuid::new_v4().to_string(),
        token_hash: token.hash(),
        token_prefix: token.display_prefix(),
        label: label.unwrap_or("bootstrap").to_string(),
        created_at: Utc::now(),
    };
    let saved = storage
        .create_admin_token(new)
        .await
        .context("inserting admin token")?;

    let path = cfg.admin.bootstrap_token_path.trim();
    let written = if path.is_empty() {
        false
    } else {
        match write_token_file(path, &plain) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("warn: failed to write bootstrap token file at {}: {}", path, e);
                false
            }
        }
    };

    println!("ADMIN TOKEN ID:  {}", saved.id);
    println!("LABEL:           {}", saved.label);
    println!("CREATED:         {}", saved.created_at.to_rfc3339());
    println!();
    println!("ADMIN BEARER TOKEN (shown once, store it now):");
    println!("  {}", plain);
    if written {
        println!();
        println!("Also written to {} with mode 0600.", path);
    }
    println!();
    println!("Use with:");
    println!(
        "  curl -H 'Authorization: Bearer {}' http://localhost:{}/admin/keys",
        plain,
        cfg.admin
            .bind
            .rsplit_once(':')
            .map(|(_, p)| p)
            .unwrap_or("8081")
    );
    Ok(())
}

async fn admin_tokens_list(config_path: &str) -> Result<()> {
    let storage = open_storage(config_path).await?;
    let tokens = storage
        .list_admin_tokens()
        .await
        .context("listing admin tokens")?;
    if tokens.is_empty() {
        println!("no admin tokens");
        return Ok(());
    }
    println!(
        "{:<36}  {:<24}  {:<20}  {:<10}  {}",
        "id", "prefix", "created_at", "status", "label"
    );
    for t in tokens {
        let status = if t.revoked_at.is_some() { "revoked" } else { "active" };
        println!(
            "{:<36}  {:<24}  {:<20}  {:<10}  {}",
            t.id,
            t.token_prefix,
            t.created_at.to_rfc3339(),
            status,
            t.label,
        );
    }
    Ok(())
}

async fn admin_tokens_revoke(config_path: &str, id: &str) -> Result<()> {
    let storage = open_storage(config_path).await?;
    storage
        .revoke_admin_token(id)
        .await
        .with_context(|| format!("revoking admin token {}", id))?;
    println!("revoked admin token {}", id);
    Ok(())
}

/// Walk the Kavach audit JSONL files and print "would have refused" events.
/// Reads the per-process JSONL files written by marg-server's audit flush
/// task. Each line is a `SignedAuditEntry`; the Marg request-lifecycle JSON
/// lives in `signed_payload.data`. We only surface entries whose schema is
/// `marg.request.v1` and whose `verdict.real_kind != "permit"`.
async fn policy_audit(
    config_path: &str,
    since: Option<&str>,
    limit: usize,
    explicit_path: Option<&str>,
) -> Result<()> {
    let source_path = if let Some(p) = explicit_path {
        std::path::PathBuf::from(p)
    } else {
        let cfg = Config::load(config_path)
            .with_context(|| format!("loading config from {}", config_path))?;
        std::path::PathBuf::from(cfg.kavach.audit_export_path)
    };

    let since_cutoff = since
        .map(parse_since)
        .transpose()
        .with_context(|| "parsing --since")?;

    let files = collect_jsonl_files(&source_path)?;
    if files.is_empty() {
        println!("no audit JSONL files found at {}", source_path.display());
        return Ok(());
    }

    let mut rows: Vec<AuditRow> = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).with_context(|| format!("reading {}", f.display()))?;
        let parsed = kavach_pq::audit::parse_jsonl(&bytes)
            .with_context(|| format!("parsing JSONL at {}", f.display()))?;
        for entry in parsed {
            let inner: serde_json::Value =
                serde_json::from_slice(&entry.signed_payload.data).unwrap_or(serde_json::Value::Null);
            let schema = inner
                .get("schema")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if schema != "marg.request.v1" {
                continue;
            }
            let real_kind = inner
                .get("verdict")
                .and_then(|v| v.get("real_kind"))
                .and_then(|v| v.as_str())
                .unwrap_or("permit");
            if real_kind == "permit" {
                continue;
            }
            let ts: Option<chrono::DateTime<Utc>> = inner
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            if let (Some(cutoff), Some(ts)) = (since_cutoff, ts) {
                if ts < cutoff {
                    continue;
                }
            }
            rows.push(AuditRow {
                index: entry.index,
                timestamp: ts,
                mode: inner
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string(),
                real_kind: real_kind.to_string(),
                effective_kind: inner
                    .get("verdict")
                    .and_then(|v| v.get("effective_kind"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string(),
                principal: inner
                    .get("principal_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string(),
                action: inner
                    .get("action_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string(),
                evaluator: inner
                    .get("verdict")
                    .and_then(|v| v.get("evaluator"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                reason_code: inner
                    .get("verdict")
                    .and_then(|v| v.get("reason_code"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                reason_text: inner
                    .get("verdict")
                    .and_then(|v| v.get("reason_text"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            });
        }
    }

    rows.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    let total = rows.len();
    let display = rows.into_iter().rev().take(limit).rev().collect::<Vec<_>>();

    if display.is_empty() {
        println!("no would-refuse events found");
        return Ok(());
    }

    println!(
        "{:<6}  {:<20}  {:<8}  {:<10}  {:<10}  {:<24}  {:<28}  {}",
        "idx", "timestamp", "mode", "real", "effective", "principal", "action", "reason"
    );
    for r in display {
        let reason = format!(
            "[{}] {}: {}",
            r.reason_code.as_deref().unwrap_or("-"),
            r.evaluator.as_deref().unwrap_or("-"),
            r.reason_text.as_deref().unwrap_or("-")
        );
        println!(
            "{:<6}  {:<20}  {:<8}  {:<10}  {:<10}  {:<24}  {:<28}  {}",
            r.index,
            r.timestamp
                .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_else(|| "-".to_string()),
            r.mode,
            r.real_kind,
            r.effective_kind,
            truncate(&r.principal, 24),
            truncate(&r.action, 28),
            reason,
        );
    }
    println!();
    println!("({} matching events total)", total);
    Ok(())
}

struct AuditRow {
    index: u64,
    timestamp: Option<chrono::DateTime<Utc>>,
    mode: String,
    real_kind: String,
    effective_kind: String,
    principal: String,
    action: String,
    evaluator: Option<String>,
    reason_code: Option<String>,
    reason_text: Option<String>,
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn parse_since(raw: &str) -> Result<chrono::DateTime<Utc>> {
    let trimmed = raw.trim();
    let (num_part, suffix) = split_duration(trimmed);
    let n: i64 = num_part
        .parse()
        .map_err(|_| anyhow!("invalid duration value '{}'", num_part))?;
    let delta = match suffix {
        "s" => chrono::Duration::seconds(n),
        "m" => chrono::Duration::minutes(n),
        "h" => chrono::Duration::hours(n),
        "d" => chrono::Duration::days(n),
        _ => return Err(anyhow!("invalid duration suffix '{}' (use s/m/h/d)", suffix)),
    };
    Ok(Utc::now() - delta)
}

fn split_duration(raw: &str) -> (&str, &str) {
    let pos = raw
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| i)
        .unwrap_or(raw.len());
    raw.split_at(pos)
}

fn collect_jsonl_files(path: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    if path.is_file() {
        out.push(path.to_path_buf());
        return Ok(out);
    }
    for entry in std::fs::read_dir(path)
        .with_context(|| format!("listing {}", path.display()))?
        .flatten()
    {
        let p = entry.path();
        if p.is_file()
            && p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.ends_with(".jsonl"))
                .unwrap_or(false)
        {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

fn write_token_file(path: &str, contents: &str) -> std::io::Result<()> {
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
