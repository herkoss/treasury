use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Serialize)]
pub struct Asset {
    pub code: String,
    pub name: String,
    pub scale: i16,
    pub archived_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

#[derive(Serialize)]
pub struct Account {
    pub id: Uuid,
    pub name: String,
    pub is_issuer: bool,
    pub created_at: OffsetDateTime,
}
