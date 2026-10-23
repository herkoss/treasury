use clap::Parser;
use std::{path::PathBuf, process, time::Duration};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(author, version, about = "Asset ledger with consent-based settlement")]
struct Args {
    #[arg(long, env = "SERVER_ADDR", default_value = "127.0.0.1:8888")]
    server_addr: String,
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
    #[arg(long, env = "OPERATOR_KEY")]
    operator_key: String,
    #[arg(long, env = "SWEEP_INTERVAL_SECS", default_value = "5")]
    sweep_interval_secs: u64,
    /// What every new account receives, as `USD=1000,XAU=10`; empty for nothing.
    #[arg(long, env = "WELCOME_GRANT", default_value = "")]
    welcome_grant: String,
    /// Directory with the demo UI, served from `/` when it exists.
    #[arg(long, env = "WEB_DIR", default_value = "web")]
    web_dir: PathBuf,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    if let Err(err) = run().await {
        eprintln!("Failed with {err}");
        process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let welcome = treasury::welcome::parse(&args.welcome_grant)
        .map_err(|err| format!("WELCOME_GRANT: {err}"))?;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let pool = treasury::db::connect(&args.database_url).await?;
    let hash = treasury::utils::key_hash(args.operator_key.as_str());
    treasury::db::ensure_issuer(&pool, &hash).await?;

    tokio::spawn(treasury::sweeper::run(
        pool.clone(),
        Duration::from_secs(args.sweep_interval_secs),
    ));

    let web_dir = if args.web_dir.is_dir() {
        tracing::info!(dir = %args.web_dir.display(), "serving the demo UI");
        Some(args.web_dir)
    } else {
        tracing::warn!(dir = %args.web_dir.display(), "no demo UI directory, serving the API only");
        None
    };
    if !welcome.is_empty() {
        tracing::info!(grant = args.welcome_grant, "welcome grant enabled");
    }

    let state = treasury::http::AppState {
        pool,
        welcome,
        web_dir,
    };

    let listener = TcpListener::bind(args.server_addr).await?;
    tracing::info!(addr = %listener.local_addr()?, "listening");

    treasury::http::serve(listener, state).await?;

    Ok(())
}
