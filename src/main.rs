use axum::{
    routing::{get, post},
    extract::{Path, Query, State},
    Json, Router,
};
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::Arc;

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

#[derive(Debug, Deserialize, Clone)]
struct SearchQuery {
    query: String,
    limit: Option<i64>,
    offset: Option<i64>,
    filters: Option<SearchFilters>,
}

#[derive(Debug, Deserialize, Clone)]
struct SearchFilters {
    pubkey: Option<String>,
    kind: Option<i32>,
    tags: Option<TagFilters>,
}

#[derive(Debug, Deserialize, Clone)]
struct TagFilters {
    exact: Option<serde_json::Value>,
    any: Option<Vec<String>>,
    values: Option<TagValueFilter>,
}

#[derive(Debug, Deserialize, Clone)]
struct TagValueFilter {
    key: String,
    values: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    results: Vec<EventWithSimilarity>,
    total: i64,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct EventWithSimilarity {
    id: String,
    pubkey: String,
    created_at: i64,
    kind: i32,
    content: String,
    tags: serde_json::Value,
    similarity: f64,
}

// App state
#[derive(Clone)]
struct AppState {
    pool: PgPool,
    openai_client: reqwest::Client,
    openai_api_key: String,
}

// Handlers
use sqlx::{QueryBuilder, Row};

async fn search_events(
    State(state): State<Arc<AppState>>,
    Json(search_query): Json<SearchQuery>,
) -> Result<Json<SearchResult>, String> {
    let limit = search_query.limit.unwrap_or(10);
    let offset = search_query.offset.unwrap_or(0);
    
    let embedding_vec = generate_embedding(&state, &search_query.query)
        .await
        .map_err(|e| e.to_string())?;

    let embedding = Vector::from(embedding_vec);

    // Start building the base query
    let mut qb = QueryBuilder::new(
        "SELECT id, pubkey, created_at, kind, content, tags, 1 - (embedding <-> "
    );

        qb.push_bind(embedding.clone())
        .push(") as similarity FROM nostr_search.events");

    // Add WHERE clause if we have filters
    if let Some(filters) = search_query.clone().filters {
        let mut first_condition = true;

        if let Some(pubkey) = filters.pubkey {
            qb.push(" WHERE pubkey = ");
            qb.push_bind(pubkey);
            first_condition = false;
        }

        if let Some(kind) = filters.kind {
            if first_condition {
                qb.push(" WHERE ");
            } else {
                qb.push(" AND ");
            }
            qb.push("kind = ");
            qb.push_bind(kind);
            first_condition = false;
        }

        if let Some(tag_filters) = filters.tags {
            if let Some(exact) = tag_filters.exact {
                if first_condition {
                    qb.push(" WHERE ");
                } else {
                    qb.push(" AND ");
                }
                qb.push("tags @> ");
                qb.push_bind(exact);
            }
        }
    }

    // Add ordering, limit and offset
    qb.push(" ORDER BY embedding <-> ")
        .push_bind(embedding)
      .push(" LIMIT ")
      .push_bind(limit)
      .push(" OFFSET ")
      .push_bind(offset);

    // Build and execute the query
    let mut query = qb.build_query_as::<EventWithSimilarity>();

    let results = query
        .fetch_all(&state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // Build the count query
    let mut count_qb = QueryBuilder::new("SELECT COUNT(*) FROM nostr_search.events");
    
    if let Some(filters) = search_query.filters {
        let mut first_condition = true;

        if let Some(pubkey) = filters.pubkey {
            count_qb.push(" WHERE pubkey = ");
            count_qb.push_bind(pubkey);
            first_condition = false;
        }

        if let Some(kind) = filters.kind {
            if first_condition {
                count_qb.push(" WHERE ");
            } else {
                count_qb.push(" AND ");
            }
            count_qb.push("kind = ");
            count_qb.push_bind(kind);
            first_condition = false;
        }

        if let Some(tag_filters) = filters.tags {
            if let Some(exact) = tag_filters.exact {
                if first_condition {
                    count_qb.push(" WHERE ");
                } else {
                    count_qb.push(" AND ");
                }
                count_qb.push("tags @> ");
                count_qb.push_bind(exact);
            }
        }
    }

    let total: i64 = count_qb
        .build()
        .fetch_one(&state.pool)
        .await
        .map_err(|e| e.to_string())?
        .get(0);

    Ok(Json(SearchResult {
        results,
        total,
        limit,
        offset,
    }))
}

async fn get_similar_events(
    State(state): State<Arc<AppState>>,
    Path(event_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<EventWithSimilarity>>, String> {
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<i64>().ok())
        .unwrap_or(5);

    let query = sqlx::query_as::<_, EventWithSimilarity>(
        "WITH event_embedding AS (
            SELECT embedding
            FROM nostr_search.events
            WHERE id = $1
        )
        SELECT 
            ne.id,
            ne.pubkey,
            ne.created_at,
            ne.kind,
            ne.content,
            ne.tags,
            1 - (ne.embedding <-> e.embedding) as similarity
        FROM nostr_search.events ne, event_embedding e
        WHERE ne.id != $1
        ORDER BY ne.embedding <-> e.embedding
        LIMIT $2"
    )
    .bind(event_id)
    .bind(limit);

    let similar_events = query
        .fetch_all(&state.pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(Json(similar_events))
}

async fn get_tag_values(
    State(state): State<Arc<AppState>>,
    Path(tag_key): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<TagValue>>, String> {
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<i64>().ok())
        .unwrap_or(100);

    let query = sqlx::query_as::<_, TagValue>(
        "SELECT DISTINCT tag->>'value' as value, COUNT(*) as count
         FROM nostr_search.events,
              jsonb_array_elements(tags) tag
         WHERE tag->>'key' = $1
         GROUP BY tag->>'value'
         ORDER BY count DESC
         LIMIT $2"
    )
    .bind(tag_key)
    .bind(limit);

    let values = query
        .fetch_all(&state.pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(Json(values))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct TagValue {
    value: String,
    count: i64,
}

async fn generate_embedding(
    state: &AppState,
    text: &str,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    #[derive(Serialize)]
    struct EmbeddingRequest<'a> {
        model: &'static str,
        content: Content<'a>,
    }

    #[derive(Serialize)]
    struct Content<'a> {
        parts: Vec<Part<'a>>,
    }

    #[derive(Serialize)]
    struct Part<'a> {
        text: &'a str,
    }
    
    #[derive(Deserialize)]
    struct EmbeddingResponse {
        embedding: Embedding,
    }

    #[derive(Deserialize)]
    struct Embedding {
        values: Vec<f32>,
    }

    let response = state
        .openai_client // Assuming this is a generic HTTP client. Rename if necessary.
        .post(format!("https://generativelanguage.googleapis.com/v1beta/models/text-embedding-004:embedContent?key={}", state.openai_api_key)) //Replace openai_api_key with the appropriate field, e.g. gemini_api_key
        .json(&EmbeddingRequest {
            model: "models/text-embedding-004",
            content: Content {
                parts: vec![Part { text }],
            },
        })
        .send()
        .await?
        .json::<EmbeddingResponse>()
        .await?;

    Ok(response.embedding.values.clone())
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
    /// Add something
    Add {
        /// If true, expects direct NostrEvent JSON instead of plugin format
        #[arg(short, long, default_value_t = false)]
        direct: bool,
    },
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

async fn process_stdin_events(state: &AppState, direct: bool) -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        
        // Parse input based on mode
        let event = if direct {
            // Direct mode: parse as NostrEvent
            match serde_json::from_str::<NostrEvent>(&line) {
                Ok(event) => event,
                Err(e) => {
                    eprintln!("Error parsing NostrEvent: {}", e);
                    continue;
                }
            }
        } else {
            // Plugin mode: parse as PluginInput
            match serde_json::from_str::<PluginInput>(&line) {
                Ok(input) => {
                    // Send immediate accept response
                    let output = PluginOutput {
                        id: input.event.id.clone(),
                        action: "accept".to_string(),
                        msg: None,
                    };
                    serde_json::to_writer(&mut stdout, &output)?;
                    stdout.write_all(b"\n")?;
                    stdout.flush()?;
                    
                    input.event
                }
                Err(e) => {
                    eprintln!("Error parsing PluginInput: {}", e);
                    continue;
                }
            }
        };

        // Check if event already exists
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM nostr_search.events WHERE id = $1)"
        )
        .bind(&event.id)
        .fetch_one(&state.pool)
        .await?;

        if !exists && event.kind == 1 {
            if let Ok(embedding) = generate_embedding(&state, &event.content).await {
                // Insert the event into the database
                if let Err(e) = sqlx::query(
                    "INSERT INTO nostr_search.events (id, pubkey, created_at, kind, content, tags, embedding) 
                     VALUES ($1, $2, $3, $4, $5, $6, $7)"
                )
                .bind(&event.id)
                .bind(&event.pubkey)
                .bind(event.created_at)
                .bind(event.kind)
                .bind(&event.content)
                .bind(&event.tags)
                .bind(&embedding[..])
                .execute(&state.pool)
                .await {
                    eprintln!("Failed to insert event {}: {}", event.id, e);
                } else if direct {
                    println!("Successfully added event {}", event.id);
                }
            } else {
                eprintln!("Failed to generate embedding for event {}", event.id);
            }
        } else if direct {
            println!("Event {} already exists, skipping", event.id);
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing for logging
    tracing_subscriber::fmt::init();

    // Create connection pool
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&std::env::var("POSTGRES_CONNECTION")?)
        .await?;

    // Initialize state
    let state = Arc::new(AppState {
        pool,
        openai_client: reqwest::Client::new(),
        openai_api_key: std::env::var("GEMINI_API_KEY")?,
    });

    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon => {
            println!("Starting daemon");
            run_webserver(state).await;
        }
        Commands::Add { direct } => {
            process_stdin_events(&state, direct).await?;
        }
    }

    Ok(())
}

use sqlx::{Pool, Postgres};

async fn run_webserver(state: Arc<AppState>) -> Result<(), Box<dyn std::error::Error>> {
    // Create router
    let app = Router::new()
        .route("/api/search", post(search_events))
        .route("/api/events/:event_id/similar", get(get_similar_events))
        .route("/api/tags/:tag_key/values", get(get_tag_values))
        .with_state(state);

    // Start server
    println!("listening on port 3000");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}
