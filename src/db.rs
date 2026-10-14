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

pub async fn select_assets(pool: &PgPool) -> Result<Vec<Asset>, sqlx::Error> {
    sqlx::query_as!(
        Asset,
        r#"
        SELECT code, name, scale, archived_at, created_at
        FROM assets
        ORDER BY code
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn select_asset_by_code(pool: &PgPool, code: &str) -> Result<Option<Asset>, sqlx::Error> {
    sqlx::query_as!(
        Asset,
        r#"
        SELECT code, name, scale, archived_at, created_at
        FROM assets
        WHERE code = $1
        "#,
        code
    )
    .fetch_optional(pool)
    .await
}

pub async fn update_asset_name(
    pool: &PgPool,
    code: &str,
    name: &str,
) -> Result<Option<Asset>, sqlx::Error> {
    sqlx::query_as!(
        Asset,
        r#"
        UPDATE assets
        SET name = $2
        WHERE code = $1
        RETURNING code, name, scale, archived_at, created_at
        "#,
        code,
        name
    )
    .fetch_optional(pool)
    .await
}

pub async fn archive_asset(pool: &PgPool, code: &str) -> Result<Option<Asset>, sqlx::Error> {
    sqlx::query_as!(
        Asset,
        r#"
        UPDATE assets
        SET archived_at = now()
        WHERE code = $1
          AND archived_at IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM entries
              WHERE asset_code = $1
              GROUP BY account_id
              HAVING SUM(amount) <> 0
          )
          AND NOT EXISTS (
              SELECT 1 FROM legs l
              JOIN proposals p ON p.id = l.proposal_id
              WHERE l.asset_code = $1
                AND p.state = 'pending'
                AND p.expires_at > now()
          )
        RETURNING code, name, scale, archived_at, created_at
        "#,
        code,
    )
    .fetch_optional(pool)
    .await
}
