//! Serde for a configured retry budget: a count, or `-1` / `"unlimited"`.

use serde::{Deserialize, Deserializer, Serializer};

pub const UNLIMITED: u32 = u32::MAX;

#[derive(Deserialize)]
#[serde(untagged)]
enum Raw {
    Int(i64),
    Str(String),
}

/// Any value other than a count, `-1` or `"unlimited"` fails the parse.
pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<Raw>::deserialize(deserializer)? {
        None => Ok(None),
        Some(raw) => parse(raw).map(Some).map_err(serde::de::Error::custom),
    }
}

fn parse(raw: Raw) -> Result<u32, String> {
    match raw {
        Raw::Int(-1) => Ok(UNLIMITED),
        Raw::Int(n) => u32::try_from(n)
            .ok()
            .filter(|n| *n != UNLIMITED)
            .ok_or_else(|| {
                format!("retry budget {n} is not valid: use 0 or more, or -1 for unlimited")
            }),
        Raw::Str(s) if s.eq_ignore_ascii_case("unlimited") => Ok(UNLIMITED),
        Raw::Str(s) => Err(format!(
            "retry budget {s:?} is not valid: use 0 or more, or -1 for unlimited"
        )),
    }
}

/// [`UNLIMITED`] is written as `-1`.
pub fn serialize<S>(value: &Option<u32>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(UNLIMITED) => serializer.serialize_i64(-1),
        Some(n) => serializer.serialize_u32(*n),
        None => serializer.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
    #[serde(default)]
    struct Table {
        #[serde(with = "super", skip_serializing_if = "Option::is_none")]
        max_retries: Option<u32>,
    }

    fn read(json: &str) -> Result<Option<u32>, serde_json::Error> {
        serde_json::from_str::<Table>(json).map(|t| t.max_retries)
    }

    #[test]
    fn reads_a_count_and_both_spellings_of_unlimited() {
        assert_eq!(read(r#"{}"#).unwrap(), None);
        assert_eq!(read(r#"{"max_retries": 0}"#).unwrap(), Some(0));
        assert_eq!(read(r#"{"max_retries": 250}"#).unwrap(), Some(250));
        assert_eq!(read(r#"{"max_retries": -1}"#).unwrap(), Some(super::UNLIMITED));
        assert_eq!(
            read(r#"{"max_retries": "Unlimited"}"#).unwrap(),
            Some(super::UNLIMITED)
        );
    }

    #[test]
    fn refuses_a_value_that_is_not_a_budget() {
        for bad in [r#"-2"#, r#""lots""#, r#"4294967295"#, r#"4294967296"#] {
            let err = read(&format!(r#"{{"max_retries": {bad}}}"#)).unwrap_err();
            assert!(err.to_string().contains("-1 for unlimited"), "{bad}: {err}");
        }
    }

    #[test]
    fn unlimited_writes_back_as_minus_one() {
        let json = serde_json::to_string(&Table {
            max_retries: Some(super::UNLIMITED),
        })
        .unwrap();
        assert_eq!(json, r#"{"max_retries":-1}"#);
        assert_eq!(read(&json).unwrap(), Some(super::UNLIMITED));
    }
}
