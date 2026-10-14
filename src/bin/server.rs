use clap::Parser;
use std::process;
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

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let pool = treasury::db::connect(&args.database_url).await?;
    let hash = treasury::utils::key_hash(args.operator_key.as_str());
    treasury::db::ensure_issuer(&pool, &hash).await?;

    let state = treasury::http::AppState { pool };

    let listener = TcpListener::bind(args.server_addr).await?;
    tracing::info!(addr = %listener.local_addr()?, "listening");

    treasury::http::serve(listener, state).await?;

    Ok(())
}
