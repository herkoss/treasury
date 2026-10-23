//! The welcome grant: what a new account receives at registration, configured as
//! `WELCOME_GRANT=USD=1000,XAU=10`. It is recorded as an ordinary settled deposit
//! from `@issuer`, so it sits in the journal like any other movement.

use std::collections::BTreeMap;

use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    db,
    domain::{Amount, Decision, Leg},
    error::{Error, Result},
    models,
};

pub const MEMO: &str = "welcome";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub asset: String,
    pub amount: String,
}

/// Parses `USD=1000,XAU=10`. An empty string is no grant at all.
pub fn parse(text: &str) -> std::result::Result<Vec<Grant>, String> {
    let mut grants: Vec<Grant> = Vec::new();
    for item in text
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (asset, amount) = item
            .split_once('=')
            .map(|(asset, amount)| (asset.trim(), amount.trim()))
            .filter(|(asset, amount)| !asset.is_empty() && !amount.is_empty())
            .ok_or_else(|| format!("expected ASSET=AMOUNT, got {item:?}"))?;
        Amount::parse(amount, 9).map_err(|err| format!("{asset}: {err}"))?;
        if grants.iter().any(|grant| grant.asset == asset) {
            return Err(format!("{asset} is listed twice"));
        }
        grants.push(Grant {
            asset: asset.to_string(),
            amount: amount.to_string(),
        });
    }
    Ok(grants)
}

/// Deposits the grant to `account` inside the caller's transaction and returns the
/// proposal id, or `None` when nothing is configured.
pub async fn apply(
    conn: &mut PgConnection,
    grants: &[Grant],
    account: Uuid,
) -> Result<Option<Uuid>> {
    if grants.is_empty() {
        return Ok(None);
    }

    let codes: Vec<String> = grants.iter().map(|grant| grant.asset.clone()).collect();
    let scales: BTreeMap<String, i16> = db::select_scales(&mut *conn, &codes)
        .await?
        .into_iter()
        .collect();
    let mut legs = Vec::with_capacity(grants.len());
    for grant in grants {
        let scale = scales.get(&grant.asset).ok_or_else(|| {
            Error::Misconfigured(format!(
                "welcome grant asset {} does not exist or is archived",
                grant.asset
            ))
        })?;
        let amount = Amount::parse(&grant.amount, models::scale(*scale)).map_err(|err| {
            Error::Misconfigured(format!("welcome grant of {}: {err}", grant.asset))
        })?;
        legs.push(Leg {
            from: Uuid::nil(),
            to: account,
            asset: grant.asset.clone(),
            amount,
        });
    }

    let id = Uuid::new_v4();
    let expires_at = OffsetDateTime::now_utc() + Duration::minutes(15);
    db::insert_proposal(conn, id, Uuid::nil(), MEMO, expires_at).await?;
    for (position, leg) in (0_i16..).zip(&legs) {
        db::insert_leg(conn, id, position, leg).await?;
    }
    db::insert_approval(conn, id, Uuid::nil(), Decision::Approved).await?;
    db::settle(conn, id).await?;

    Ok(Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(asset: &str, amount: &str) -> Grant {
        Grant {
            asset: asset.into(),
            amount: amount.into(),
        }
    }

    #[test]
    fn parses_a_list() {
        assert_eq!(parse("").unwrap(), vec![]);
        assert_eq!(parse(" , ").unwrap(), vec![]);
        assert_eq!(
            parse("USD=1000, XAU = 10.5").unwrap(),
            vec![grant("USD", "1000"), grant("XAU", "10.5")]
        );
    }

    #[test]
    fn rejects_bad_items() {
        assert!(parse("USD").unwrap_err().contains("ASSET=AMOUNT"));
        assert!(parse("USD=").unwrap_err().contains("ASSET=AMOUNT"));
        assert!(parse("=5").unwrap_err().contains("ASSET=AMOUNT"));
        assert!(parse("USD=0").unwrap_err().contains("greater than zero"));
        assert!(parse("USD=ten").unwrap_err().contains("digits"));
        assert!(parse("USD=1,USD=2").unwrap_err().contains("twice"));
    }
}
