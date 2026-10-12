use axum::{
    extract::{FromRef, Request},
    http::StatusCode,
    routing::get,
    Router,
};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

pub async fn serve(listener: TcpListener, state: AppState) -> std::io::Result<()> {
    let router = Router::new()
        .route("/health", get(health_check))
        .layer(TraceLayer::new_for_http().make_span_with(|request: &Request| {
            tracing::info_span!("request", method = %request.method(), uri = %request.uri(), request_id = %Uuid::new_v4())
        }))
        .with_state(state);

    axum::serve(listener, router).await
}

async fn health_check() -> StatusCode {
    StatusCode::OK
}
