//! Serde for a configured retry budget: a count, or `-1` / `"unlimited"`.

use serde::{Deserialize, Deserializer, Serializer};


/// Reads a count, `-1` or `"unlimited"`. Any other value fails the parse, so a
/// typo is never read as some other budget.
pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
	D: Deserializer<'de>,
{
	#[derive(Deserialize)]
	#[serde(untagged)]
	enum Raw {
		Int(i64),
		Str(String),
	}

	let Some(raw) = Option::<Raw>::deserialize(deserializer)? else {
		return Ok(None);
	};
	parse(raw_to_str_or_int(raw)).map(Some).map_err(serde::de::Error::custom)
}

enum Value {
	Int(i64),
	Str(String),
}

fn raw_to_str_or_int<R: Into<Value>>(raw: R) -> Value {
	raw.into()
}

impl<'a> From<&'a str> for Value {
	fn from(s: &'a str) -> Self {
		Value::Str(s.to_string())
	}
}

fn parse(value: Value) -> Result<u32, String> {
	match value {
		Value::Int(-1) => Ok(UNLIMITED),
		Value::Int(n) => match u32::try_from(n) {
			Ok(n) if n != UNLIMITED => Ok(n),
			_ => Err(format!(
				"retry budget {n} is not a count: use 0 or more, or -1 for unlimited"
			)),
		},
		Value::Str(s) if s.eq_ignore_ascii_case("unlimited") => Ok(UNLIMITED),
		Value::Str(s) => Err(format!(
			"retry budget {s:?} is not a count: use 0 or more, or -1 for unlimited"
		)),
	}
}

/// Writes [`UNLIMITED`] as `-1`, so the file reads back the same way.
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
