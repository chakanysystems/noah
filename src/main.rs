use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{post, get},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::migrate::Migrator;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::Arc;

pub mod db;
#[cfg(feature = "discovery")]
pub mod discovery;

// Types
#[derive(Debug, Serialize, Deserialize)]
struct NostrEvent {
    id: String,
    pubkey: String,
    created_at: i64,
    kind: i32,
    content: String,
    tags: serde_json::Value,
}

// App state
#[derive(Clone)]
pub struct App {
    //pub db: db::Db,
    pub web_client: reqwest::Client,
    pub cloudflare_account_id: Option<String>,
    // is this a bad idea? ;)
    pub cloudflare_api_key: Option<String>,
}

use clap::{Parser, Subcommand};

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

use std::io::{self, BufRead, Write};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing for logging
    tracing_subscriber::fmt::init();

    //let pool = db::initDb(&std::env::var("POSTGRES_CONNECTION")?).await?;

    let cf_acc_id = match std::env::var("CLOUDFLARE_ACCOUNT_ID") {
        Ok(v) => Some(v),
        Err(e) => None,
    };

    let cf_api_key = match std::env::var("CLOUDFLARE_API_KEY") {
        Ok(v) => Some(v),
        Err(e) => None,
    };

    // Initialize state
    let state = Arc::new(App {
        //db: pool,
        web_client: reqwest::Client::new(),
        cloudflare_account_id: cf_acc_id,
        cloudflare_api_key: cf_api_key,
    });

    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon => {
            println!("Starting daemon");
            run_webserver(state).await;
        }
    }

    Ok(())
}

async fn home_path(
    State(state): State<Arc<App>>
) -> Result<&'static str, AppError> {
    Ok("Hello World")
}

async fn run_webserver(state: Arc<App>) -> Result<()> {
    println!("Welcome to noah");
    println!("from Chakany Systems");
    // Create router
    let app = Router::new()
        .route("/", get(home_path));
        //.route("/api/events/:event_id/similar", get(get_similar_events))
        //.route("/api/tags/:tag_key/values", get(get_tag_values))

    #[cfg(feature = "search")]
    let app = app.route("/api/search", post(api_search_events));
    let app = app.with_state(state);

    // Start server
    println!("listening on port 3000");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", self.0),
        )
            .into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

#[cfg(feature = "search")]
async fn api_search_events(
    State(state): State<Arc<App>>,
    Json(search_query): Json<queries::SearchQuery>,
) -> Result<Json<queries::SearchResult>, AppError> {
    Ok(Json(queries::search_events(&state, search_query).await?))
}
