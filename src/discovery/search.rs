use crate::{App, embedding::generate_embedding};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row};
use pgvector::Vector;

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

#[derive(Debug, Deserialize, Clone)]
pub struct SearchQuery {
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

#[derive(Debug, Serialize)]
pub struct SearchResult {
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

pub async fn search_events(app: &App, search_query: SearchQuery) -> Result<SearchResult> {
    let limit = search_query.limit.unwrap_or(10);
    let offset = search_query.offset.unwrap_or(0);

    let embedding_vec = generate_embedding(&app, &search_query.query).await?;

    let embedding = Vector::from(embedding_vec);

    // Start building the base query
    let mut qb = QueryBuilder::new(
        "SELECT id, pubkey, created_at, kind, content, tags, 1 - (embedding <=> ",
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
    qb.push(" ORDER BY embedding <=> ")
        .push_bind(embedding)
        .push(" LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);

    // Build and execute the query
    let query = qb.build_query_as::<EventWithSimilarity>();

    let results = query.fetch_all(&app.db).await?;

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
        .fetch_one(&app.db)
        .await?
        .get(0);

    Ok(SearchResult {
        results,
        total,
        limit,
        offset,
    })
}

//pub async fn get_similar_events(
//    State(state): State<Arc<App>>,
//    Path(event_id): Path<String>,
//    Query(params): Query<std::collections::HashMap<String, String>>,
//) -> Result<Json<Vec<EventWithSimilarity>>, String> {
//    let limit = params
//        .get("limit")
//        .and_then(|l| l.parse::<i64>().ok())
//        .unwrap_or(5);
//
//    let query = sqlx::query_as::<_, EventWithSimilarity>(
//        "WITH event_embedding AS (
//            SELECT embedding
//            FROM nostr_search.events
//            WHERE id = $1
//        )
//        SELECT
//            ne.id,
//            ne.pubkey,
//            ne.created_at,
//            ne.kind,
//            ne.content,
//            ne.tags,
//            1 - (ne.embedding <=> e.embedding) as similarity
//        FROM nostr_search.events ne, event_embedding e
//        WHERE ne.id != $1
//        ORDER BY ne.embedding <=> e.embedding
//        LIMIT $2",
//    )
//    .bind(event_id)
//    .bind(limit);
//
//    let similar_events = query
//        .fetch_all(&state.pool)
//        .await
//        .map_err(|e| e.to_string())?;
//
//    Ok(Json(similar_events))
//}
//
//pub async fn get_tag_values(
//    State(state): State<Arc<App>>,
//    Path(tag_key): Path<String>,
//    Query(params): Query<std::collections::HashMap<String, String>>,
//) -> Result<Json<Vec<TagValue>>, String> {
//    let limit = params
//        .get("limit")
//        .and_then(|l| l.parse::<i64>().ok())
//        .unwrap_or(100);
//
//    let query = sqlx::query_as::<_, TagValue>(
//        "SELECT DISTINCT tag->>'value' as value, COUNT(*) as count
//         FROM nostr_search.events,
//              jsonb_array_elements(tags) tag
//         WHERE tag->>'key' = $1
//         GROUP BY tag->>'value'
//         ORDER BY count DESC
//         LIMIT $2",
//    )
//    .bind(tag_key)
//    .bind(limit);
//
//    let values = query
//        .fetch_all(&state.pool)
//        .await
//        .map_err(|e| e.to_string())?;
//
//    Ok(Json(values))
//}
//
//#[derive(Debug, Serialize, sqlx::FromRow)]
//struct TagValue {
//    value: String,
//    count: i64,
//}
