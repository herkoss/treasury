use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

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
