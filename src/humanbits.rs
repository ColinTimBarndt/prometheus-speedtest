//! Serializer for bit units such as "100", "10K", "100G"

use serde::{Deserialize, Deserializer, Serialize, Serializer};

static UNITS: &'static [char] = &['K', 'M', 'G', 'T', 'P'];

pub fn serialize<S>(&(mut value): &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    use std::fmt::Write;

    let mut use_unit = None::<char>;
    for &unit in UNITS {
        if value % 1000 != 0 {
            break;
        }
        value /= 1000;
        use_unit = Some(unit);
    }
    let mut buf = String::with_capacity(16);
    _ = write!(buf, "{value}");
    if let Some(unit) = use_unit {
        buf.push(unit);
    }

    Serialize::serialize(&buf, serializer)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let mut s: String = Deserialize::deserialize(deserializer)?;

    let thousands = if let Some(unit) = s.chars().rev().next()
        && !unit.is_ascii_digit()
    {
        let Some(index) = UNITS.iter().position(|ch| *ch == unit) else {
            return Err(serde::de::Error::custom(format!("Unknown unit '{unit}'")));
        };
        s.pop();
        index + 1
    } else {
        0
    };

    let mut number = s.parse().map_err(|_| {
        serde::de::Error::invalid_value(serde::de::Unexpected::Str(&s), &"32-bit integer")
    })?;

    for _ in 0..thousands {
        number *= 1000;
    }

    Ok(number)
}
