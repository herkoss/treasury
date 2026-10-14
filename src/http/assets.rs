use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::{
    db,
    domain::validate_asset,
    error::{Error, Problem, Result},
    models::Asset,
};

use super::Actor;

#[derive(Deserialize, ToSchema)]
pub struct CreateAsset {
    pub code: String,
    pub name: String,
    pub scale: u8,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateAsset {
    pub name: String,
}

#[utoipa::path(
    post,
    path = "/assets",
    tag = "assets",
    request_body = CreateAsset,
    responses(
        (status = 201, body = Asset),
        (status = 403, body = Problem, description = "not the issuer"),
        (status = 409, body = Problem, description = "code already exists"),
        (status = 422, body = Problem, description = "bad code or scale"),
    ),
    security(("account_key" = []))
)]
pub async fn create(
    Actor(actor): Actor,
    State(pool): State<PgPool>,
    Json(input): Json<CreateAsset>,
) -> Result<(StatusCode, Json<Asset>)> {
    if !actor.is_issuer {
        return Err(Error::Forbidden);
    }
    validate_asset(&input.code, input.scale)?;

    let asset = db::insert_asset(&pool, &input.code, &input.name, i16::from(input.scale)).await?;

    Ok((StatusCode::CREATED, Json(asset)))
}

#[utoipa::path(
    get,
    path = "/assets",
    tag = "assets",
    responses((status = 200, body = Vec<Asset>)),
    security(("account_key" = []))
)]
pub async fn list(Actor(_actor): Actor, State(pool): State<PgPool>) -> Result<Json<Vec<Asset>>> {
    let assets = db::select_assets(&pool).await?;

    Ok(Json(assets))
}

#[utoipa::path(
    get,
    path = "/assets/{code}",
    tag = "assets",
    params(("code" = String, Path)),
    responses((status = 200, body = Asset), (status = 404, body = Problem)),
    security(("account_key" = []))
)]
pub async fn read(
    Actor(_actor): Actor,
    State(pool): State<PgPool>,
    Path(code): Path<String>,
) -> Result<Json<Asset>> {
    let asset = db::select_asset_by_code(&pool, &code)
        .await?
        .ok_or(Error::NotFound("asset"))?;

    Ok(Json(asset))
}

#[utoipa::path(
    patch,
    path = "/assets/{code}",
    tag = "assets",
    params(("code" = String, Path)),
    request_body = UpdateAsset,
    responses(
        (status = 200, body = Asset),
        (status = 403, body = Problem),
        (status = 404, body = Problem),
    ),
    security(("account_key" = []))
)]
pub async fn update(
    Actor(actor): Actor,
    State(pool): State<PgPool>,
    Path(code): Path<String>,
    Json(input): Json<UpdateAsset>,
) -> Result<Json<Asset>> {
    if !actor.is_issuer {
        return Err(Error::Forbidden);
    }

    let asset = db::update_asset_name(&pool, &code, &input.name)
        .await?
        .ok_or(Error::NotFound("asset"))?;

    Ok(Json(asset))
}

#[utoipa::path(
    delete,
    path = "/assets/{code}",
    tag = "assets",
    params(("code" = String, Path)),
    responses(
        (status = 200, body = Asset, description = "archived"),
        (status = 403, body = Problem),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "balances or pending offers still use it"),
    ),
    security(("account_key" = []))
)]
pub async fn archive(
    Actor(actor): Actor,
    State(pool): State<PgPool>,
    Path(code): Path<String>,
) -> Result<Json<Asset>> {
    if !actor.is_issuer {
        return Err(Error::Forbidden);
    }
    db::select_asset_by_code(&pool, &code)
        .await?
        .ok_or(Error::NotFound("asset"))?;

    let asset = db::archive_asset(&pool, &code)
        .await?
        .ok_or(Error::InUse("asset"))?;

    Ok(Json(asset))
}
