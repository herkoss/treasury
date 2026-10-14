use axum::{
    extract::{FromRef, FromRequestParts, Path, Request, State},
    http::{request::Parts, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::{
    db,
    error::{Error, Result as TreasuryResult},
    models::{Account, Asset},
    utils,
};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
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
    let router = Router::new()
        .route("/health", get(health_check))
        .route("/assets", post(create_asset).get(get_assets))
        .route("/assets/{code}", get(get_asset))
        .layer(TraceLayer::new_for_http().make_span_with(|request: &Request| {
            tracing::info_span!("request", method = %request.method(), uri = %request.uri(), request_id = %Uuid::new_v4())
        }))
        .with_state(state);

    axum::serve(listener, router).await
}

async fn health_check() -> StatusCode {
    StatusCode::OK
}

#[derive(Deserialize)]
pub struct CreateAsset {
    pub code: String,
    pub name: String,
    pub scale: u8,
}

pub async fn create_asset(
    Actor(actor): Actor,
    State(pool): State<PgPool>,
    Json(create_asset): Json<CreateAsset>,
) -> TreasuryResult<(StatusCode, Json<Asset>)> {
    if !actor.is_issuer {
        return Err(Error::Forbidden);
    }

    let asset = db::insert_asset(&pool, create_asset).await?;

    Ok((StatusCode::CREATED, Json(asset)))
}

pub async fn get_assets(
    Actor(_actor): Actor,
    State(pool): State<PgPool>,
) -> TreasuryResult<Json<Vec<Asset>>> {
    let assets = db::select_assets(&pool).await?;

    Ok(Json(assets))
}

pub async fn get_asset(
    Actor(_actor): Actor,
    State(pool): State<PgPool>,
    Path(code): Path<String>,
) -> TreasuryResult<Json<Asset>> {
    let asset = db::select_asset_by_code(&pool, code.as_str())
        .await?
        .ok_or(Error::NotFound("asset"))?;

    Ok(Json(asset))
}
