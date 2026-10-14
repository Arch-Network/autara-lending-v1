use std::ops::Deref;

use arch_program::{account::AccountInfo, pubkey::Pubkey};
use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    error::{LendingError, LendingResult, LendingResultExt},
    oracle::{
        chaos::{ChaosProvider, PRICE_CONFIG_SEED},
        oracle_config::OracleValidationConfig,
        oracle_price::OracleRate,
        pyth::PythProvider,
    },
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(tag = "type", content = "content")
)]
pub enum OracleProvider {
    Pyth(PythProvider),
    Chaos(ChaosProvider),
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct UncheckedOracleRate {
    rate: OracleRate,
    publish_time: i64,
}

impl UncheckedOracleRate {
    pub fn new(rate: OracleRate, publish_time: i64) -> Self {
        Self { rate, publish_time }
    }

    pub fn unsafe_rate(&self) -> OracleRate {
        self.rate
    }

    pub fn validate(
        &self,
        config: &OracleValidationConfig,
        unix_timestamp: i64,
    ) -> LendingResult<OracleRate> {
        if self.rate.rate().is_negative() {
            return Err(LendingError::NegativeOracleRate.into()).with_msg("rate is negative");
        }
        if self.rate.confidence().is_negative() {
            return Err(LendingError::NegativeOracleRate.into()).with_msg("confidence is negative");
        }
        if self.rate.rate().is_zero() {
            return Err(LendingError::OracleRateIsNull.into());
        }
        let age = unix_timestamp
            .checked_sub(self.publish_time)
            .ok_or(LendingError::SubtractionOverflow)?
            .max(0) as u64;
        if config.max_age().is_some_and(|max_age| age > max_age) {
            return Err(LendingError::OracleRateTooOld.into());
        }
        let relative_confidence = self.rate.relative_confidence()?;
        if config
            .min_relative_confidence()
            .is_some_and(|min_conf| &relative_confidence > min_conf)
        {
            return Err(LendingError::OracleRateRelativeConfidenceTooLow.into());
        }
        Ok(self.rate)
    }
}

pub trait OracleLoader {
    fn load_oracle_price<D: Deref<Target = [u8]>>(
        &self,
        view: AccountView<D>,
    ) -> LendingResult<UncheckedOracleRate>;
}

impl OracleProvider {
    pub fn as_ref<'a>(&'a self) -> OracleProviderRef<'a> {
        match self {
            OracleProvider::Pyth(provider) => OracleProviderRef::Pyth(provider),
            OracleProvider::Chaos(provider) => OracleProviderRef::Chaos(provider),
        }
    }
}

pub enum OracleProviderRef<'a> {
    Pyth(&'a PythProvider),
    Chaos(&'a ChaosProvider),
}

impl<'a> OracleProviderRef<'a> {
    pub fn oracle_feed_pubkey(&self) -> Option<Pubkey> {
        match self {
            OracleProviderRef::Pyth(provider) => {
                Some(Pubkey::find_program_address(&[&provider.feed_id], &provider.program_id).0)
            }
            OracleProviderRef::Chaos(provider) => Some(
                Pubkey::find_program_address(
                    &[PRICE_CONFIG_SEED.as_bytes(), &provider.feed_id],
                    &provider.program_id,
                )
                .0,
            ),
        }
    }
}

impl<'a> OracleLoader for OracleProviderRef<'a> {
    fn load_oracle_price<D: Deref<Target = [u8]>>(
        &self,
        view: AccountView<D>,
    ) -> LendingResult<UncheckedOracleRate> {
        // The feed's canonical address is derived from its feed_id, so the
        // account handed to us must live at that exact address. Without this
        // check, the underlying providers only validate the account's owner
        // and its self-reported `feed_id`/`price_id` field, both of which
        // live in account *data* the account's own authority controls (the
        // oracle program lets any signer create a feed under a feed_id of
        // their choosing). That would let anyone substitute their own
        // self-authorized account, with a forged id, for a market's real
        // price feed.
        if let Some(expected_key) = self.oracle_feed_pubkey() {
            if view.key != &expected_key {
                return Err(LendingError::InvalidOracleFeedAccount.into())
                    .with_msg("oracle account is not the feed's canonical account");
            }
        }
        match self {
            OracleProviderRef::Pyth(pyth_provider) => pyth_provider.load_oracle_price(view),
            OracleProviderRef::Chaos(chaos_provider) => chaos_provider.load_oracle_price(view),
        }
    }
}

#[derive(Clone, Copy)]
pub struct AccountView<'a, D> {
    pub key: &'a Pubkey,
    pub data: D,
    pub owner: &'a Pubkey,
}

impl<'a, 'b> TryFrom<&'b AccountInfo<'a>> for AccountView<'a, RefWrapper<'a, 'b>>
where
    'a: 'b,
{
    type Error = LendingError;

    fn try_from(
        account_info: &'b AccountInfo<'a>,
    ) -> Result<AccountView<'a, RefWrapper<'a, 'b>>, Self::Error> {
        Ok(AccountView {
            key: account_info.key,
            data: RefWrapper(
                account_info
                    .data
                    .try_borrow()
                    .map_err(|_| LendingError::FailedToLoadAccount)?,
            ),
            owner: account_info.owner,
        })
    }
}

pub struct RefWrapper<'a, 'b>(std::cell::Ref<'b, &'a mut [u8]>);

impl<'a, 'b> Deref for RefWrapper<'a, 'b> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl<'a, D: Deref<Target = [u8]>> From<(&'a Pubkey, D, &'a Pubkey)> for AccountView<'a, D> {
    fn from((key, data, owner): (&'a Pubkey, D, &'a Pubkey)) -> Self {
        AccountView { key, owner, data }
    }
}

#[cfg(feature = "client")]
pub mod client {
    use crate::oracle::oracle_provider::AccountView;
    use arch_sdk::AccountInfoWithPubkey;

    impl<'a> From<&'a AccountInfoWithPubkey> for AccountView<'a, &'a [u8]> {
        fn from(info: &'a AccountInfoWithPubkey) -> Self {
            AccountView {
                key: &info.key,
                data: &info.data,
                owner: &info.owner,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::LendingError, math::ifixed_point::IFixedPoint, oracle::oracle_price::OracleRate,
    };

    /// The oracle program lets anyone create a fresh feed account under a
    /// feed_id of their own choosing and become its authority (see
    /// `programs/autara-oracle`). Nothing stops that authority from later
    /// writing an arbitrary `id` into their own account's data. If the
    /// consumer only checks the account's *data* (owner + embedded feed id)
    /// and never checks that the account's *address* is the canonical PDA
    /// for the configured feed, an attacker can submit their own
    /// self-controlled account, with a forged `id` matching a real market's
    /// feed, as if it were that market's genuine price feed.
    #[test]
    fn load_oracle_price_rejects_account_at_wrong_address() {
        use crate::oracle::pyth::{Metadata, PriceData, PythPrice, PythPriceAccount};
        use arch_program::pubkey::Pubkey;

        let victim_feed_id = [7u8; 32];
        let program_id = Pubkey::new_unique();
        let provider = crate::oracle::pyth::PythProvider {
            feed_id: victim_feed_id,
            program_id,
        };

        // Attacker-controlled account: owned by the right program, and its
        // embedded `id` is forged to match the victim's feed_id, but it does
        // NOT live at the canonical PDA for that feed_id (it lives wherever
        // the attacker created their own feed account).
        let forged_account = PythPriceAccount {
            pyth_price: PythPrice {
                id: victim_feed_id,
                price: PriceData {
                    price: 1,
                    conf: 0,
                    expo: 0,
                    publish_time: 1_000,
                },
                ema_price: PriceData {
                    price: 1,
                    conf: 0,
                    expo: 0,
                    publish_time: 1_000,
                },
                metadata: Metadata {
                    slot: 0,
                    proof_available_time: 1_000,
                    prev_publish_time: 999,
                },
            },
            authority: Pubkey::new_unique(),
        };
        let bytes = bytemuck::bytes_of(&forged_account).to_vec();
        let attacker_owned_key = Pubkey::new_unique();
        assert_ne!(
            attacker_owned_key,
            Pubkey::find_program_address(&[&victim_feed_id], &program_id).0
        );

        let view: AccountView<Vec<u8>> = (&attacker_owned_key, bytes, &program_id).into();
        let result = OracleProviderRef::Pyth(&provider).load_oracle_price(view);
        assert_eq!(*result.unwrap_err(), LendingError::InvalidOracleFeedAccount);
    }

    #[test]
    fn test_validate_valid_rate() {
        let rate = OracleRate::new(IFixedPoint::lit("1.5"), IFixedPoint::lit("0.01"));
        let oracle_rate = UncheckedOracleRate::new(rate, 100);
        let config = OracleValidationConfig::new(60, 0.05.into());
        let result = oracle_rate.validate(&config, 120);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), rate);
    }

    #[test]
    fn test_validate_rate_too_old() {
        let rate = OracleRate::new(IFixedPoint::lit("1.5"), IFixedPoint::lit("0.01"));
        let oracle_rate = UncheckedOracleRate::new(rate, 100);
        let config = OracleValidationConfig::new(10, 0.05.into());
        let result = oracle_rate.validate(&config, 120);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LendingError::OracleRateTooOld);
    }

    #[test]
    fn test_validate_confidence_too_low() {
        let rate = OracleRate::new(IFixedPoint::lit("1.5"), IFixedPoint::lit("0.1"));
        let oracle_rate = UncheckedOracleRate::new(rate, 100);
        let config = OracleValidationConfig::new(60, 0.05.into());
        let result = oracle_rate.validate(&config, 120);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            LendingError::OracleRateRelativeConfidenceTooLow
        );
    }

    #[test]
    fn test_reject_negative_rate_at_construction() {
        let result = OracleRate::try_new(IFixedPoint::lit("-1.5"), IFixedPoint::lit("0.01"));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LendingError::OracleRateIsNull);
    }

    #[test]
    fn test_reject_negative_confidence_at_construction() {
        let result = OracleRate::try_new(IFixedPoint::lit("1.5"), IFixedPoint::lit("-0.01"));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LendingError::NegativeOracleRate);
    }
}
