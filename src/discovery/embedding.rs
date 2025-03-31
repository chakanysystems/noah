use crate::App;
use anyhow::Result;
use serde::{Deserialize, Serialize};

pub type Embedding = Vec<f32>;

pub mod cloudflare {
    #[derive(Serialize)]
    struct EmbeddingContext<'a> {
        text: &'a str,
    }

    #[derive(Serialize)]
    struct EmbeddingRequest<'a> {
        query: Option<&'a str>, // honestly we are probably never going to use this field
        contexts: Vec<CloudflareEmbeddingContext<'a>>,
    }

    #[derive(Deserialize)]
    struct EmbeddingResult {
        response: Vec<Embedding>,
    }

    #[derive(Deserialize)]
    struct EmbeddingResponse {
        result: CloudflareEmbeddingResult,
        success: bool,
        errors: Vec<String>,
        messages: Vec<String>,
    }

    pub async fn generate_embedding(app: &App, text: &str) -> Result<Embedding> {
        let response = app
            .web_client
            .post(format!(
                "https://api.cloudflare.com/client/v4/accounts/{}/ai/run/@cf/baai/bge-m3",
                app.cloudflare_account_id
            ))
            .header(
                "Authorization",
                format!("Bearer {}", app.cloudflare_api_key),
            )
            .json(&EmbeddingRequest {
                query: None,
                contexts: vec![EmbeddingContext { text: text }],
            })
            .send()
            .await?
            .json::<EmbeddingResponse>()
            .await?;

        Ok(response.result.response[0].clone())
    }
}
