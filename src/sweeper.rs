use std::time::Duration;

use sqlx::PgPool;

pub async fn run(pool: PgPool, every: Duration) {
    let mut ticks = tokio::time::interval(every);
    loop {
        ticks.tick().await;
        match crate::db::expire_due(&pool).await {
            Ok(0) => {}
            Ok(expired) => tracing::info!(expired, "proposals expired"),
            Err(error) => tracing::error!(error = ?error, "sweep failed"),
        }
    }
}
