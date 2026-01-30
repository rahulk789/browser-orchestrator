pub mod api;

use api::WorkerPoolService;
use restate_sdk::prelude::*;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let restate_handle = tokio::spawn(async {
        HttpServer::new(
            Endpoint::builder()
                .bind(api::Pool::default().serve())
                .build(),
        )
        .listen_and_serve("127.0.0.1:4000".parse().unwrap())
        .await;
    });

    let restate_ingress = "http://127.0.0.1:8080".to_string();
    let listener = TcpListener::bind("127.0.0.1:3000").await?;
    let axum_handle = tokio::spawn(async move {
        axum::serve(listener, api::router(restate_ingress))
            .await
            .expect("axum server failed");
    });
    // Run restate + axum in background
    tokio::select! {
        _ = restate_handle => {},
        _ = axum_handle => {},
    }

    Ok(())
}
