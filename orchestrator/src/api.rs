use axum::Router;
use axum::{extract::Path, extract::State, http::StatusCode};
use reqwest::Client;
use restate_sdk::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::net::TcpListener;
use tokio::process::Command;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_scalar::Scalar;
use utoipa_scalar::Servable;

#[derive(OpenApi)]
#[openapi(paths(health, status, get_session, post_session, delete_session))]
pub struct ApiDoc;

#[derive(Clone)]
pub struct AppState {
    pub restate_base_url: String,
}

pub fn router(restate_base_url: String) -> Router {
    let state = AppState { restate_base_url };
    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health))
        .routes(routes!(status))
        .routes(routes!(get_session, post_session, delete_session))
        .with_state(state)
        .split_for_parts();
    router.merge(Scalar::with_url("/", api))
}
#[derive(Default, Clone, Deserialize, Serialize, JsonSchema)]
pub struct Session {
    id: String,
    available: bool,
    worker_id: String,
}

#[derive(Default, Clone, Deserialize, Serialize)]
pub struct Worker {
    id: String,
    port: Option<u16>,
    available: bool,
}
#[derive(Default, Deserialize, Serialize)]
pub struct Pool {
    session_list: Vec<Session>,
    worker_list: Vec<Worker>,
}
// Restate service definition
#[restate_sdk::object]
pub trait WorkerPoolService {
    async fn spawn_worker() -> Result<String, HandlerError>;
    async fn health_check(session_id: String) -> Result<String, HandlerError>;
    async fn status_check(session_id: String) -> Result<String, HandlerError>;
    // async fn health_poll(session_id: String) -> Result<(), HandlerError>;
    //async fn spawn_session(session_id: String) -> Result<(), HandlerError>;
    async fn get_session(session_id: String) -> Result<String, HandlerError>;
    async fn delete_session(session_id: String) -> Result<String, HandlerError>;
}
// Restate service implementation
impl WorkerPoolService for Pool {
    async fn health_check(
        &self,
        ctx: ObjectContext<'_>,
        session_id: String,
    ) -> Result<String, HandlerError> {
        let pool: Pool = match ctx.get::<Vec<u8>>("pool_state").await? {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => Pool::default(),
        };

        let session = pool
            .session_list
            .iter()
            .find(|s| s.id == session_id)
            .ok_or(TerminalError::new(format!(
                "Error fetching session from session_list"
            )))?;

        let worker = pool
            .worker_list
            .iter()
            .find(|w| w.id == session.worker_id)
            .ok_or(TerminalError::new(format!(
                "Error fetching session_worker from worker_list"
            )))?;

        let worker_port = worker
            .port
            .ok_or(TerminalError::new(format!("Error fetching worker port")))?;

        let client = Client::new();
        let health_status: String = ctx
            .run(move || async move {
                let response = client
                    .get(format!("http://localhost:{}/health", worker_port))
                    .send()
                    .await
                    .map_err(|e| {
                        TerminalError::new(format!("Failed to send health request: {}", e))
                    })?;

                let body = response.text().await.map_err(|e| {
                    TerminalError::new(format!("Failed to read health response body: {}", e))
                })?;

                Ok(body)
            })
            .await?;

        Ok(health_status)
    }
    async fn status_check(
        &self,
        ctx: ObjectContext<'_>,
        session_id: String,
    ) -> Result<String, HandlerError> {
        let pool: Pool = match ctx.get::<Vec<u8>>("pool_state").await? {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => Pool::default(),
        };

        let session = pool
            .session_list
            .iter()
            .find(|s| s.id == session_id)
            .ok_or(TerminalError::new(format!(
                "Error fetching session from session_list"
            )))?;

        let worker = pool
            .worker_list
            .iter()
            .find(|w| w.id == session.worker_id)
            .ok_or(TerminalError::new(format!(
                "Error fetching session_worker from worker_list"
            )))?;

        let worker_port = worker
            .port
            .ok_or(TerminalError::new(format!("Error fetching worker port")))?;

        let client = Client::new();
        let health_status: String = ctx
            .run(move || async move {
                let response = client
                    .get(format!("http://localhost:{}/status", worker_port))
                    .send()
                    .await
                    .map_err(|e| {
                        TerminalError::new(format!("Failed to send status request: {}", e))
                    })?;

                let body = response.text().await.map_err(|e| {
                    TerminalError::new(format!("Failed to read status response body: {}", e))
                })?;

                Ok(body)
            })
            .await?;

        Ok(health_status)
    }
    async fn spawn_worker(&self, mut ctx: ObjectContext<'_>) -> Result<String, HandlerError> {
        let ready_port = get_port();
        ctx.run(|| async {
            Command::new("steel-browser")
                .env("PORT", ready_port.unwrap_or_default().to_string())
                .spawn()
                .map(|_| ())
                .map_err(|e| {
                    TerminalError::new(format!("Error starting steel-browser: {}", e)).into()
                })
        })
        .await?;

        let worker_id = ctx.rand_uuid().to_string();
        let worker = Worker {
            id: worker_id.clone(),
            port: ready_port,
            available: true,
        };
        let session = Session {
            id: ctx.rand_uuid().to_string(),
            available: true,
            worker_id: worker_id,
        };
        let mut pool: Pool = match ctx.get::<Vec<u8>>("pool_state").await? {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => Pool::default(),
        };

        // Update state
        pool.worker_list.insert(0, worker.clone());
        pool.session_list.insert(0, session.clone());
        let worker_port = worker
            .port
            .ok_or(TerminalError::new(format!("Error fetching worker port")))?;

        let client = Client::new();
        let health_status: String = ctx
            .run(move || async move {
                let response = client
                    .post(format!(
                        "http://localhost:{}/sessions/{}",
                        worker_port, session.id
                    ))
                    .send()
                    .await
                    .map_err(|e| {
                        TerminalError::new(format!("Failed to send health request: {}", e))
                    })?;

                let body = response.text().await.map_err(|e| {
                    TerminalError::new(format!("Failed to read health response body: {}", e))
                })?;

                Ok(body)
            })
            .await?;
        let bytes = serde_json::to_vec(&pool)?;
        // Persist state
        ctx.set("pool_state", bytes);
        Ok(health_status)
    }
    async fn get_session(
        &self,
        ctx: ObjectContext<'_>,
        session_id: String,
    ) -> Result<String, HandlerError> {
        let pool: Pool = match ctx.get::<Vec<u8>>("pool_state").await? {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => Pool::default(),
        };

        let session = pool
            .session_list
            .iter()
            .find(|s| s.id == session_id)
            .ok_or(TerminalError::new(format!(
                "Error fetching session from session_list"
            )))?;

        let worker = pool
            .worker_list
            .iter()
            .find(|w| w.id == session.worker_id)
            .ok_or(TerminalError::new(format!(
                "Error fetching session_worker from worker_list"
            )))?;

        let worker_port = worker
            .port
            .ok_or(TerminalError::new(format!("Error fetching worker port")))?;

        let client = Client::new();
        let health_status: String = ctx
            .run(move || async move {
                let response = client
                    .get(format!(
                        "http://localhost:{}/sessions/{}",
                        worker_port, session.id
                    ))
                    .send()
                    .await
                    .map_err(|e| {
                        TerminalError::new(format!("Failed to send health request: {}", e))
                    })?;

                let body = response.text().await.map_err(|e| {
                    TerminalError::new(format!("Failed to read health response body: {}", e))
                })?;

                Ok(body)
            })
            .await?;

        Ok(health_status)
    }
    // This does not manage the worker state (worker remains undeleted)
    async fn delete_session(
        &self,
        ctx: ObjectContext<'_>,
        session_id: String,
    ) -> Result<String, HandlerError> {
        let pool: Pool = match ctx.get::<Vec<u8>>("pool_state").await? {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => Pool::default(),
        };

        let session = pool
            .session_list
            .iter()
            .find(|s| s.id == session_id)
            .ok_or(TerminalError::new(format!(
                "Error fetching session from session_list"
            )))?;

        let worker = pool
            .worker_list
            .iter()
            .find(|w| w.id == session.worker_id)
            .ok_or(TerminalError::new(format!(
                "Error fetching session_worker from worker_list"
            )))?;

        let worker_port = worker
            .port
            .ok_or(TerminalError::new(format!("Error fetching worker port")))?;

        let client = Client::new();
        let health_status: String = ctx
            .run(move || async move {
                let response = client
                    .delete(format!(
                        "http://localhost:{}/sessions/{}",
                        worker_port, session.id
                    ))
                    .send()
                    .await
                    .map_err(|e| {
                        TerminalError::new(format!("Failed to send health request: {}", e))
                    })?;

                let body = response.text().await.map_err(|e| {
                    TerminalError::new(format!("Failed to read health response body: {}", e))
                })?;

                Ok(body)
            })
            .await?;

        Ok(health_status)
    }
}
#[utoipa::path(
    get,
    path = "/health/{id}",
    params(
        ("id" = String, Path, description = "session id")
    ),
    responses(
        (status = 200, description = "health check status", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn health(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<String, (StatusCode, String)> {
    let client = Client::new();

    let url = format!(
        "{}/WorkerPoolService/health_check/{}",
        state.restate_base_url, id
    );

    let response = client.get(url).send().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to send health request: {e}"),
        )
    })?;

    let body = response.text().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read health response body: {e}"),
        )
    })?;

    Ok(body)
}
#[utoipa::path(
    get,
    path = "/status/{id}",
    params(
        ("id" = String, Path, description = "session id")  // ← Change to Path
    ),
    responses(
        (status = 200, description = "Status check of worker", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<String, (StatusCode, String)> {
    let client = Client::new();

    let url = format!(
        "{}/WorkerPoolService/status_check/{}",
        state.restate_base_url, id
    );

    let response = client.get(url).send().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to send status request: {e}"),
        )
    })?;

    response.text().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read status response: {e}"),
        )
    })
}
#[utoipa::path(
    get,
    path = "/session/{id}",
    params(
        ("id" = String, Path, description = "session id")
    ),
    responses(
        (status = 200, description = "session details", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<String, (StatusCode, String)> {
    let client = Client::new();

    let url = format!(
        "{}/WorkerPoolService/get_session/{}",
        state.restate_base_url, id
    );

    let response = client.get(url).send().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to send get_session request: {e}"),
        )
    })?;

    response.text().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read get_session response: {e}"),
        )
    })
}
#[utoipa::path(
    post,
    path = "/session",
    responses(
        (status = 200, description = "session created", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn post_session(State(state): State<AppState>) -> Result<String, (StatusCode, String)> {
    let client = Client::new();

    let url = format!("{}/WorkerPoolService/spawn_worker", state.restate_base_url);

    let response = client.post(url).send().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to spawn session: {e}"),
        )
    })?;

    response.text().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read spawn response: {e}"),
        )
    })
}
#[utoipa::path(
    delete,
    path = "/session/{id}",
    params(
        ("id" = String, Path, description = "session id")
    ),
    responses(
        (status = 200, description = "session deleted", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<String, (StatusCode, String)> {
    let client = Client::new();

    let url = format!(
        "{}/WorkerPoolService/delete_session/{}",
        state.restate_base_url, id
    );

    let response = client.delete(url).send().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to delete session: {e}"),
        )
    })?;

    response.text().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read delete response: {e}"),
        )
    })
}

// Affected by TOCTOU, fix for improvement
pub fn get_port() -> Option<u16> {
    let min_port: u16 = 3000;
    let max_port: u16 = u16::MAX;
    for p in min_port..max_port {
        if TcpListener::bind(("0.0.0.0", p)).is_ok() {
            return Some(p);
        }
    }
    None
}
