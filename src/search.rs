use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool};
use sqlx::{QueryBuilder, Row};
use std::sync::Arc;

use crate::AppState;

// Search-specific types
#[derive(Debug, Deserialize, Clone)]
pub struct SearchQuery {
    pub query: String,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub filters: Option<SearchFilters>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SearchFilters {
    pub pubkey: Option<String>,
    pub kind: Option<i32>,
    pub tags: Option<TagFilters>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TagFilters {
    pub exact: Option<serde_json::Value>,
    pub any: Option<Vec<String>>,
    pub values: Option<TagValueFilter>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TagValueFilter {
    pub key: String,
    pub values: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub results: Vec<EventWithSimilarity>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EventWithSimilarity {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: i32,
    pub content: String,
    pub tags: serde_json::Value,
    pub similarity: f64,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TagValue {
    pub value: String,
    pub count: i64,
}

// Handlers
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
        "SELECT id, pubkey, created_at, kind, content, tags, 1 - (embedding <-> ",
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
        LIMIT $2",
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
         LIMIT $2",
    )
    .bind(tag_key)
    .bind(limit);

    let values = query
        .fetch_all(&state.pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(Json(values))
}

pub async fn generate_embedding(
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
        .openai_client
        .post(format!("https://generativelanguage.googleapis.com/v1beta/models/text-embedding-004:embedContent?key={}", state.openai_api_key))
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

pub async fn run_webserver(state: Arc<AppState>) -> Result<(), Box<dyn std::error::Error>> {
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
