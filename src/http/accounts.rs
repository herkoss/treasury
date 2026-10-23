use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    db,
    domain::format_units,
    error::{Error, Problem, Result},
    models::{self, Account},
    utils, welcome,
};

use super::{
    idempotency::{self, Claimed},
    parse_account, Actor, AppState,
};

const DEFAULT_PAGE: i64 = 50;
const MAX_PAGE: i64 = 200;
const MAX_NAME_CHARS: usize = 32;

#[derive(Deserialize, ToSchema)]
pub struct CreateAccount {
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct CreatedAccount {
    pub id: Uuid,
    pub name: String,
    pub key: String,
}

#[derive(Serialize, ToSchema)]
pub struct AccountView {
    pub id: Uuid,
    pub name: String,

    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Serialize, ToSchema)]
pub struct Balance {
    pub asset: String,
    pub posted: String,
    pub on_hold: String,
    pub available: String,
}

#[derive(Deserialize, IntoParams)]
pub struct Page {
    #[serde(default)]
    pub after: i64,
    pub limit: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct Entry {
    pub id: i64,
    pub proposal_id: Uuid,
    pub asset: String,
    pub amount: String,

    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

fn validate_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Name("must not be empty"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(Error::Name("must be at most 32 characters"));
    }
    if name.starts_with('@') {
        return Err(Error::Name("must not start with @"));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::Name("must not contain control characters"));
    }
    Ok(name)
}

#[utoipa::path(
    post,
    path = "/accounts",
    tag = "accounts",
    request_body = CreateAccount,
    responses(
        (status = 201, body = CreatedAccount, description = "the key is returned only here"),
        (status = 409, body = Problem, description = "the name is taken"),
        (status = 422, body = Problem, description = "invalid name"),
    ),
    security((), ("account_key" = []))
)]
pub async fn create(
    Claimed(claim): Claimed,
    State(state): State<AppState>,
    Json(input): Json<CreateAccount>,
) -> Result<(StatusCode, Json<CreatedAccount>)> {
    let name = validate_name(&input.name)?;

    let key = hex::encode(rand::random::<[u8; 32]>());
    let mut tx = state.pool.begin().await?;
    let account = db::insert_account(&mut tx, Uuid::new_v4(), name, &utils::key_hash(&key)).await?;
    welcome::apply(&mut tx, &state.welcome, account.id).await?;

    let created = CreatedAccount {
        id: account.id,
        name: account.name,
        key,
    };
    idempotency::commit(tx, claim, StatusCode::CREATED, &created).await?;

    Ok((StatusCode::CREATED, Json(created)))
}

#[utoipa::path(
    get,
    path = "/accounts",
    tag = "accounts",
    responses((status = 200, body = Vec<AccountView>), (status = 401, body = Problem)),
    security(("account_key" = []))
)]
pub async fn list(
    Actor(_actor): Actor,
    State(pool): State<PgPool>,
) -> Result<Json<Vec<AccountView>>> {
    let accounts = db::select_accounts(&pool)
        .await?
        .into_iter()
        .map(|account| AccountView {
            id: account.id,
            name: account.name,
            created_at: account.created_at,
        })
        .collect();

    Ok(Json(accounts))
}

pub fn authorize(actor: &Account, account: &str) -> Result<Uuid> {
    let id = parse_account(account)?;
    if actor.id != id && !actor.is_issuer {
        return Err(Error::Forbidden);
    }
    Ok(id)
}

#[utoipa::path(
    get,
    path = "/accounts/{id}/balances",
    tag = "accounts",
    params(("id" = String, Path, description = "account UUID or @issuer")),
    responses((status = 200, body = Vec<Balance>), (status = 403, body = Problem)),
    security(("account_key" = []))
)]
pub async fn balances(
    Actor(actor): Actor,
    State(pool): State<PgPool>,
    Path(account): Path<String>,
) -> Result<Json<Vec<Balance>>> {
    let id = authorize(&actor, &account)?;

    let balances = db::select_balances(&pool, id)
        .await?
        .into_iter()
        .map(|row| {
            let scale = models::scale(row.scale);
            Balance {
                asset: row.asset,
                posted: format_units(row.posted, scale),
                on_hold: format_units(row.on_hold, scale),
                available: format_units(row.posted - row.on_hold, scale),
            }
        })
        .collect();

    Ok(Json(balances))
}

#[utoipa::path(
    get,
    path = "/accounts/{id}/entries",
    tag = "accounts",
    params(("id" = String, Path, description = "account UUID or @issuer"), Page),
    responses((status = 200, body = Vec<Entry>), (status = 403, body = Problem)),
    security(("account_key" = []))
)]
pub async fn entries(
    Actor(actor): Actor,
    State(pool): State<PgPool>,
    Path(account): Path<String>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<Entry>>> {
    let id = authorize(&actor, &account)?;
    let limit = page.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);

    let entries = db::select_entries(&pool, id, page.after, limit)
        .await?
        .into_iter()
        .map(|row| Entry {
            id: row.id,
            proposal_id: row.proposal_id,
            amount: format_units(row.amount, models::scale(row.scale)),
            asset: row.asset_code,
            created_at: row.created_at,
        })
        .collect();

    Ok(Json(entries))
}
