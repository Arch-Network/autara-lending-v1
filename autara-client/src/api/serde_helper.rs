pub mod serde_pubkey {
    use arch_sdk::arch_program::pubkey::Pubkey;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::parse_pubkey_str;

    pub fn serialize<S>(pubkey: &Pubkey, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&bs58::encode(pubkey.serialize()).into_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Pubkey, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        parse_pubkey_str(&s).map_err(serde::de::Error::custom)
    }
}

pub mod serde_pubkey_vec {
    use arch_sdk::arch_program::pubkey::Pubkey;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::parse_pubkey_str;

    pub fn serialize<S>(keys: &[Pubkey], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let keys: Vec<String> = keys
            .iter()
            .map(|k| bs58::encode(k.serialize()).into_string())
            .collect();
        keys.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Pubkey>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let keys: Vec<String> = Vec::deserialize(deserializer)?;
        keys.into_iter()
            .map(|s| parse_pubkey_str(&s).map_err(serde::de::Error::custom))
            .collect()
    }
}

pub mod serde_optional_pubkey {
    use arch_sdk::arch_program::pubkey::Pubkey;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::parse_pubkey_str;

    pub fn serialize<S>(pubkey: &Option<Pubkey>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match pubkey {
            Some(pk) => serializer.serialize_str(&bs58::encode(pk.serialize()).into_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Pubkey>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = Option::<String>::deserialize(deserializer)?;
        match s {
            Some(s) => Ok(Some(
                parse_pubkey_str(&s).map_err(serde::de::Error::custom)?,
            )),
            None => Ok(None),
        }
    }
}

pub mod serde_from_str {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::str::FromStr;

    pub fn serialize<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: ToString,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr,
        T::Err: std::fmt::Display,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

pub mod serde_from_optional_str {
    use std::str::FromStr;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S, T>(value: &Option<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: ToString,
    {
        match value {
            Some(v) => serializer.serialize_str(&v.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr,
        T::Err: std::fmt::Display,
    {
        let s = Option::<String>::deserialize(deserializer)?;
        match s {
            Some(s) => s.parse().map_err(serde::de::Error::custom).map(Some),
            None => Ok(None),
        }
    }
}

/// Parse a pubkey from base58 (preferred) or hex (transition).
fn parse_pubkey_str(raw: &str) -> Result<arch_sdk::arch_program::pubkey::Pubkey, String> {
    use arch_sdk::arch_program::pubkey::Pubkey;
    use std::str::FromStr;

    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty pubkey".into());
    }
    // Prefer base58 (human-readable / explorer form).
    if let Ok(bytes) = bs58::decode(raw).into_vec() {
        if bytes.len() == 32 {
            return Ok(Pubkey::from_slice(&bytes));
        }
    }
    // Fall back to hex for older clients.
    if let Ok(pk) = Pubkey::from_str(raw) {
        return Ok(pk);
    }
    if let Ok(bytes) = hex::decode(raw) {
        if bytes.len() == 32 {
            return Ok(Pubkey::from_slice(&bytes));
        }
    }
    Err(format!("invalid pubkey '{raw}' (expected base58 or hex)"))
}
