use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use std::str::FromStr;
use tracing_subscriber::EnvFilter;

use marg_core::{Config, MargToken, NewKey, PrincipalKind, BudgetSpec};
use marg_storage::{SqliteStorage, Storage};

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
    install_tracing();
    let cli = Cli::parse();
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
    }
}

fn install_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
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

async fn open_storage(config_path: &str) -> Result<SqliteStorage> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("loading config from {}", config_path))?;
    let storage = SqliteStorage::open(&cfg.storage.path)
        .await
        .with_context(|| format!("opening sqlite at {}", cfg.storage.path))?;
    storage.migrate().await.context("running database migrations")?;
    Ok(storage)
}

async fn db_migrate(config_path: &str) -> Result<()> {
    let _ = open_storage(config_path).await?;
    println!("migrations applied");
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
