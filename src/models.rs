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
    pub created_at: OffsetDateTime,
}
