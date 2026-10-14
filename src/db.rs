use sqlx::{postgres::PgPoolOptions, PgConnection, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    domain::{Decision, Leg, State},
    error::{Error, Result as TreasuryResult},
    models::{Account, ApprovalRow, Asset, BalanceRow, EntryRow, LegRow, ProposalRow},
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
    sqlx::query_as!(
        Account,
        r#"
        SELECT id, name, is_issuer, created_at
        FROM accounts
        WHERE key_hash = $1
        "#,
        hash
    )
    .fetch_optional(pool)
    .await
}

pub async fn insert_account(
    pool: &PgPool,
    id: Uuid,
    name: &str,
    hash: &[u8],
) -> Result<Account, sqlx::Error> {
    sqlx::query_as!(
        Account,
        r#"
        INSERT INTO accounts (id, name, key_hash)
        VALUES ($1, $2, $3)
        RETURNING id, name, is_issuer, created_at
        "#,
        id,
        name,
        hash
    )
    .fetch_one(pool)
    .await
}

pub async fn insert_asset(
    pool: &PgPool,
    code: &str,
    name: &str,
    scale: i16,
) -> TreasuryResult<Asset> {
    sqlx::query_as!(
        Asset,
        r#"
        INSERT INTO assets (code, name, scale)
        VALUES ($1, $2, $3)
        RETURNING code, name, scale, archived_at, created_at
        "#,
        code,
        name,
        scale,
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

pub async fn select_scales(
    pool: &PgPool,
    codes: &[String],
) -> Result<Vec<(String, i16)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT code, scale
        FROM assets
        WHERE code = ANY($1) AND archived_at IS NULL
        "#,
        codes
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|row| (row.code, row.scale)).collect())
}

pub async fn lock_accounts(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT id
        FROM accounts
        WHERE id = ANY($1)
        ORDER BY id
        FOR UPDATE
        "#,
        ids
    )
    .fetch_all(conn)
    .await?;

    Ok(rows.into_iter().map(|row| row.id).collect())
}

pub async fn available(
    conn: &mut PgConnection,
    account: Uuid,
    asset: &str,
) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT
            COALESCE((
                SELECT SUM(amount)
                FROM entries
                WHERE account_id = $1 AND asset_code = $2
            ), 0)::BIGINT
            -
            COALESCE((
                SELECT SUM(l.amount)
                FROM legs l
                JOIN proposals p ON p.id = l.proposal_id
                JOIN approvals a ON a.proposal_id = l.proposal_id AND a.account_id = l.from_id
                WHERE l.from_id = $1
                  AND l.asset_code = $2
                  AND a.decision = 'approved'
                  AND p.state = 'pending'
                  AND p.expires_at > now()
            ), 0)::BIGINT
            AS "available!"
        "#,
        account,
        asset
    )
    .fetch_one(conn)
    .await?;

    Ok(row.available)
}

pub async fn insert_proposal(
    conn: &mut PgConnection,
    id: Uuid,
    proposer: Uuid,
    memo: &str,
    expires_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO proposals (id, proposer_id, memo, state, expires_at)
        VALUES ($1, $2, $3, 'pending', $4)
        "#,
        id,
        proposer,
        memo,
        expires_at
    )
    .execute(conn)
    .await?;

    Ok(())
}

pub async fn insert_leg(
    conn: &mut PgConnection,
    proposal: Uuid,
    position: i16,
    leg: &Leg,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO legs (proposal_id, position, from_id, to_id, asset_code, amount)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
        proposal,
        position,
        leg.from,
        leg.to,
        leg.asset,
        leg.amount.minor_units()
    )
    .execute(conn)
    .await?;

    Ok(())
}

pub async fn insert_approval(
    conn: &mut PgConnection,
    proposal: Uuid,
    account: Uuid,
    decision: Decision,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO approvals (proposal_id, account_id, decision)
        VALUES ($1, $2, $3)
        "#,
        proposal,
        account,
        decision.as_str()
    )
    .execute(conn)
    .await?;

    Ok(())
}

pub async fn select_proposal(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<ProposalRow>, sqlx::Error> {
    sqlx::query_as!(
        ProposalRow,
        r#"
        SELECT id, proposer_id, memo, expires_at, created_at, closed_at,
               CASE WHEN state = 'pending' AND expires_at <= now() THEN 'expired' ELSE state END
                   AS "state!"
        FROM proposals
        WHERE id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
}

pub async fn lock_proposal(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<ProposalRow>, sqlx::Error> {
    sqlx::query_as!(
        ProposalRow,
        r#"
        SELECT id, proposer_id, memo, expires_at, created_at, closed_at,
               CASE WHEN state = 'pending' AND expires_at <= now() THEN 'expired' ELSE state END
                   AS "state!"
        FROM proposals
        WHERE id = $1
        FOR UPDATE
        "#,
        id
    )
    .fetch_optional(conn)
    .await
}

pub async fn select_legs(
    conn: &mut PgConnection,
    proposal: Uuid,
) -> Result<Vec<LegRow>, sqlx::Error> {
    sqlx::query_as!(
        LegRow,
        r#"
        SELECT l.from_id, l.to_id, l.asset_code, l.amount, a.scale
        FROM legs l
        JOIN assets a ON a.code = l.asset_code
        WHERE l.proposal_id = $1
        ORDER BY l.position
        "#,
        proposal
    )
    .fetch_all(conn)
    .await
}

pub async fn select_approvals(
    conn: &mut PgConnection,
    proposal: Uuid,
) -> Result<Vec<ApprovalRow>, sqlx::Error> {
    sqlx::query_as!(
        ApprovalRow,
        r#"
        SELECT account_id, decision, decided_at
        FROM approvals
        WHERE proposal_id = $1
        ORDER BY decided_at
        "#,
        proposal
    )
    .fetch_all(conn)
    .await
}

pub async fn close_proposal(
    conn: &mut PgConnection,
    id: Uuid,
    state: State,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE proposals
        SET state = $2, closed_at = now()
        WHERE id = $1 AND state = 'pending'
        "#,
        id,
        state.as_str()
    )
    .execute(conn)
    .await?;

    Ok(())
}

pub async fn settle(conn: &mut PgConnection, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO entries (proposal_id, account_id, asset_code, amount)
        SELECT proposal_id, from_id, asset_code, -amount FROM legs WHERE proposal_id = $1
        UNION ALL
        SELECT proposal_id, to_id, asset_code, amount FROM legs WHERE proposal_id = $1
        "#,
        id
    )
    .execute(&mut *conn)
    .await?;

    close_proposal(conn, id, State::Settled).await
}

pub async fn select_balances(pool: &PgPool, account: Uuid) -> Result<Vec<BalanceRow>, sqlx::Error> {
    sqlx::query_as!(
        BalanceRow,
        r#"
        WITH posted AS (
            SELECT asset_code, SUM(amount) AS amount
            FROM entries
            WHERE account_id = $1
            GROUP BY asset_code
        ),
        held AS (
            SELECT l.asset_code, SUM(l.amount) AS amount
            FROM legs l
            JOIN proposals p ON p.id = l.proposal_id
            JOIN approvals a ON a.proposal_id = l.proposal_id AND a.account_id = l.from_id
            WHERE l.from_id = $1
              AND a.decision = 'approved'
              AND p.state = 'pending'
              AND p.expires_at > now()
            GROUP BY l.asset_code
        )
        SELECT a.code AS asset, a.scale,
               COALESCE(posted.amount, 0)::BIGINT AS "posted!",
               COALESCE(held.amount, 0)::BIGINT AS "on_hold!"
        FROM assets a
        LEFT JOIN posted ON posted.asset_code = a.code
        LEFT JOIN held ON held.asset_code = a.code
        WHERE posted.asset_code IS NOT NULL OR held.asset_code IS NOT NULL
        ORDER BY a.code
        "#,
        account
    )
    .fetch_all(pool)
    .await
}

pub async fn select_entries(
    pool: &PgPool,
    account: Uuid,
    after: i64,
    limit: i64,
) -> Result<Vec<EntryRow>, sqlx::Error> {
    sqlx::query_as!(
        EntryRow,
        r#"
        SELECT e.id, e.proposal_id, e.asset_code, e.amount, a.scale, e.created_at
        FROM entries e
        JOIN assets a ON a.code = e.asset_code
        WHERE e.account_id = $1 AND e.id > $2
        ORDER BY e.id
        LIMIT $3
        "#,
        account,
        after,
        limit
    )
    .fetch_all(pool)
    .await
}

pub async fn select_inbox(pool: &PgPool, account: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT p.id, p.created_at
        FROM proposals p
        JOIN legs l ON l.proposal_id = p.id
        WHERE l.from_id = $1
          AND p.state = 'pending'
          AND p.expires_at > now()
          AND NOT EXISTS (
              SELECT 1 FROM approvals a
              WHERE a.proposal_id = p.id AND a.account_id = $1
          )
        ORDER BY p.created_at
        "#,
        account
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|row| row.id).collect())
}

pub async fn select_outbox(pool: &PgPool, account: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT id
        FROM proposals
        WHERE proposer_id = $1
          AND state = 'pending'
          AND expires_at > now()
        ORDER BY created_at
        "#,
        account
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|row| row.id).collect())
}

pub async fn expire_due(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        r#"
        UPDATE proposals
        SET state = 'expired', closed_at = expires_at
        WHERE state = 'pending' AND expires_at <= now()
        "#
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}
