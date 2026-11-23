mod infrai_client;

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use infrai_client::{ApiError, Infrai};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{env, sync::Arc};
use thiserror::Error;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Kind { MaintenanceRequest, TenantDocument, InspectionReminder }

#[derive(Clone, Deserialize, Serialize)]
struct Document { property_id: String, kind: Kind, url: String }

#[derive(Clone)]
struct Service { api: Infrai, documents: Vec<Document>, collection: String }

#[derive(Debug, Error)]
enum ServiceError {
    #[error("configuration: {0}")]
    Config(String),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl axum::response::IntoResponse for ServiceError {
    fn into_response(self) -> axum::response::Response {
        let status = match &self {
            Self::Api(ApiError::Rejected { status, .. }) if (400..500).contains(status) => StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_REQUEST),
            Self::Config(_) | Self::Json(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::BAD_GATEWAY,
        };
        (status, Json(json!({"error":self.to_string()}))).into_response()
    }
}

fn document_id(document: &Document) -> String {
    let kind = match document.kind { Kind::MaintenanceRequest => "maintenance", Kind::TenantDocument => "tenant", Kind::InspectionReminder => "inspection" };
    let digest = Sha256::digest(format!("{}:{kind}:{}", document.property_id, document.url).as_bytes());
    hex::encode(digest)
}

fn selected<'a>(documents: &'a [Document], property_id: &str) -> Vec<&'a Document> {
    documents.iter().filter(|doc| doc.property_id == property_id).collect()
}

async fn reindex(State(state): State<Arc<Service>>) -> Result<Json<Value>, ServiceError> {
    let mut indexed = 0;
    for doc in &state.documents {
        let scraped = state.api.scrape(&doc.url).await?;
        let content = scraped.get("content").and_then(Value::as_str).map(str::to_owned)
            .unwrap_or_else(|| scraped.to_string());
        let embedding = state.api.embedding(&content).await?;
        state.api.upsert(&state.collection, &document_id(doc), embedding,
            json!({"property_id":doc.property_id,"kind":doc.kind,"url":doc.url,"content":content})).await?;
        indexed += 1;
    }
    Ok(Json(json!({"indexed":indexed,"collection":state.collection})))
}

#[derive(Deserialize)]
struct Search { property_id: String, query: String }

async fn search(State(state): State<Arc<Service>>, Json(input): Json<Search>) -> Result<Json<Value>, ServiceError> {
    if selected(&state.documents, &input.property_id).is_empty() {
        return Err(ServiceError::Config("unknown property_id".into()));
    }
    let embedding = state.api.embedding(&input.query).await?;
    let results = state.api.query(&state.collection, embedding, &input.property_id).await?;
    Ok(Json(json!({"property_id":input.property_id,"results":results})))
}

#[tokio::main]
async fn main() -> Result<(), ServiceError> {
    let key = env::var("INFRAI_API_KEY").map_err(|e| ServiceError::Config(e.to_string()))?;
    let documents: Vec<Document> = serde_json::from_str(&env::var("PROPERTY_DOCUMENTS_JSON")
        .map_err(|e| ServiceError::Config(e.to_string()))?)?;
    let collection = env::var("INFRAI_COLLECTION").unwrap_or_else(|_| "property_records".into());
    let api = Infrai::new(key);
    if env::args().nth(1).as_deref() == Some("setup") {
        api.create_collection(&collection).await?;
        println!("collection={collection}");
        return Ok(());
    }
    if env::args().nth(1).as_deref() == Some("schedule") {
        let task = env::var("PUBLIC_REINDEX_URL").map_err(|e| ServiceError::Config(e.to_string()))?;
        let id = format!("nightly:{}:{task}", collection);
        println!("job_id={}", api.schedule(&task, &id).await?);
        return Ok(());
    }
    let bind = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let app = Router::new().route("/reindex", post(reindex)).route("/search", post(search))
        .with_state(Arc::new(Service { api, documents, collection }));
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reindex_scope_and_stable_identity() {
        let docs = vec![
            Document { property_id: "elm-12".into(), kind: Kind::MaintenanceRequest, url: "https://example.org/repair".into() },
            Document { property_id: "elm-12".into(), kind: Kind::InspectionReminder, url: "https://example.org/inspection".into() },
            Document { property_id: "oak-4".into(), kind: Kind::TenantDocument, url: "https://example.org/lease".into() },
        ];
        let scoped = selected(&docs, "elm-12");
        assert_eq!(scoped.len(), 2);
        assert_eq!(document_id(scoped[0]), document_id(&docs[0]));
        assert_ne!(document_id(scoped[0]), document_id(scoped[1]));
    }
}
