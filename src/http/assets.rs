use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use sqlx::PgPool;

use crate::{
    db,
    domain::validate_asset,
    error::{Error, Result},
    models::Asset,
};

use super::Actor;

#[derive(Deserialize)]
pub struct CreateAsset {
    pub code: String,
    pub name: String,
    pub scale: u8,
}

#[derive(Deserialize)]
pub struct UpdateAsset {
    pub name: String,
}

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

pub async fn list(Actor(_actor): Actor, State(pool): State<PgPool>) -> Result<Json<Vec<Asset>>> {
    let assets = db::select_assets(&pool).await?;

    Ok(Json(assets))
}

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
