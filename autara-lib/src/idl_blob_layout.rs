//! Phase 3 verification for the published IDL (`idl/autara_lending.idl.json`).
//!
//! `create_market` / `update_config` represent their nested config types as
//! opaque fixed-size `u8` arrays. Those byte counts must equal the actual
//! Borsh-serialized size of the real types — which is the on-wire size, and is
//! NOT always the in-memory `size_of`: some of these types use a manual
//! `unsafe impl Pod` and contain alignment padding that Borsh drops (e.g.
//! `OracleConfig` is 264 bytes in memory but 257 over Borsh). These tests
//! measure Borsh directly and pin the sizes so a future field change can't
//! silently desync the on-chain IDL from the program.
//!
//! If any assertion here changes, update the matching `array`/variant sizes in
//! `idl/autara_lending.idl.json` (and `idl/autara-idl-import.json`).

use bytemuck::Zeroable;

use crate::{
    interest_rate::{
        curve::{adaptative_curve::AdaptiveInterestRateCurve, polyline::PolylineInterestRateCurve},
        interest_rate_kind::InterestRateCurveKind,
        interest_rate_per_second::InterestRatePerSecond,
    },
    math::ifixed_point::IFixedPoint,
    oracle::oracle_config::OracleConfig,
    state::market_config::LtvConfig,
};

fn borsh_len<T: borsh::BorshSerialize>(value: &T) -> usize {
    borsh::to_vec(value).expect("borsh serialize").len()
}

#[test]
fn opaque_blob_sizes_match_idl() {
    assert_eq!(borsh_len(&IFixedPoint::zeroed()), 16, "IFixedPoint");
    assert_eq!(borsh_len(&LtvConfig::zeroed()), 48, "LtvConfig = 3x IFixedPoint");
    // 257 over Borsh (264 in memory; the difference is dropped alignment padding).
    assert_eq!(borsh_len(&OracleConfig::zeroed()), 257, "OracleConfig");
}

#[test]
fn interest_rate_curve_kind_variant_sizes_match_idl() {
    // Borsh enum = 1 tag byte + variant payload. The IDL models each variant as
    // a tuple of one opaque `[u8; N]`, so N must equal the payload size.
    assert_eq!(borsh_len(&InterestRatePerSecond::zeroed()), 16, "Fixed payload");
    assert_eq!(
        borsh_len(&PolylineInterestRateCurve::zeroed()),
        64,
        "Polyline payload = 8 points x 8 bytes"
    );
    assert_eq!(
        borsh_len(&AdaptiveInterestRateCurve::zeroed()),
        16,
        "Adaptive payload"
    );

    assert_eq!(
        borsh_len(&InterestRateCurveKind::Fixed(InterestRatePerSecond::zeroed())),
        1 + 16
    );
    assert_eq!(
        borsh_len(&InterestRateCurveKind::Polyline(
            PolylineInterestRateCurve::zeroed()
        )),
        1 + 64
    );
    assert_eq!(
        borsh_len(&InterestRateCurveKind::Adaptive(
            AdaptiveInterestRateCurve::zeroed()
        )),
        1 + 16
    );
}
