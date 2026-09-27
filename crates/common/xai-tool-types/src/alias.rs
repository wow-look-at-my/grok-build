//! A field its sender may name more than one way.
//!
//! `#[serde(alias = "x")]` accepts a second key for one field, and the derived
//! deserializer treats that key as a type error the moment BOTH arrive: a delta
//! carrying `"reasoning":"We"` and `"reasoning_content":"We"` fails with
//! `duplicate field`, although the two values are one string. Providers that put
//! reasoning on both spellings in every chunk therefore lose the whole response
//! to a duplicate the sender never meant as a second value.
//!
//! A wire struct instead names each spelling its own field on a private shadow
//! (`#[serde(try_from = "Shadow")]`) and folds them through [`Aliases::fold`].
//! A sender that repeats itself then parses, and a sender that contradicts
//! itself is an error naming both keys. The serialized shape never changes:
//! `Serialize` stays derived on the real struct, so the outgoing key cannot
//! drift from the canonical one.

use std::error::Error;
use std::fmt;

/// The keys one field is read from: the canonical one first, then every alias.
///
/// Declare one as a `const` on the type that owns the field, and call
/// [`fold`](Self::fold) from its `TryFrom` for the wire shadow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aliases {
    /// The key this type writes, and the one the rest of the code names.
    pub canonical: &'static str,
    /// Every other key the field is accepted under.
    pub aliases: &'static [&'static str],
}

impl Aliases {
    pub const fn new(canonical: &'static str, aliases: &'static [&'static str]) -> Self {
        Self { canonical, aliases }
    }

    /// Every key the field is read from, canonical first.
    pub fn keys(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.canonical).chain(self.aliases.iter().copied())
    }

    /// Reduce the values read under each key to the one value the field holds.
    ///
    /// `values` holds one entry per key of [`keys`](Self::keys), in that order;
    /// a key absent from the input contributes `None`. The first value present
    /// is the result. A later key carrying a value equal to it is the same
    /// statement twice and is accepted. A later key carrying a different value
    /// contradicts the first, which no reader may resolve silently.
    pub fn fold<T>(&self, values: Vec<Option<T>>) -> Result<Option<T>, AliasConflict>
    where
        T: PartialEq + std::fmt::Debug,
    {
        let wanted = self.keys().count();
        if values.len() != wanted {
            return Err(AliasConflict::ShapeMismatch {
                canonical: self.canonical,
                keys: wanted,
                values: values.len(),
            });
        }

        let mut held: Option<(&'static str, T)> = None;
        for (key, value) in self.keys().zip(values) {
            let Some(value) = value else { continue };
            match &held {
                None => held = Some((key, value)),
                Some((from, prior)) => {
                    if prior != &value {
                        return Err(AliasConflict::DifferingValues {
                            canonical: self.canonical,
                            first_key: *from,
                            first_value: format!("{prior:?}"),
                            second_key: key,
                            second_value: format!("{value:?}"),
                        });
                    }
                }
            }
        }
        Ok(held.map(|(_, value)| value))
    }
}

/// What went wrong folding one field's key spellings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasConflict {
    /// Two keys carried different values, so the input disagrees with itself.
    DifferingValues {
        canonical: &'static str,
        first_key: &'static str,
        first_value: String,
        second_key: &'static str,
        second_value: String,
    },
    /// The shadow struct named a different number of keys than the [`Aliases`]
    /// lists. A bug in this program, reported rather than panic-ed because the
    /// fold sits on a deserialization path.
    ShapeMismatch {
        canonical: &'static str,
        keys: usize,
        values: usize,
    },
}

impl fmt::Display for AliasConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DifferingValues {
                canonical,
                first_key,
                first_value,
                second_key,
                second_value,
            } => write!(
                f,
                "field `{canonical}` was sent twice with different values: \
                 `{first_key}` = {first_value} and `{second_key}` = {second_value}"
            ),
            Self::ShapeMismatch {
                canonical,
                keys,
                values,
            } => write!(
                f,
                "field `{canonical}` reads {keys} keys but {values} values were offered"
            ),
        }
    }
}

impl Error for AliasConflict {}

/// One field a wire reader accepts under more than one key.
#[derive(Debug, Clone, Copy)]
pub struct WireAlias {
    /// Crate-relative path of the file that folds these keys.
    pub file: &'static str,
    /// The type whose shadow folds them.
    pub ty: &'static str,
    /// The key the type writes.
    pub canonical: &'static str,
    /// The keys it also accepts.
    pub aliases: &'static [&'static str],
}

/// Every field on an untrusted path that reads more than one key spelling,
/// folded through [`Aliases`].
///
/// The drift test in this module reads this table against the source: a field
/// that goes back to a bare `#[serde(alias)]` shows up as an unclassified alias,
/// and so does one newly added.
pub const WIRED: &[WireAlias] = &[WireAlias {
    file: "crates/codegen/xai-grok-sampling-types/src/types.rs",
    ty: "ChatChunkDelta",
    canonical: "reasoning_content",
    aliases: &["reasoning"],
}];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_value_under_one_key_reads_as_that_value() {
        let spelling = Aliases::new("reasoning_content", &["reasoning"]);
        assert_eq!(
            spelling
                .fold(vec![Some("We".to_owned()), None])
                .unwrap()
                .as_deref(),
            Some("We")
        );
        assert_eq!(
            spelling
                .fold(vec![None, Some("We".to_owned())])
                .unwrap()
                .as_deref(),
            Some("We")
        );
        assert_eq!(spelling.fold::<String>(vec![None, None]).unwrap(), None);
    }

    /// The whole point: a gateway that sends the same text twice is not an error.
    #[test]
    fn the_same_value_under_both_keys_reads_once_and_is_not_an_error() {
        let spelling = Aliases::new("reasoning_content", &["reasoning"]);
        let folded = spelling
            .fold(vec![Some("We".to_owned()), Some("We".to_owned())])
            .expect("identical spellings must not conflict");
        assert_eq!(folded.as_deref(), Some("We"));
    }

    #[test]
    fn two_different_values_error_naming_both_keys() {
        let spelling = Aliases::new("reasoning_content", &["reasoning"]);
        let err = spelling
            .fold(vec![Some("one".to_owned()), Some("two".to_owned())])
            .expect_err("conflicting spellings must not resolve silently");
        let message = err.to_string();
        assert!(message.contains("reasoning_content"), "{message}");
        assert!(message.contains("`reasoning`"), "{message}");
        assert!(message.contains("different values"), "{message}");
    }

    #[test]
    fn a_shadow_naming_the_wrong_number_of_keys_is_an_error_not_a_panic() {
        let spelling = Aliases::new("reasoning_content", &["reasoning"]);
        let err = spelling
            .fold(vec![Some("We".to_owned())])
            .expect_err("one value for two keys is a bug");
        assert!(err.to_string().contains("reads 2 keys"), "{err}");
    }

    #[test]
    fn keys_lists_the_canonical_key_first() {
        let spelling = Aliases::new("cost_in_usd_ticks", &["cost_usd_ticks", "cost"]);
        assert_eq!(
            spelling.keys().collect::<Vec<_>>(),
            vec!["cost_in_usd_ticks", "cost_usd_ticks", "cost"]
        );
    }

    #[test]
    fn every_wired_alias_is_folded_in_the_file_that_declares_it() {
        let root = workspace_root();
        for wired in WIRED {
            let path = root.join(wired.file);
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{} is not readable: {e}", wired.file));
            let declared = format!(
                "Aliases::new(\"{}\", &[{}])",
                wired.canonical,
                wired
                    .aliases
                    .iter()
                    .map(|a| format!("\"{a}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            assert!(
                source.contains(&declared),
                "{} names no `Aliases` for `{}`; write `{}` in it",
                wired.file,
                wired.canonical,
                declared
            );
            assert!(
                !source.contains(&format!("alias = \"{}\"", wired.aliases[0])),
                "{} still reads `{}` through a bare #[serde(alias)], which is \
                 the duplicate-field failure this table exists to prevent",
                wired.file,
                wired.aliases[0]
            );
        }
    }

    fn workspace_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .and_then(std::path::Path::parent)
            .expect("crates/common/xai-tool-types sits three levels under the workspace root")
            .to_path_buf()
    }
}
