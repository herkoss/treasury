use std::collections::{BTreeMap, BTreeSet};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use time::{Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    db,
    domain::{self, Amount, Decision, Leg, Outcome, State as ProposalState},
    error::{Error, Problem, Result},
    models::{self, Account, ApprovalRow, LegRow, ProposalRow},
};

use super::{
    account_ref,
    accounts::authorize,
    idempotency::{self, Claim, Claimed},
    parse_account, Actor,
};

const DEFAULT_TTL_SECS: i64 = 900;
const MAX_TTL_SECS: i64 = 86_400;

#[derive(Deserialize, ToSchema)]
pub struct CreateProposal {
    #[serde(default)]
    pub memo: String,
    pub expires_in_secs: Option<i64>,
    pub legs: Vec<LegInput>,
}

#[derive(Deserialize, ToSchema)]
pub struct LegInput {
    pub from: String,
    pub to: String,
    pub asset: String,
    pub amount: String,
}

#[derive(Serialize, ToSchema)]
pub struct ProposalView {
    pub id: Uuid,
    pub state: String,
    pub memo: String,
    pub proposer: String,

    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339::option")]
    pub closed_at: Option<OffsetDateTime>,

    pub legs: Vec<LegView>,
    pub required_approvals: Vec<ApprovalView>,
}

#[derive(Serialize, ToSchema)]
pub struct LegView {
    pub from: String,
    pub to: String,
    pub asset: String,
    pub amount: String,
}

#[derive(Serialize, ToSchema)]
pub struct ApprovalView {
    pub account: String,
    pub decision: String,

    #[serde(with = "time::serde::rfc3339::option")]
    pub at: Option<OffsetDateTime>,
}

#[utoipa::path(
    post,
    path = "/transactions",
    tag = "transactions",
    request_body = CreateProposal,
    responses(
        (status = 201, body = ProposalView, description = "settled at once"),
        (status = 202, body = ProposalView, description = "waiting for approvals"),
        (status = 404, body = Problem, description = "unknown asset or account"),
        (status = 422, body = Problem, description = "bad legs, bad amount or insufficient balance"),
    ),
    security(("account_key" = []))
)]
pub async fn create(
    Actor(actor): Actor,
    Claimed(claim): Claimed,
    State(pool): State<PgPool>,
    Json(input): Json<CreateProposal>,
) -> Result<(StatusCode, Json<ProposalView>)> {
    let ttl = input
        .expires_in_secs
        .unwrap_or(DEFAULT_TTL_SECS)
        .clamp(1, MAX_TTL_SECS);
    let codes: Vec<String> = input.legs.iter().map(|leg| leg.asset.clone()).collect();
    let scales: BTreeMap<String, i16> = db::select_scales(&pool, &codes)
        .await?
        .into_iter()
        .collect();

    let legs = input
        .legs
        .iter()
        .map(|leg| parse_leg(leg, &scales))
        .collect::<Result<Vec<Leg>>>()?;
    domain::validate_legs(&legs)?;
    let required = domain::required_approvals(&legs);
    let debits = domain::debit(&legs)?;
    let accounts = account_ids(&legs);

    let mut tx = pool.begin().await?;
    let locked = db::lock_accounts(&mut tx, &accounts).await?;
    if locked.len() != accounts.len() {
        return Err(Error::NotFound("account"));
    }

    let id = Uuid::new_v4();
    let expires_at = OffsetDateTime::now_utc() + Duration::seconds(ttl);
    db::insert_proposal(&mut tx, id, actor.id, &input.memo, expires_at).await?;
    for (position, leg) in (0_i16..).zip(&legs) {
        db::insert_leg(&mut tx, id, position, leg).await?;
    }

    let settled = if required.contains(&actor.id) {
        ensure_available(&mut tx, &actor, &debits).await?;
        db::insert_approval(&mut tx, id, actor.id, Decision::Approved).await?;
        required.len() == 1
    } else {
        false
    };
    if settled {
        db::settle(&mut tx, id).await?;
    }

    let view = load_view(&mut tx, id)
        .await?
        .ok_or(Error::NotFound("proposal"))?;
    let status = if settled {
        StatusCode::CREATED
    } else {
        StatusCode::ACCEPTED
    };
    idempotency::commit(tx, claim, status, &view).await?;

    Ok((status, Json(view)))
}

#[utoipa::path(
    get,
    path = "/transactions/{id}",
    tag = "transactions",
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, body = ProposalView),
        (status = 403, body = Problem, description = "not a party"),
        (status = 404, body = Problem),
    ),
    security(("account_key" = []))
)]
pub async fn read(
    Actor(actor): Actor,
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Result<Json<ProposalView>> {
    let mut conn = pool.acquire().await?;
    let view = load_view(&mut conn, id)
        .await?
        .ok_or(Error::NotFound("proposal"))?;

    let party = view.proposer == account_ref(actor.id)
        || view
            .legs
            .iter()
            .any(|leg| leg.from == account_ref(actor.id) || leg.to == account_ref(actor.id));
    if !party && !actor.is_issuer {
        return Err(Error::Forbidden);
    }

    Ok(Json(view))
}

#[utoipa::path(
    post,
    path = "/transactions/{id}/approve",
    tag = "transactions",
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, body = ProposalView),
        (status = 403, body = Problem, description = "not a debited party"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "already decided or no longer pending"),
        (status = 422, body = Problem, description = "insufficient available balance"),
    ),
    security(("account_key" = []))
)]
pub async fn approve(
    Actor(actor): Actor,
    Claimed(claim): Claimed,
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Result<Json<ProposalView>> {
    vote(&pool, &actor, claim, id, Decision::Approved).await
}

#[utoipa::path(
    post,
    path = "/transactions/{id}/reject",
    tag = "transactions",
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, body = ProposalView),
        (status = 403, body = Problem, description = "not a debited party"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "already decided or no longer pending"),
    ),
    security(("account_key" = []))
)]
pub async fn reject(
    Actor(actor): Actor,
    Claimed(claim): Claimed,
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Result<Json<ProposalView>> {
    vote(&pool, &actor, claim, id, Decision::Rejected).await
}

#[utoipa::path(
    post,
    path = "/transactions/{id}/cancel",
    tag = "transactions",
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, body = ProposalView),
        (status = 403, body = Problem, description = "not the proposer"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "no longer pending"),
    ),
    security(("account_key" = []))
)]
pub async fn cancel(
    Actor(actor): Actor,
    Claimed(claim): Claimed,
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Result<Json<ProposalView>> {
    let mut tx = pool.begin().await?;
    let row = db::lock_proposal(&mut tx, id)
        .await?
        .ok_or(Error::NotFound("proposal"))?;
    let legs = to_legs(&db::select_legs(&mut tx, id).await?)?;
    let approvals = db::select_approvals(&mut tx, id).await?;

    facts(&row, &legs, &approvals)?.cancel(actor.id)?;
    db::close_proposal(&mut tx, id, ProposalState::Cancelled).await?;

    let view = load_view(&mut tx, id)
        .await?
        .ok_or(Error::NotFound("proposal"))?;
    idempotency::commit(tx, claim, StatusCode::OK, &view).await?;

    Ok(Json(view))
}

#[utoipa::path(
    get,
    path = "/accounts/{id}/inbox",
    tag = "transactions",
    params(("id" = String, Path, description = "account UUID or @issuer")),
    responses((status = 200, body = Vec<ProposalView>), (status = 403, body = Problem)),
    security(("account_key" = []))
)]
pub async fn inbox(
    Actor(actor): Actor,
    State(pool): State<PgPool>,
    Path(account): Path<String>,
) -> Result<Json<Vec<ProposalView>>> {
    let id = authorize(&actor, &account)?;
    let ids = db::select_inbox(&pool, id).await?;

    load_views(&pool, &ids).await.map(Json)
}

#[utoipa::path(
    get,
    path = "/accounts/{id}/outbox",
    tag = "transactions",
    params(("id" = String, Path, description = "account UUID or @issuer")),
    responses((status = 200, body = Vec<ProposalView>), (status = 403, body = Problem)),
    security(("account_key" = []))
)]
pub async fn outbox(
    Actor(actor): Actor,
    State(pool): State<PgPool>,
    Path(account): Path<String>,
) -> Result<Json<Vec<ProposalView>>> {
    let id = authorize(&actor, &account)?;
    let ids = db::select_outbox(&pool, id).await?;

    load_views(&pool, &ids).await.map(Json)
}

async fn vote(
    pool: &PgPool,
    actor: &Account,
    claim: Option<Claim>,
    id: Uuid,
    decision: Decision,
) -> Result<Json<ProposalView>> {
    let mut tx = pool.begin().await?;
    let row = db::lock_proposal(&mut tx, id)
        .await?
        .ok_or(Error::NotFound("proposal"))?;
    let legs = to_legs(&db::select_legs(&mut tx, id).await?)?;
    let approvals = db::select_approvals(&mut tx, id).await?;

    let outcome = facts(&row, &legs, &approvals)?.vote(actor.id, decision)?;
    db::lock_accounts(&mut tx, &account_ids(&legs)).await?;

    match outcome {
        Outcome::Rejected => {
            db::insert_approval(&mut tx, id, actor.id, Decision::Rejected).await?;
            db::close_proposal(&mut tx, id, ProposalState::Rejected).await?;
        }
        Outcome::Pending => {
            ensure_available(&mut tx, actor, &domain::debit(&legs)?).await?;
            db::insert_approval(&mut tx, id, actor.id, Decision::Approved).await?;
        }
        Outcome::Settled => {
            ensure_available(&mut tx, actor, &domain::debit(&legs)?).await?;
            db::insert_approval(&mut tx, id, actor.id, Decision::Approved).await?;
            db::settle(&mut tx, id).await?;
        }
    }

    let view = load_view(&mut tx, id)
        .await?
        .ok_or(Error::NotFound("proposal"))?;
    idempotency::commit(tx, claim, StatusCode::OK, &view).await?;

    Ok(Json(view))
}

fn parse_leg(input: &LegInput, scales: &BTreeMap<String, i16>) -> Result<Leg> {
    let scale = scales.get(&input.asset).ok_or(Error::NotFound("asset"))?;

    Ok(Leg {
        from: parse_account(&input.from)?,
        to: parse_account(&input.to)?,
        asset: input.asset.clone(),
        amount: Amount::parse(&input.amount, models::scale(*scale))?,
    })
}

fn to_legs(rows: &[LegRow]) -> Result<Vec<Leg>> {
    rows.iter()
        .map(|row| {
            Ok(Leg {
                from: row.from_id,
                to: row.to_id,
                asset: row.asset_code.clone(),
                amount: Amount::from_minor_units(row.amount)?,
            })
        })
        .collect()
}

fn account_ids(legs: &[Leg]) -> Vec<Uuid> {
    legs.iter()
        .flat_map(|leg| [leg.from, leg.to])
        .collect::<BTreeSet<Uuid>>()
        .into_iter()
        .collect()
}

fn facts(row: &ProposalRow, legs: &[Leg], approvals: &[ApprovalRow]) -> Result<domain::Proposal> {
    Ok(domain::Proposal {
        state: row.state.parse()?,
        proposer: row.proposer_id,
        required: domain::required_approvals(legs),
        approved: approvals
            .iter()
            .filter(|approval| approval.decision == Decision::Approved.as_str())
            .map(|approval| approval.account_id)
            .collect(),
    })
}

async fn ensure_available(
    conn: &mut PgConnection,
    actor: &Account,
    debits: &BTreeMap<(Uuid, String), Amount>,
) -> Result<()> {
    if actor.is_issuer {
        return Ok(());
    }
    for ((account, asset), amount) in debits {
        if *account != actor.id {
            continue;
        }
        if db::available(conn, *account, asset).await? < amount.minor_units() {
            return Err(Error::Insufficient(asset.clone()));
        }
    }
    Ok(())
}

async fn load_views(pool: &PgPool, ids: &[Uuid]) -> Result<Vec<ProposalView>> {
    let mut conn = pool.acquire().await?;
    let mut views = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(view) = load_view(&mut conn, *id).await? {
            views.push(view);
        }
    }
    Ok(views)
}

async fn load_view(conn: &mut PgConnection, id: Uuid) -> Result<Option<ProposalView>> {
    let Some(row) = db::select_proposal(conn, id).await? else {
        return Ok(None);
    };
    let legs = db::select_legs(conn, id).await?;
    let approvals = db::select_approvals(conn, id).await?;

    let required: BTreeSet<Uuid> = legs.iter().map(|leg| leg.from_id).collect();
    let required_approvals = required
        .into_iter()
        .map(|account| {
            let approval = approvals.iter().find(|a| a.account_id == account);
            ApprovalView {
                account: account_ref(account),
                decision: approval.map_or("pending".to_string(), |a| a.decision.clone()),
                at: approval.map(|a| a.decided_at),
            }
        })
        .collect();

    Ok(Some(ProposalView {
        id: row.id,
        state: row.state,
        memo: row.memo,
        proposer: account_ref(row.proposer_id),
        expires_at: row.expires_at,
        created_at: row.created_at,
        closed_at: row.closed_at,
        legs: legs
            .iter()
            .map(|leg| LegView {
                from: account_ref(leg.from_id),
                to: account_ref(leg.to_id),
                asset: leg.asset_code.clone(),
                amount: domain::format_units(leg.amount, models::scale(leg.scale)),
            })
            .collect(),
        required_approvals,
    }))
}
