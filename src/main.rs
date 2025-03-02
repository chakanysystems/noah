use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use tracing_subscriber;

mod relay;
#[cfg(feature = "search")]
mod search;

// Types
#[derive(Debug, Serialize, Deserialize)]
struct NostrEvent {
    id: String,
    pubkey: String,
    created_at: i64,
    kind: i32,
    content: String,
    tags: Value,
}

#[derive(Debug, Deserialize)]
struct PluginInput {
    #[serde(rename = "type")]
    msg_type: String,
    event: NostrEvent,
    receivedAt: i64,
    sourceType: String,
    sourceInfo: String,
}

#[derive(Debug, Serialize)]
struct PluginOutput {
    id: String,
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    msg: Option<String>,
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the daemon
    Daemon,
}

// App state definition - available regardless of features
#[derive(Clone)]
struct AppState {
    #[cfg(feature = "postgres")]
    pool: sqlx::PgPool,
    #[cfg(feature = "search")]
    openai_client: reqwest::Client,
    #[cfg(feature = "search")]
    openai_api_key: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Initialize tracing for logging
    tracing_subscriber::fmt::init();

    #[cfg(feature = "postgres")]
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&std::env::var("POSTGRES_CONNECTION")?)
        .await?;

    // Initialize state
    let state = Arc::new(AppState {
        #[cfg(feature = "postgres")]
        pool,
        openai_client: reqwest::Client::new(),
        openai_api_key: std::env::var("GEMINI_API_KEY")?,
    });

    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon => {
            println!("Starting daemon");
            #[cfg(feature = "search")]
            search::run_webserver(state).await?;

            #[cfg(not(feature = "search"))]
            println!("Search feature not enabled, webserver functionality unavailable");
        }
    }

    Ok(())
}
