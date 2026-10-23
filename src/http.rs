use axum::{
    extract::{FromRef, FromRequestParts, Request},
    http::{request::Parts, StatusCode},
    middleware,
    routing::{get, post},
    Json, Router,
};
use sqlx::PgPool;
use std::path::PathBuf;
use tokio::net::TcpListener;
use tower_http::{services::ServeDir, trace::TraceLayer};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};
use uuid::Uuid;

use crate::{db, error::Error, models::Account, utils, welcome::Grant};

pub mod accounts;
pub mod assets;
pub mod idempotency;
pub mod openapi;
pub mod transactions;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// What a new account receives at registration; empty means nothing.
    pub welcome: Vec<Grant>,
    /// The demo UI, served as static files from `/`; `None` leaves the API alone.
    pub web_dir: Option<PathBuf>,
}

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            welcome: Vec::new(),
            web_dir: None,
        }
    }
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

pub struct Actor(pub Account);

impl<S> FromRequestParts<S> for Actor
where
    S: Send + Sync,
    PgPool: FromRef<S>,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let key = parts
            .headers
            .get("x-account-key")
            .ok_or(Error::Unauthorized)?
            .to_str()
            .map_err(|_| Error::Unauthorized)?;

        let pool = PgPool::from_ref(state);
        let hash = utils::key_hash(key);

        let account = db::select_account_by_key_hash(&pool, &hash)
            .await?
            .ok_or(Error::Unauthorized)?;

        Ok(Actor(account))
    }
}

pub async fn serve(listener: TcpListener, state: AppState) -> std::io::Result<()> {
    let mut router = Router::new()
        .route("/health", get(health_check))
        .route(
            "/openapi.json",
            get(|| async { Json(openapi::ApiDoc::openapi()) }),
        )
        .merge(Scalar::with_url("/docs", openapi::ApiDoc::openapi()))
        .route("/assets", get(assets::list).post(assets::create))
        .route(
            "/assets/{code}",
            get(assets::read)
                .patch(assets::update)
                .delete(assets::archive),
        )
        .route("/accounts", get(accounts::list).post(accounts::create))
        .route("/accounts/{id}/balances", get(accounts::balances))
        .route("/accounts/{id}/entries", get(accounts::entries))
        .route("/accounts/{id}/inbox", get(transactions::inbox))
        .route("/accounts/{id}/outbox", get(transactions::outbox))
        .route("/transactions", post(transactions::create))
        .route("/transactions/{id}", get(transactions::read))
        .route("/transactions/{id}/approve", post(transactions::approve))
        .route("/transactions/{id}/reject", post(transactions::reject))
        .route("/transactions/{id}/cancel", post(transactions::cancel));
    if let Some(dir) = &state.web_dir {
        router = router.fallback_service(ServeDir::new(dir));
    }
    let router = router
        .layer(middleware::from_fn_with_state(
            state.pool.clone(),
            idempotency::layer,
        ))
        .layer(TraceLayer::new_for_http().make_span_with(|request: &Request| {
            tracing::info_span!("request", method = %request.method(), uri = %request.uri(), request_id = %Uuid::new_v4())
        }))
        .with_state(state);

    axum::serve(listener, router).await
}

async fn health_check() -> StatusCode {
    StatusCode::OK
}

pub fn parse_account(text: &str) -> Result<Uuid, Error> {
    if text == "@issuer" {
        return Ok(Uuid::nil());
    }
    Uuid::parse_str(text).map_err(|_| Error::InvalidAccount(text.to_string()))
}

pub fn account_ref(id: Uuid) -> String {
    if id.is_nil() {
        "@issuer".to_string()
    } else {
        id.to_string()
    }
}
