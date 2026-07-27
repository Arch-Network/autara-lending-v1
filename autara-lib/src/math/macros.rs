#[macro_export]
macro_rules! define_fixed_point {
    (
        $name:ident,        // e.g. IFixedPoint
        $pod:ty,            // e.g. PodI128
        $fixed:ty,          // e.g. I80F48
        $inner:ty          // e.g. i128
    ) => {
        #[repr(transparent)]
        #[derive(
            Clone,
            Copy,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            bytemuck::Pod,
            bytemuck::Zeroable,
            borsh::BorshSerialize,
            borsh::BorshDeserialize,
        )]
        pub struct $name($pod);

        impl $name {
            pub const MAX: Self = $name(<$pod>::from_fixed(<$fixed>::MAX));
            pub const MIN: Self = $name(<$pod>::from_fixed(<$fixed>::MIN));
            pub const FRAC_NBITS: u32 = <$fixed>::FRAC_NBITS;

            pub const fn bits(self) -> $inner {
                self.0.fixed().to_bits()
            }

            pub const fn lit(str: &str) -> Self {
                $name(<$pod>::from_fixed(<$fixed>::lit(str)))
            }

            pub const fn from_u64(num: u64) -> Self {
                $name(<$pod>::from_fixed(<$fixed>::from_bits(
                    (num as $inner) << Self::FRAC_NBITS,
                )))
            }

            pub const fn from_bits(bits: $inner) -> Self {
                $name(<$pod>::from_fixed(<$fixed>::from_bits(bits)))
            }

            pub const fn from_fixed(num: $fixed) -> Self {
                $name(<$pod>::from_fixed(num))
            }

            pub const fn to_fixed(self) -> $fixed {
                self.0.fixed()
            }

            pub fn from_num<N: fixed::traits::ToFixed>(num: N) -> Self {
                $name(<$pod>::from_fixed(<$fixed>::from_num(num)))
            }

            pub fn from_num_checked<N: fixed::traits::ToFixed>(num: N) -> Option<Self> {
                Some($name(<$pod>::from_fixed(num.checked_to_fixed()?)))
            }

            pub const fn zero() -> Self {
                Self::from_u64(0)
            }

            pub const fn one() -> Self {
                Self::from_u64(1)
            }

            pub const fn is_zero(self) -> bool {
                self.0.fixed().to_bits() == 0
            }

            pub fn to_float(&self) -> f64 {
                self.0.fixed().to_num()
            }

            #[track_caller]
            pub fn checked_to_num<N: fixed::traits::FromFixed>(
                self,
            ) -> $crate::error::LendingResult<N> {
                self.0
                    .fixed()
                    .checked_to_num()
                    .ok_or_else($crate::with_context!(
                        $crate::error::LendingError::CastOverflow
                    ))
            }

            #[track_caller]
            pub fn as_u64_rounded_down(self) -> $crate::error::LendingResult<u64> {
                self.0
                    .fixed()
                    .checked_to_num()
                    .ok_or_else($crate::with_context!(
                        $crate::error::LendingError::CastOverflow
                    ))
            }

            #[track_caller]
            pub fn as_u64_rounded_up(self) -> $crate::error::LendingResult<u64> {
                let rounded_down =
                    self.0
                        .fixed()
                        .checked_to_num::<u64>()
                        .ok_or_else($crate::with_context!(
                            $crate::error::LendingError::CastOverflow
                        ))?;
                if self.0.fixed().frac() == 0 {
                    return Ok(rounded_down);
                } else {
                    return Ok(rounded_down.saturating_add(1));
                }
            }

            #[track_caller]
            pub fn as_u64_rounded(
                self,
                rounding: $crate::math::rounding::RoundingMode,
            ) -> $crate::error::LendingResult<u64> {
                match rounding {
                    $crate::math::rounding::RoundingMode::RoundDown => self.as_u64_rounded_down(),
                    $crate::math::rounding::RoundingMode::RoundUp => self.as_u64_rounded_up(),
                }
                .map_err($crate::map_context!(
                    $crate::error::LendingError::CastOverflow
                ))
            }

            #[track_caller]
            pub fn from_ratio<N: fixed::traits::ToFixed, M: fixed::traits::ToFixed>(
                num: N,
                dem: M,
            ) -> $crate::error::LendingResult<Self> {
                use $crate::math::safe_math::SafeMath;
                Self::from_num(num)
                    .safe_div(Self::from_num(dem))
                    .map_err($crate::map_context!(
                        $crate::error::LendingError::DivisionOverflow
                    ))
            }

            /// If shift > 0, shifts left by shift as u32 bits.
            /// If shift < 0, shifts right by -shift as u32 bits.
            ///
            /// Returns None if a left shift would overflow: the underlying
            /// `checked_shl` only rejects shift amounts >= the bit width and
            /// silently discards overflowing high bits otherwise, so the shift
            /// is verified by round-tripping back.
            pub fn checked_shift(&self, shift: i32) -> Option<Self> {
                if shift < 0 {
                    return self
                        .0
                        .fixed()
                        .checked_shr(-shift as u32)
                        .map($name::from_fixed);
                } else {
                    let shifted = self.0.fixed().checked_shl(shift as u32)?;
                    if shifted.checked_shr(shift as u32) != Some(self.0.fixed()) {
                        return None;
                    }
                    return Some($name::from_fixed(shifted));
                };
            }
        }

        impl<N: fixed::traits::ToFixed> From<N> for $name {
            fn from(num: N) -> Self {
                Self::from_num(num)
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0.fixed())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0.fixed())
            }
        }

        impl std::str::FromStr for $name {
            type Err = fixed::ParseFixedError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                <$fixed>::from_str(s).map($name::from_num)
            }
        }

        impl<N: fixed::traits::ToFixed + Copy> PartialEq<N> for $name {
            fn eq(&self, other: &N) -> bool {
                self.0.fixed() == <$fixed>::from_num(*other)
            }
        }

        impl $crate::math::safe_math::SafeMath for $name {
            fn safe_add(self, other: Self) -> $crate::error::LendingResult<Self> {
                self.0
                    .fixed()
                    .checked_add(other.0.fixed())
                    .map($name::from_fixed)
                    .ok_or_else($crate::with_context!(
                        $crate::error::LendingError::AdditionOverflow
                    ))
            }

            fn safe_sub(self, other: Self) -> $crate::error::LendingResult<Self> {
                self.0
                    .fixed()
                    .checked_sub(other.0.fixed())
                    .map($name::from_fixed)
                    .ok_or_else($crate::with_context!(
                        $crate::error::LendingError::SubtractionOverflow
                    ))
            }

            fn safe_mul(self, other: Self) -> $crate::error::LendingResult<Self> {
                self.0
                    .fixed()
                    .checked_mul(other.0.fixed())
                    .map($name::from_fixed)
                    .ok_or_else($crate::with_context!(
                        $crate::error::LendingError::MultiplicationOverflow
                    ))
            }

            #[track_caller]
            fn safe_div(self, other: Self) -> $crate::error::LendingResult<Self> {
                self.0
                    .fixed()
                    .checked_div(other.0.fixed())
                    .map($name::from_fixed)
                    .ok_or_else($crate::with_context!(
                        $crate::error::LendingError::DivisionOverflow
                    ))
            }
        }

        impl $crate::math::safe_math::SafeMath<u64, Self> for $name {
            fn safe_add(self, other: u64) -> $crate::error::LendingResult<Self> {
                self.0
                    .fixed()
                    .checked_add(<$fixed>::from_num(other))
                    .map($name::from_fixed)
                    .ok_or_else($crate::with_context!(
                        $crate::error::LendingError::AdditionOverflow
                    ))
            }

            fn safe_sub(self, other: u64) -> $crate::error::LendingResult<Self> {
                self.0
                    .fixed()
                    .checked_sub(<$fixed>::from_num(other))
                    .map($name::from_fixed)
                    .ok_or_else($crate::with_context!(
                        $crate::error::LendingError::SubtractionOverflow
                    ))
            }

            fn safe_mul(self, other: u64) -> $crate::error::LendingResult<Self> {
                self.0
                    .to_inner()
                    .checked_mul(other as $inner)
                    .map($name::from_bits)
                    .ok_or_else($crate::with_context!(
                        $crate::error::LendingError::MultiplicationOverflow
                    ))
            }

            fn safe_div(self, other: u64) -> $crate::error::LendingResult<Self> {
                self.0
                    .to_inner()
                    .checked_div(other as $inner)
                    .map($name::from_bits)
                    .ok_or_else($crate::with_context!(
                        $crate::error::LendingError::DivisionByZero
                    ))
            }
        }

        #[cfg(feature = "client")]
        pub mod serde {
            use super::*;

            impl ::serde::Serialize for $name {
                fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                where
                    S: ::serde::Serializer,
                {
                    serializer.serialize_str(&self.0.fixed().to_string())
                }
            }

            impl<'a> ::serde::de::Deserialize<'a> for $name {
                fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                where
                    D: ::serde::Deserializer<'a>,
                {
                    let s: String = ::serde::de::Deserialize::deserialize(deserializer)?;
                    s.parse().map_err(::serde::de::Error::custom)
                }
            }
        }
    };
}

#[macro_export]
macro_rules! define_pod_int {
    (
        $name:ident,
        $inner:ty,
        $fixed:ty
    ) => {
        #[repr(transparent)]
        #[derive(
            Clone,
            Copy,
            Default,
            PartialEq,
            Eq,
            bytemuck::Pod,
            bytemuck::Zeroable,
            borsh::BorshSerialize,
            borsh::BorshDeserialize,
        )]
        pub struct $name([u8; std::mem::size_of::<$inner>()]);

        impl $name {
            pub const fn new(value: $inner) -> Self {
                Self(value.to_le_bytes())
            }

            pub const fn to_inner(self) -> $inner {
                <$inner>::from_le_bytes(self.0)
            }

            pub const fn fixed(self) -> $fixed {
                <$fixed>::from_bits(self.to_inner())
            }

            pub const fn from_fixed(value: $fixed) -> Self {
                Self::new(value.to_bits())
            }
        }

        impl PartialOrd for $name {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                self.to_inner().partial_cmp(&other.to_inner())
            }
        }

        impl Ord for $name {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.to_inner().cmp(&other.to_inner())
            }
        }

        impl Into<$fixed> for $name {
            fn into(self) -> $fixed {
                self.fixed()
            }
        }

        impl From<$fixed> for $name {
            fn from(value: $fixed) -> Self {
                Self::from_fixed(value)
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({})", stringify!($name), self.to_inner())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.to_inner())
            }
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            use borsh::{BorshDeserialize, BorshSerialize};

            #[test]
            fn test_new_and_conversion() {
                let value = $name::new(12345678901234567890 as $inner);
                assert_eq!(value.to_inner(), 12345678901234567890 as $inner);
                assert_eq!(
                    value.fixed().to_bits(),
                    <$fixed>::from_bits(12345678901234567890 as $inner).to_bits()
                );

                let fixed_value = <$fixed>::from_bits(12345678901234567890 as $inner);
                let pod_from_fixed = $name::from_fixed(fixed_value);
                assert_eq!(pod_from_fixed.to_inner(), 12345678901234567890 as $inner);
            }

            #[test]
            fn test_ordering() {
                let a = $name::new(10 as $inner);
                let b = $name::new(20 as $inner);
                assert!(a < b);
                assert!(b > a);
                assert!(a <= b);
                assert!(b >= a);
                assert!(a != b);
                assert!(a == $name::new(10 as $inner));
            }

            #[test]
            fn test_display_debug() {
                let value = $name::new(12345678901234567890 as $inner);
                assert_eq!(
                    format!("{}", value),
                    format!("{}", 12345678901234567890 as $inner)
                );
                assert_eq!(
                    format!("{:?}", value),
                    format!("{}({})", stringify!($name), 12345678901234567890 as $inner)
                );
            }

            #[test]
            fn test_serialize_deserialize() {
                let value = $name::new(12345678901234567890 as $inner);
                let mut serialized = vec![];
                value.serialize(&mut serialized).unwrap();
                let deserialized = $name::try_from_slice(&serialized).unwrap();
                assert_eq!(value, deserialized);
                assert_eq!(serialized, value.to_inner().to_le_bytes());
            }

            #[test]
            fn test_bytemuck() {
                let value = $name::new(12345678901234567890 as $inner);
                let bytes: [u8; std::mem::size_of::<$inner>()] = bytemuck::cast(value);
                let pod_from_bytes: $name = bytemuck::cast(bytes);
                assert_eq!(value, pod_from_bytes);
                assert_eq!(bytes, (12345678901234567890 as $inner).to_le_bytes());
            }
        }
    };
}
