use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("amount must be digits with at most one dot")]
    InvalidFormat,
    #[error("fractional part exceeds the asset scale")]
    TooManyDecimalPlaces,
    #[error("amount must be greater than zero")]
    NotPositive,
    #[error("amount is too large")]
    Overflow,
    #[error("legs cannot be empty")]
    EmptyLegs,
    #[error("too many legs")]
    TooManyLegs,
    #[error("sender and receiver cannot be the same")]
    InvalidReceiver,
    #[error("unknown state")]
    UnknownState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Amount(i64);

impl Amount {
    pub fn parse(text: &str, scale: u8) -> Result<Self, DomainError> {
        let (int_part, frac_part) = match text.split_once('.') {
            Some((int_part, frac_part)) if !int_part.is_empty() && !frac_part.is_empty() => {
                (int_part, frac_part)
            }
            Some(_) => return Err(DomainError::InvalidFormat),
            None => (text, ""),
        };

        let is_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
        if int_part.is_empty() || !is_digits(int_part) || !is_digits(frac_part) {
            return Err(DomainError::InvalidFormat);
        }
        if frac_part.len() > usize::from(scale) {
            return Err(DomainError::TooManyDecimalPlaces);
        }

        let factor = 10_i64
            .checked_pow(u32::from(scale))
            .ok_or(DomainError::Overflow)?;
        let int_value: i64 = int_part.parse().map_err(|_| DomainError::Overflow)?;
        let frac_value = if frac_part.is_empty() {
            0
        } else {
            let digits: i64 = frac_part.parse().map_err(|_| DomainError::Overflow)?;
            digits * 10_i64.pow(u32::from(scale) - frac_part.len() as u32)
        };

        let total = int_value
            .checked_mul(factor)
            .and_then(|value| value.checked_add(frac_value))
            .ok_or(DomainError::Overflow)?;

        Self::from_minor_units(total)
    }

    pub fn from_minor_units(value: i64) -> Result<Self, DomainError> {
        if value <= 0 {
            return Err(DomainError::NotPositive);
        }
        Ok(Self(value))
    }

    pub fn minor_units(self) -> i64 {
        self.0
    }

    pub fn checked_add(self, other: Self) -> Result<Self, DomainError> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or(DomainError::Overflow)
    }

    pub fn format(self, scale: u8) -> String {
        if scale == 0 {
            return self.0.to_string();
        }
        let factor = 10_i64.pow(u32::from(scale));
        format!(
            "{}.{:0width$}",
            self.0 / factor,
            self.0 % factor,
            width = usize::from(scale)
        )
    }
}

pub struct Leg {
    pub from: Uuid,
    pub to: Uuid,
    pub asset: String,
    pub amount: Amount,
}

pub fn validate_legs(legs: &[Leg]) -> Result<(), DomainError> {
    if legs.is_empty() {
        return Err(DomainError::EmptyLegs);
    }

    if legs.len() > 16 {
        return Err(DomainError::TooManyLegs);
    }

    if legs.iter().any(|leg| leg.from == leg.to) {
        return Err(DomainError::InvalidReceiver);
    }

    Ok(())
}

pub fn required_approvals(legs: &[Leg]) -> BTreeSet<Uuid> {
    let mut res: BTreeSet<Uuid> = BTreeSet::new();

    for leg in legs {
        res.insert(leg.from);
    }

    res
}
pub fn debit(legs: &[Leg]) -> Result<BTreeMap<(Uuid, String), Amount>, DomainError> {
    let mut res: BTreeMap<(Uuid, String), Amount> = BTreeMap::new();

    for leg in legs {
        match res.entry((leg.from, leg.asset.clone())) {
            Entry::Occupied(mut slot) => {
                let total = slot.get().checked_add(leg.amount)?;
                slot.insert(total);
            }
            Entry::Vacant(slot) => {
                slot.insert(leg.amount);
            }
        }
    }

    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_with_scale() {
        assert_eq!(Amount::parse("10.50", 2), Ok(Amount(1050)));
        assert_eq!(Amount::parse("1.5", 3), Ok(Amount(1500)));
        assert_eq!(Amount::parse("7", 2), Ok(Amount(700)));
        assert_eq!(Amount::parse("3", 0), Ok(Amount(3)));
        assert_eq!(Amount::parse("0.01", 2), Ok(Amount(1)));
    }

    #[test]
    fn rejects_bad_input() {
        for text in [
            "", ".", "abc", "-1", "+1", "1.-5", "1,5", "1.2.3", ".5", "1.", " 1",
        ] {
            assert_eq!(
                Amount::parse(text, 2),
                Err(DomainError::InvalidFormat),
                "{text:?}"
            );
        }
        assert_eq!(
            Amount::parse("1.234", 2),
            Err(DomainError::TooManyDecimalPlaces)
        );
        assert_eq!(
            Amount::parse("5.1", 0),
            Err(DomainError::TooManyDecimalPlaces)
        );
        assert_eq!(Amount::parse("0", 2), Err(DomainError::NotPositive));
        assert_eq!(Amount::parse("0.00", 2), Err(DomainError::NotPositive));
        assert_eq!(
            Amount::parse("9223372036854775807", 2),
            Err(DomainError::Overflow)
        );
        assert_eq!(
            Amount::parse("99999999999999999999", 0),
            Err(DomainError::Overflow)
        );
    }

    #[test]
    fn formats_with_scale() {
        assert_eq!(Amount(1050).format(2), "10.50");
        assert_eq!(Amount(5).format(2), "0.05");
        assert_eq!(Amount(1500).format(3), "1.500");
        assert_eq!(Amount(42).format(0), "42");
    }

    #[test]
    fn format_round_trips() {
        for (text, scale) in [("100.00", 2), ("0.01", 2), ("1.500", 3), ("12", 0)] {
            assert_eq!(Amount::parse(text, scale).unwrap().format(scale), text);
        }
    }

    #[test]
    fn checked_add_detects_overflow() {
        assert_eq!(Amount(1).checked_add(Amount(2)), Ok(Amount(3)));
        assert_eq!(
            Amount(i64::MAX).checked_add(Amount(1)),
            Err(DomainError::Overflow)
        );
    }

    #[test]
    fn minor_units_must_be_positive() {
        assert_eq!(Amount::from_minor_units(0), Err(DomainError::NotPositive));
        assert_eq!(Amount::from_minor_units(-5), Err(DomainError::NotPositive));
        assert_eq!(Amount::from_minor_units(5).map(Amount::minor_units), Ok(5));
    }

    fn leg(from: Uuid, to: Uuid, asset: &str, amount: &str) -> Leg {
        Leg {
            from,
            to,
            asset: asset.into(),
            amount: Amount::parse(amount, 2).unwrap(),
        }
    }

    #[test]
    fn transfer_requires_only_the_sender() {
        let (alice, bob) = (Uuid::new_v4(), Uuid::new_v4());
        let legs = [leg(alice, bob, "USD", "12.50")];

        assert_eq!(required_approvals(&legs), BTreeSet::from([alice]));
    }

    #[test]
    fn swap_requires_both_sides() {
        let (alice, bob) = (Uuid::new_v4(), Uuid::new_v4());
        let legs = [
            leg(alice, bob, "USD", "12.50"),
            leg(bob, alice, "XAU", "1.50"),
        ];

        assert_eq!(required_approvals(&legs), BTreeSet::from([alice, bob]));
    }

    #[test]
    fn debits_are_grouped_by_sender_and_asset() {
        let (alice, bob) = (Uuid::new_v4(), Uuid::new_v4());
        let legs = [
            leg(alice, bob, "USD", "12.50"),
            leg(bob, alice, "XAU", "1.50"),
        ];

        let expected = BTreeMap::from([
            ((alice, "USD".to_string()), Amount(1250)),
            ((bob, "XAU".to_string()), Amount(150)),
        ]);
        assert_eq!(debit(&legs), Ok(expected));
    }

    #[test]
    fn debits_sum_legs_with_the_same_sender_and_asset() {
        let (alice, bob, carol) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let legs = [
            leg(alice, bob, "USD", "60.00"),
            leg(alice, carol, "USD", "60.00"),
        ];

        let expected = BTreeMap::from([((alice, "USD".to_string()), Amount(12000))]);
        assert_eq!(debit(&legs), Ok(expected));
    }

    #[test]
    fn debits_detect_overflow() {
        let (alice, bob) = (Uuid::new_v4(), Uuid::new_v4());
        let huge = Leg {
            amount: Amount(i64::MAX),
            ..leg(alice, bob, "USD", "1.00")
        };
        let legs = [huge, leg(alice, bob, "USD", "1.00")];

        assert_eq!(debit(&legs), Err(DomainError::Overflow));
    }

    #[test]
    fn validate_legs_rejects_bad_sets() {
        let (alice, bob) = (Uuid::new_v4(), Uuid::new_v4());
        let too_many: Vec<Leg> = (0..17).map(|_| leg(alice, bob, "USD", "1.00")).collect();

        assert_eq!(validate_legs(&[]), Err(DomainError::EmptyLegs));
        assert_eq!(validate_legs(&too_many), Err(DomainError::TooManyLegs));
        assert_eq!(
            validate_legs(&[leg(alice, alice, "USD", "1.00")]),
            Err(DomainError::InvalidReceiver)
        );
        assert_eq!(
            validate_legs(&[
                leg(alice, bob, "USD", "1.00"),
                leg(bob, alice, "XAU", "1.50")
            ]),
            Ok(())
        );
    }
}
