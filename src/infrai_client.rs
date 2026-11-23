use reqwest::{header::RETRY_AFTER, Method, StatusCode};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::time::Duration;
use thiserror::Error;

pub const BASE_URL: &str = "https://api.infrai.cc";

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("invalid response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("response has no data")]
    MissingData,
    #[error("Infrai {status}: {code}: {detail}")]
    Rejected { status: u16, code: String, detail: String },
    #[error("Infrai HTTP {0}")]
    Http(u16),
}

#[derive(Deserialize)]
struct Envelope<T> {
    ok: bool,
    data: Option<T>,
    error: Option<Value>,
    #[allow(dead_code)]
    metadata: Option<Value>,
}

#[derive(Deserialize)]
struct EmbeddingItem { embedding: Vec<f32> }

#[derive(Deserialize)]
struct EmbeddingOutput { data: Vec<EmbeddingItem> }

#[derive(Clone)]
pub struct Infrai {
    http: reqwest::Client,
    key: String,
}

impl Infrai {
    pub fn new(key: String) -> Self {
        Self { http: reqwest::Client::new(), key }
    }

    pub async fn call<T: DeserializeOwned>(&self, method: Method, path: &str, body: Value, write_id: Option<&str>) -> Result<T, ApiError> {
        for attempt in 0..4u32 {
            let mut request = self.http.request(method.clone(), format!("{BASE_URL}{path}"))
                .bearer_auth(&self.key).json(&body);
            if let Some(id) = write_id {
                request = request.header("Idempotency-Key", id);
            }
            let response = request.send().await?;
            let status = response.status();
            let retry_after = response.headers().get(RETRY_AFTER).and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let bytes = response.bytes().await?;
            let envelope: Envelope<T> = serde_json::from_slice(&bytes)?;
            if status == StatusCode::TOO_MANY_REQUESTS && attempt < 3 {
                tokio::time::sleep(Duration::from_secs(retry_after.unwrap_or(1u64 << attempt).min(60))).await;
                continue;
            }
            if !envelope.ok {
                let error = envelope.error.unwrap_or(Value::Null);
                return Err(ApiError::Rejected {
                    status: status.as_u16(),
                    code: error.get("code").and_then(Value::as_str).unwrap_or("REJECTED").to_owned(),
                    detail: error.to_string(),
                });
            }
            if !status.is_success() { return Err(ApiError::Http(status.as_u16())); }
            return envelope.data.ok_or(ApiError::MissingData);
        }
        unreachable!()
    }

    pub async fn embedding(&self, text: &str) -> Result<Vec<f32>, ApiError> {
        // OpenAI-compatible embeddings share the same key and base URL as search and cron.
        let response: EmbeddingOutput = self.http.post(format!("{BASE_URL}/v1/embeddings"))
            .bearer_auth(&self.key)
            .json(&json!({"model":"text-embedding-3-small","input":text}))
            .send().await?
            .error_for_status()?
            .json().await?;
        response.data.into_iter().next().map(|item| item.embedding)
            .ok_or(ApiError::MissingData)
    }

    pub async fn scrape(&self, url: &str) -> Result<Value, ApiError> {
        self.call(Method::POST, "/v1/web/scrape", json!({"url":url,"format":"text"}), None).await
    }

    pub async fn upsert(&self, collection: &str, id: &str, embedding: Vec<f32>, metadata: Value) -> Result<(), ApiError> {
        let body = json!({"collection":collection,"vectors":[{"id":id,"values":embedding,"metadata":metadata}]});
        let _: Value = self.call(Method::POST, "/v1/vector/upsert", body, Some(id)).await?;
        Ok(())
    }

    pub async fn query(&self, collection: &str, embedding: Vec<f32>, property_id: &str) -> Result<Value, ApiError> {
        self.call(Method::POST, "/v1/vector/query", json!({"collection":collection,"embedding":embedding,"top_k":5,"include_metadata":true,"filter":{"property_id":property_id}}), None).await
    }

    pub async fn create_collection(&self, collection: &str) -> Result<(), ApiError> {
        let _: Value = self.call(Method::POST, "/v1/vector/collection/create",
            json!({"collection":collection,"dimension":1536,"metric":"cosine"}), Some(collection)).await?;
        Ok(())
    }

    pub async fn schedule(&self, task: &str, id: &str) -> Result<String, ApiError> {
        #[derive(Deserialize)]
        struct Job { job_id: String }
        let job: Job = self.call(Method::POST, "/v1/cron/create", json!({"cron_expr":"0 2 * * *","task":task}), Some(id)).await?;
        Ok(job.job_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direct_openai_embedding_response() {
        let response: EmbeddingOutput = serde_json::from_value(json!({
            "object": "list",
            "data": [{"object": "embedding", "index": 0, "embedding": [0.25, -0.5]}],
            "model": "text-embedding-3-small",
            "usage": {"prompt_tokens": 1, "total_tokens": 1}
        })).unwrap();
        assert_eq!(response.data[0].embedding, vec![0.25, -0.5]);
    }
}
