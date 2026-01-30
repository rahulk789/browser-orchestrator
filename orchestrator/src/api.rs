use axum::Json;
use axum::{Router, response::IntoResponse};
use axum::{extract::Path, extract::State, http::StatusCode};
use reqwest::Client;
use restate_sdk::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::net::TcpListener;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_scalar::Scalar;
use utoipa_scalar::Servable;

#[derive(OpenApi)]
#[openapi(paths(health, status, get_session, post_session, delete_session))]
pub struct ApiDoc;

#[derive(Default, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AppState {
    pub restate_base_url: String,
}

pub fn router(restate_base_url: String) -> Router {
    let state = AppState { restate_base_url };
    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health))
        .routes(routes!(status))
        .routes(routes!(get_all_sessions))
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
    user: String,
    //   last_active: i64, // data: Data,
    // created_at: i64,
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
#[derive(Deserialize)]
struct CreateSessionResponse {
    id: String,
    created_at: i64,
    data: Data,
}
// #[derive(Deserialize)]
// struct CreateWorkerResponse {
//     id: String,
//     available: bool,
//     worker_id: String,
//     user: String,
//     last_active: i64,
//     created_at: i64,
//     data: Data,
// }
#[derive(Default, Clone, Deserialize, Serialize, JsonSchema, ToSchema)]
pub struct Data {
    pub user: String,
}
// Restate service definition
#[restate_sdk::object]
pub trait WorkerPoolService {
    async fn poll_stale_sessions() -> Result<(), HandlerError>;
    async fn poll_stale_workers() -> Result<(), HandlerError>;
    async fn spawn_worker(user: String) -> Result<String, HandlerError>;
    async fn health_check(session_id: String) -> Result<String, HandlerError>;
    async fn status_check(session_id: String) -> Result<String, HandlerError>;
    // async fn health_poll(session_id: String) -> Result<(), HandlerError>;
    //async fn spawn_session(session_id: String) -> Result<(), HandlerError>;
    async fn get_session(session_id: String) -> Result<String, HandlerError>;
    async fn get_all_sessions() -> Result<String, HandlerError>;
    async fn delete_session(session_id: String) -> Result<String, HandlerError>;
}
// Restate service implementation
impl WorkerPoolService for Pool {
    // Helps with inducing TTL based session timeouts
    async fn poll_stale_sessions(&self, ctx: ObjectContext<'_>) -> Result<(), HandlerError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let mut pool: Pool = match ctx.get::<Vec<u8>>("pool_state").await? {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => return Ok(()), // nothing to do
        };

        let mut remaining_sessions = Vec::new();

        for session in pool.session_list.into_iter() {
            let age = now - 234;

            if age < 60 {
                remaining_sessions.push(session);
                continue;
            }

            // stale session → delete remotely
            let worker = pool
                .worker_list
                .iter_mut()
                .find(|w| w.id == session.worker_id);

            if let Some(worker) = worker {
                if let Some(port) = worker.port {
                    let session_id = session.id.clone();

                    ctx.run(move || async move {
                        let client = Client::new();
                        let _ = client
                            .delete(format!("http://localhost:{}/sessions/{}", port, session_id))
                            .send()
                            .await;
                        Ok(())
                    })
                    .await?;
                }

                worker.available = true;
            }
        }

        pool.session_list = remaining_sessions;

        ctx.set("pool_state", serde_json::to_vec(&pool)?);

        Ok(())
    }
    async fn poll_stale_workers(&self, ctx: ObjectContext<'_>) -> Result<(), HandlerError> {
        let mut pool: Pool = match ctx.get::<Vec<u8>>("pool_state").await? {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => return Ok(()),
        };

        let mut healthy_workers = Vec::new();

        for mut worker in pool.worker_list.into_iter() {
            let client = Client::new();
            let health_status: String = ctx
                .run(move || async move {
                    let response = client
                        .get(format!(
                            "http://localhost:{}/health",
                            worker.port.unwrap_or_default().to_string()
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
            if health_status == "ok".to_string() {
                worker.available = true;
                healthy_workers.push(worker);
            }
        }

        pool.worker_list = healthy_workers;

        ctx.set("pool_state", serde_json::to_vec(&pool)?);

        Ok(())
    }

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
    async fn spawn_worker(
        &self,
        mut ctx: ObjectContext<'_>,
        user: String,
    ) -> Result<String, HandlerError> {
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

        // Give it a moment to start
        ctx.sleep(std::time::Duration::from_millis(500)).await?;
        let worker_id = ctx.rand_uuid().to_string();

        let worker = Worker {
            id: worker_id.clone(),
            port: ready_port,
            available: true,
        };

        let mut pool: Pool = match ctx.get::<Vec<u8>>("pool_state").await? {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => Pool::default(),
        };
        // Update worker
        pool.worker_list.insert(0, worker.clone());
        let worker_port = worker
            .port
            .ok_or(TerminalError::new(format!("Error fetching worker port")))?;
        let spawn_session: String = ctx
            .run(move || async move {
                let client = Client::new();

                let response = client
                    .post(format!("http://localhost:{}/sessions", worker_port))
                    .json(&serde_json::json!({ "user": user }))
                    .send()
                    .await
                    .map_err(|e| TerminalError::new(format!("Failed to create session: {}", e)))?;

                let body = response.text().await.map_err(|e| {
                    TerminalError::new(format!("Failed to read response body: {}", e))
                })?;

                Ok(body)
            })
            .await?;
        let parsed: CreateSessionResponse = serde_json::from_str(&spawn_session.clone())
            .map_err(|e| TerminalError::new(format!("Invalid JSON response: {}", e)))?;

        let session = Session {
            id: parsed.id.clone(),
            available: true,
            worker_id: worker_id.clone(),
            user: parsed.data.user.clone(),
            // last_active: parsed.created_at.clone(),
        };
        // Update Session
        pool.session_list.insert(0, session.clone());
        let bytes = serde_json::to_vec(&pool)?;
        // Persist state
        ctx.set("pool_state", bytes);
        Ok(spawn_session)
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
    async fn get_all_sessions(&self, ctx: ObjectContext<'_>) -> Result<String, HandlerError> {
        let pool: Pool = match ctx.get::<Vec<u8>>("pool_state").await? {
            Some(bytes) => serde_json::from_slice(&bytes)?,
            None => Pool::default(),
        };

        let mut results: Vec<String> = Vec::new();

        for worker in &pool.worker_list {
            let Some(port) = worker.port else {
                continue;
            };

            let client = Client::new();
            let body: String = ctx
                .run(move || async move {
                    let response = client
                        .get(format!("http://localhost:{}/status", port))
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
            results.push(body);
        }

        Ok(serde_json::to_string(&results)?)
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
        "{}/WorkerPoolService/pool/health_check",
        state.restate_base_url
    );

    let response = client.post(url).json(&id).send().await.map_err(|e| {
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
        "{}/WorkerPoolService/pool/status_check",
        state.restate_base_url
    );

    let response = client.post(url).json(&id).send().await.map_err(|e| {
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
pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<String, (StatusCode, String)> {
    let client = Client::new();

    let url = format!(
        "{}/WorkerPoolService/pool/get_session",
        state.restate_base_url
    );

    let response = client.post(url).json(&id).send().await.map_err(|e| {
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
    get,
    path = "/get_all_sessions",
    responses(
        (status = 200, description = "session details", body = String),
        (status = 500, description = "Internal server error", body = String)
    )
)]
pub async fn get_all_sessions(
    State(state): State<AppState>,
) -> Result<String, (StatusCode, String)> {
    let client = Client::new();

    let url = format!(
        "{}/WorkerPoolService/pool/get_all_sessions",
        state.restate_base_url
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
pub async fn post_session(
    State(state): State<AppState>,
    Json(payload): Json<Data>,
) -> impl IntoResponse {
    let client = Client::new();

    let url = format!(
        "{}/WorkerPoolService/pool/spawn_worker",
        state.restate_base_url
    );

    let response = client
        .post(url)
        .json(&payload.user)
        .send()
        .await
        .map_err(|e| {
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
pub async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<String, (StatusCode, String)> {
    let client = Client::new();

    let url = format!(
        "{}/WorkerPoolService/pool/delete_session",
        state.restate_base_url
    );

    let response = client.delete(url).json(&id).send().await.map_err(|e| {
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
