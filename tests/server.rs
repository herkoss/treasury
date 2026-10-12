use reqwest::Client;
use sqlx::PgPool;
use tokio::net::TcpListener;

struct TestApp {
    addr: String,
    client: Client,
}

impl TestApp {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.addr)
    }
}

async fn spawn_app(pool: PgPool) -> TestApp {
    let state = treasury::http::AppState { pool };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { treasury::http::serve(listener, state).await.unwrap() });

    let client = Client::new();

    TestApp { addr, client }
}

#[sqlx::test]
async fn health_check(pool: PgPool) {
    let app = spawn_app(pool).await;

    let response = app.client.get(app.url("/health")).send().await.unwrap();

    assert_eq!(response.status(), 200);
}
