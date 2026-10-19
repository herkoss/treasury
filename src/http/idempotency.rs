use std::convert::Infallible;

use axum::{
    body::{to_bytes, Body},
    extract::{FromRequestParts, Request, State},
    http::{header::CONTENT_TYPE, request::Parts, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{db, error::Error, utils};

const MAX_BODY: usize = 1 << 20;

#[derive(Clone)]
pub struct Claim {
    account: Uuid,
    key: String,
    token: Uuid,
}

pub struct Claimed(pub Option<Claim>);

impl<S: Send + Sync> FromRequestParts<S> for Claimed {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Infallible> {
        Ok(Self(parts.extensions.get::<Claim>().cloned()))
    }
}

pub async fn commit<T: Serialize>(
    mut tx: Transaction<'_, Postgres>,
    claim: Option<Claim>,
    status: StatusCode,
    body: &T,
) -> Result<(), Error> {
    if let Some(claim) = claim {
        let stored = db::complete_idempotency_key(
            &mut tx,
            claim.account,
            &claim.key,
            claim.token,
            i32::from(status.as_u16()),
            &serde_json::to_vec(body)?,
        )
        .await?;
        if !stored {
            return Err(Error::InProgress);
        }
    }
    tx.commit().await?;

    Ok(())
}

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

    let claim = Claim {
        account: account.id,
        key,
        token: Uuid::new_v4(),
    };
    if !db::claim_idempotency_key(&pool, claim.account, &claim.key, &fingerprint, claim.token)
        .await?
    {
        return replay(&pool, claim.account, &claim.key, &fingerprint).await;
    }

    let mut request = Request::from_parts(parts, Body::from(bytes));
    request.extensions_mut().insert(claim.clone());
    let response = next.run(request).await;
    if !response.status().is_success() {
        db::release_idempotency_key(&pool, claim.account, &claim.key, claim.token).await?;
    }

    Ok(response)
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
