use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Serialize)]
pub struct Asset {
    pub code: String,
    pub name: String,
    pub scale: i16,

    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339::option")]
    pub archived_at: Option<OffsetDateTime>,
}

#[derive(Serialize)]
pub struct Account {
    pub id: Uuid,
    pub name: String,
    pub is_issuer: bool,

    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

pub struct ProposalRow {
    pub id: Uuid,
    pub proposer_id: Uuid,
    pub memo: String,
    pub state: String,
    pub expires_at: OffsetDateTime,
    pub created_at: OffsetDateTime,
    pub closed_at: Option<OffsetDateTime>,
}

pub struct LegRow {
    pub from_id: Uuid,
    pub to_id: Uuid,
    pub asset_code: String,
    pub amount: i64,
    pub scale: i16,
}

pub struct ApprovalRow {
    pub account_id: Uuid,
    pub decision: String,
    pub decided_at: OffsetDateTime,
}

pub struct BalanceRow {
    pub asset: String,
    pub scale: i16,
    pub posted: i64,
    pub on_hold: i64,
}

pub struct EntryRow {
    pub id: i64,
    pub proposal_id: Uuid,
    pub asset_code: String,
    pub amount: i64,
    pub scale: i16,
    pub created_at: OffsetDateTime,
}

pub fn scale(value: i16) -> u8 {
    u8::try_from(value).expect("asset scale is checked by the schema")
}

pub struct IdempotencyRow {
    pub fingerprint: Vec<u8>,
    pub status: Option<i32>,
    pub body: Option<Vec<u8>>,
}
