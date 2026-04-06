use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPayload {
    pub message: String,
}

struct EventState {
    sender: broadcast::Sender<EventPayload>,
}

pub async fn run(port: u16, sender: broadcast::Sender<EventPayload>) -> anyhow::Result<()> {
    let state = Arc::new(EventState { sender });
    let router = Router::new()
        .route("/event", post(handle_event))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("Event server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow::anyhow!("Cannot bind event server on port {}: {}", port, e))?;
    axum::serve(listener, router).await?;
    Ok(())
}

async fn handle_event(
    State(state): State<Arc<EventState>>,
    Json(payload): Json<EventPayload>,
) -> StatusCode {
    tracing::info!("Received event: {:?}", payload.message);
    let _ = state.sender.send(payload);
    StatusCode::OK
}
