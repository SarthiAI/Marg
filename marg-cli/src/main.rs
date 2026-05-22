use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use std::str::FromStr;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use marg_core::{secret, BudgetSpec, Config, MargToken, NewAdminToken, NewKey, PrincipalKind};
use marg_storage::{PostgresStorage, SqliteStorage, Storage};

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
        Command::Start { config } => marg_server::run(&config).await,
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
    }
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
            let storage = PostgresStorage::connect(&dsn)
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
