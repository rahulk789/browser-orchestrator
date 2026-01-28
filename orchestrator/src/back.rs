use axum::{Router, extract::Query, extract::State, http::StatusCode};
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
struct Session {
    id: String,
    available: bool,
    worker_id: String,
}

#[derive(Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
struct Worker {
    id: String,
    port: Option<u16>,
    available: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct WorkerPool {
    sessions: HashSet<Session>,
    workers: HashSet<Worker>,
}

// Restate service definition
#[restate_sdk::service]
trait WorkerPoolService {
    async fn spawn_worker(&self, ctx: Context<'_>, session_id: String) -> Result<String, String>;
    async fn get_session(
        &self,
        ctx: Context<'_>,
        session_id: String,
    ) -> Result<Option<Session>, String>;
    async fn delete_session(&self, ctx: Context<'_>, session_id: String) -> Result<(), String>;
    async fn get_available_worker(&self, ctx: Context<'_>) -> Result<Option<Worker>, String>;
}

// Restate service implementation
#[restate_sdk::service]
impl WorkerPoolService for WorkerPool {
    async fn spawn_worker(
        &self,
        mut ctx: Context<'_>,
        session_id: String,
    ) -> Result<String, String> {
        let ready_port = get_port().ok_or("No available ports")?;

        Command::new("steel-browser")
            .env("PORT", ready_port.to_string())
            .spawn()
            .map_err(|e| format!("Failed to create browser session: {}", e))?;

        let worker_id = Uuid::new_v4().to_string();
        let worker = Worker {
            id: worker_id.clone(),
            port: Some(ready_port),
            available: true,
        };
        let session = Session {
            id: session_id.clone(),
            available: true,
            worker_id: worker_id.clone(),
        };

        // Get current state
        let mut pool: WorkerPool = ctx.get("pool").await.ok().flatten().unwrap_or_default();

        // Update state
        pool.workers.insert(worker);
        pool.sessions.insert(session);

        // Persist state
        ctx.set("pool", pool).await.map_err(|e| e.to_string())?;

        Ok(format!(
            "Worker {} spawned on port {}",
            worker_id, ready_port
        ))
    }

    async fn get_session(
        &self,
        ctx: Context<'_>,
        session_id: String,
    ) -> Result<Option<Session>, String> {
        let pool: WorkerPool = ctx.get("pool").await.ok().flatten().unwrap_or_default();
        Ok(pool.sessions.iter().find(|s| s.id == session_id).cloned())
    }

    async fn delete_session(&self, mut ctx: Context<'_>, session_id: String) -> Result<(), String> {
        let mut pool: WorkerPool = ctx.get("pool").await.ok().flatten().unwrap_or_default();

        // Remove session and associated worker
        if let Some(session) = pool.sessions.iter().find(|s| s.id == session_id).cloned() {
            pool.sessions.remove(&session);
            pool.workers.retain(|w| w.id != session.worker_id);
            ctx.set("pool", pool).await.map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    async fn get_available_worker(&self, ctx: Context<'_>) -> Result<Option<Worker>, String> {
        let pool: WorkerPool = ctx.get("pool").await.ok().flatten().unwrap_or_default();
        Ok(pool.workers.iter().find(|w| w.available).cloned())
    }
}

// Axum app state
#[derive(Clone)]
struct SharedState {
    restate_client: Arc<RestateClient>,
}

pub fn router(restate_client: RestateClient) -> Router {
    let state = SharedState {
        restate_client: Arc::new(restate_client),
    };

    OpenApiRouter::new()
        .routes(routes!(health, get_session, post_session, delete_session,))
        .with_state(state)
        .into()
}

#[derive(Deserialize, ToSchema)]
struct SessionParams {
    id: String,
}

#[utoipa::path(
    get,
    path = "/health",
    params(("id" = String, Query, description = "specify worker port")),
    responses(
        (status = 200, description = "health check ok", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn health(Query(params): Query<SessionParams>) -> Result<String, (StatusCode, String)> {
    let client = reqwest::Client::new();
    let res = client
        .get(format!("http://localhost:{}/health", params.id))
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if res.status().is_success() {
        Ok("ok".to_string())
    } else {
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("bad status: {}", res.status()),
        ))
    }
}

#[utoipa::path(
    get,
    path = "/session/{id}",
    params(("id" = String, Query, description = "session id")),
    responses(
        (status = 200, description = "session info", body = String),
        (status = 404, description = "Session not found", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn get_session(
    State(state): State<AppState>,
    Query(params): Query<SessionParams>,
) -> Result<String, (StatusCode, String)> {
    let session = state
        .restate_client
        .call::<WorkerPoolService>()
        .get_session(params.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match session {
        Some(s) => Ok(serde_json::to_string(&s).unwrap()),
        None => Err((StatusCode::NOT_FOUND, "Session not found".to_string())),
    }
}

#[utoipa::path(
    post,
    path = "/session",
    params(("id" = String, Query, description = "session id")),
    responses(
        (status = 200, description = "session created", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn post_session(
    State(state): State<AppState>,
    Query(params): Query<SessionParams>,
) -> Result<String, (StatusCode, String)> {
    state
        .restate_client
        .call::<WorkerPoolService>()
        .spawn_worker(params.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
}

#[utoipa::path(
    delete,
    path = "/session/{id}",
    params(("id" = String, Query, description = "session id")),
    responses(
        (status = 200, description = "session deleted", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn delete_session(
    State(state): State<AppState>,
    Query(params): Query<SessionParams>,
) -> Result<String, (StatusCode, String)> {
    state
        .restate_client
        .call::<WorkerPoolService>()
        .delete_session(params.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok("Session deleted".to_string())
}

// Affected by TOCTOU, fix for improvement
pub fn get_port() -> Option<u16> {
    let min_port: u16 = 1024;
    let max_port: u16 = u16::MAX;
    for p in min_port..max_port {
        if TcpListener::bind(("0.0.0.0", p)).is_ok() {
            return Some(p);
        }
    }
    None
}
