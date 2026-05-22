use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    install_tracing();

    let cli = Cli::parse();
    match cli.command {
        Command::Start { config } => marg_server::run(&config).await,
        Command::Version { verbose } => {
            print_version(verbose);
            Ok(())
        }
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
