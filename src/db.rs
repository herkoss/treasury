use sqlx::{postgres::PgPoolOptions, PgPool};

pub async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new().connect(url).await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}
