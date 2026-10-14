use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header::CONTENT_TYPE, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{db, error::Error, utils};

const MAX_BODY: usize = 1 << 20;

pub async fn layer(State(pool): State<PgPool>, request: Request, next: Next) -> Response {
    match handle(pool, request, next).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn handle(pool: PgPool, request: Request, next: Next) -> Result<Response, Error> {
    let key = header(request.headers(), "idempotency-key");
    let account_key = header(request.headers(), "x-account-key");
    let (Some(key), Some(account_key)) = (key, account_key) else {
        return Ok(next.run(request).await);
    };
    if request.method() == Method::GET {
        return Ok(next.run(request).await);
    }
    let Some(account) =
        db::select_account_by_key_hash(&pool, &utils::key_hash(&account_key)).await?
    else {
        return Ok(next.run(request).await);
    };

    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, MAX_BODY)
        .await
        .map_err(|_| Error::BadRequest("request body is unreadable or too large"))?;
    let fingerprint = utils::fingerprint(parts.method.as_str(), parts.uri.path(), &bytes);

    if !db::claim_idempotency_key(&pool, account.id, &key, &fingerprint).await? {
        return replay(&pool, account.id, &key, &fingerprint).await;
    }

    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    if !response.status().is_success() {
        db::release_idempotency_key(&pool, account.id, &key).await?;
        return Ok(response);
    }

    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body, MAX_BODY)
        .await
        .map_err(|_| Error::BadRequest("response body is too large to remember"))?;
    db::complete_idempotency_key(
        &pool,
        account.id,
        &key,
        i32::from(parts.status.as_u16()),
        &bytes,
    )
    .await?;

    Ok(Response::from_parts(parts, Body::from(bytes)))
}

async fn replay(
    pool: &PgPool,
    account: Uuid,
    key: &str,
    fingerprint: &[u8],
) -> Result<Response, Error> {
    let row = db::select_idempotency_key(pool, account, key)
        .await?
        .ok_or(Error::InProgress)?;
    if row.fingerprint != fingerprint {
        return Err(Error::KeyReused);
    }
    let (Some(status), Some(body)) = (row.status, row.body) else {
        return Err(Error::InProgress);
    };
    let status = u16::try_from(status)
        .ok()
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or(StatusCode::OK);

    Ok((status, [(CONTENT_TYPE, "application/json")], body).into_response())
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}
