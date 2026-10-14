use axum::{
    http::{header::CONTENT_TYPE, StatusCode},
    response::IntoResponse,
    response::Response,
    Json,
};
use serde::Serialize;

use crate::domain::{AmountError, LegsError, ProposalError, UnknownValue};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid amount {0}")]
    Amount(#[from] AmountError),
    #[error("invalid legs {0}")]
    Legs(#[from] LegsError),
    #[error("invalid proposal {0}")]
    Proposal(#[from] ProposalError),
    #[error("unknown value {0}")]
    Unknown(#[from] UnknownValue),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("missing or invalid token")]
    Unauthorized,
    #[error("access denied")]
    Forbidden,
    #[error("{0} already exists")]
    AlreadyExists(&'static str),
    #[error("insufficient available balance of {0}")]
    Insufficient(String),
    #[error("{0} is in use or already archived")]
    InUse(&'static str),
    #[error("{0} is invalid")]
    InvalidAsset(&'static str),
}

impl Error {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Amount(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Legs(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Proposal(e) => match e {
                ProposalError::NotPending(_) => StatusCode::CONFLICT,
                ProposalError::AlreadyDecided => StatusCode::CONFLICT,
                ProposalError::WrongActor => StatusCode::FORBIDDEN,
            },
            Self::Unknown(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::AlreadyExists(_) => StatusCode::CONFLICT,
            Self::Insufficient(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::InUse(_) => StatusCode::CONFLICT,
            Self::InvalidAsset(_) => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status();

        let detail = if status.is_server_error() {
            tracing::error!(error = ?self, "request failed");

            "internal error".into()
        } else {
            tracing::warn!(error = %self, "request failed");

            self.to_string()
        };

        (
            status,
            [(CONTENT_TYPE, "application/problem+json")],
            Json(Problem {
                type_: "about:blank",
                title: status.canonical_reason().unwrap_or("Error"),
                status: status.as_u16(),
                detail,
            }),
        )
            .into_response()
    }
}

#[derive(Serialize)]
struct Problem {
    #[serde(rename = "type")]
    type_: &'static str,
    title: &'static str,
    status: u16,
    detail: String,
}
