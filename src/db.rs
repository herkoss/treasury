use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

use crate::{
    error::{Error, Result as TreasuryResult},
    http::CreateAsset,
    models::{Account, Asset},
};

pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new().connect(url).await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}

pub async fn ensure_issuer(pool: &PgPool, hash: &[u8]) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO accounts (id, name, key_hash, is_issuer)
        VALUES ($1, '@issuer', $2, true)
        ON CONFLICT (id) DO UPDATE SET key_hash = EXCLUDED.key_hash
        "#,
        Uuid::nil(),
        hash
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn select_account_by_key_hash(
    pool: &PgPool,
    hash: &[u8],
) -> Result<Option<Account>, sqlx::Error> {
    let v = sqlx::query_as!(
        Account,
        r#"
        SELECT id, name, is_issuer, created_at
        FROM accounts
        WHERE key_hash = $1
        "#,
        hash
    )
    .fetch_optional(pool)
    .await?;

    Ok(v)
}

pub async fn insert_asset(pool: &PgPool, asset: CreateAsset) -> TreasuryResult<Asset> {
    sqlx::query_as!(
        Asset,
        r#"
        INSERT INTO assets (code, name, scale)
        VALUES ($1, $2, $3)
        RETURNING code, name, scale, archived_at, created_at
        "#,
        asset.code,
        asset.name,
        i16::from(asset.scale),
    )
    .fetch_one(pool)
    .await
    .map_err(|err| match err {
        sqlx::Error::Database(e) if e.is_unique_violation() => Error::AlreadyExists("asset"),
        other => Error::Db(other),
    })
}
