//! The formatter for money the UI shows.

use std::fmt;

/// API cost ticks per US dollar.
const TICKS_PER_USD: i64 = 10_000_000_000;
const TICKS_PER_CENT: i64 = TICKS_PER_USD / 100;

/// An amount in US dollars, held as the API's exact ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usd {
    ticks: i64,
}

impl Usd {
    pub const fn from_ticks(ticks: i64) -> Self {
        Self { ticks }
    }

    pub const fn from_cents(cents: i64) -> Self {
        Self {
            ticks: cents.saturating_mul(TICKS_PER_CENT),
        }
    }

    /// A cost a server reported. An absent or non-positive cost is `None`, so
    /// the caller shows nothing rather than a `$0.00` nobody reported.
    pub fn reported(ticks: Option<i64>) -> Option<Self> {
        ticks.filter(|ticks| *ticks > 0).map(Self::from_ticks)
    }

    /// The amount in whole cents, rounded to the nearest cent. A half cent rounds away from zero.
    pub fn cents(self) -> i64 {
        let half = TICKS_PER_CENT / 2;
        let away = if self.ticks < 0 { -half } else { half };
        self.ticks.saturating_add(away) / TICKS_PER_CENT
    }
}

impl fmt::Display for Usd {
    /// `$12.34`. A positive amount under half a cent is `<$0.01`, so a real
    /// cost never reads as zero.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cents = self.cents();
        if cents == 0 && self.ticks > 0 {
            return f.write_str("<$0.01");
        }
        let sign = if cents < 0 { "-" } else { "" };
        let cents = cents.unsigned_abs();
        write!(f, "{sign}${}.{:02}", cents / 100, cents % 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(usd: Usd) -> String {
        usd.to_string()
    }

    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// A `$` right before a format placeholder is a second money formatter. Use [`Usd`] instead.
    #[test]
    fn no_other_code_formats_dollars() {
        let placeholder = regex::Regex::new(r"\$\{[A-Za-z0-9_.]*(:[^}]*)?\}").unwrap();
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        let found: Vec<String> = files
            .iter()
            .filter(|path| !path.ends_with("usd.rs"))
            .flat_map(|path| {
                let text = std::fs::read_to_string(path).unwrap();
                text.lines()
                    .enumerate()
                    .filter(|(_, line)| placeholder.is_match(line))
                    .map(|(index, line)| {
                        format!("{}:{}: {}", path.display(), index + 1, line.trim())
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(
            files.len() > 100,
            "the scan found only {} files",
            files.len()
        );
        assert!(
            found.is_empty(),
            "format money with crate::usd::Usd:\n{}",
            found.join("\n")
        );
    }

    #[test]
    fn every_amount_shows_dollars_and_two_cent_digits() {
        assert_eq!(text(Usd::from_ticks(1_234_500_000)), "$0.12");
        assert_eq!(text(Usd::from_ticks(5_000_000_000_000)), "$500.00");
        assert_eq!(text(Usd::from_ticks(342_000_000_000)), "$34.20");
        assert_eq!(text(Usd::from_ticks(5_000_000_000)), "$0.50");
        assert_eq!(text(Usd::from_ticks(0)), "$0.00");
        assert_eq!(text(Usd::from_cents(10_000)), "$100.00");
        assert_eq!(text(Usd::from_cents(1_999)), "$19.99");
    }

    #[test]
    fn amounts_round_to_the_nearest_cent() {
        // $0.004503, the top bar's old six-digit reading.
        assert_eq!(text(Usd::from_ticks(45_030_000)), "<$0.01");
        assert_eq!(text(Usd::from_ticks(49_999_999)), "<$0.01");
        assert_eq!(text(Usd::from_ticks(50_000_000)), "$0.01");
        assert_eq!(text(Usd::from_ticks(1_235_000_000)), "$0.12");
        assert_eq!(text(Usd::from_ticks(1_250_000_000)), "$0.13");
        assert_eq!(text(Usd::from_ticks(9_999_999_999)), "$1.00");
    }

    #[test]
    fn a_real_cost_under_half_a_cent_never_reads_as_zero() {
        assert_eq!(text(Usd::from_ticks(30_000)), "<$0.01");
        assert_eq!(text(Usd::from_ticks(1)), "<$0.01");
        assert_eq!(Usd::from_ticks(30_000).cents(), 0);
    }

    #[test]
    fn a_negative_amount_keeps_its_sign() {
        assert_eq!(text(Usd::from_cents(-250)), "-$2.50");
        assert_eq!(text(Usd::from_ticks(-1_250_000_000)), "-$0.13");
        assert_eq!(text(Usd::from_ticks(-1)), "$0.00");
    }

    #[test]
    fn a_reported_cost_is_absent_unless_positive() {
        assert_eq!(Usd::reported(None), None);
        assert_eq!(Usd::reported(Some(0)), None);
        assert_eq!(Usd::reported(Some(-5)), None);
        assert_eq!(Usd::reported(Some(7)), Some(Usd::from_ticks(7)));
    }

    #[test]
    fn huge_amounts_do_not_overflow() {
        assert_eq!(
            text(Usd::from_cents(i64::MAX)),
            text(Usd::from_ticks(i64::MAX))
        );
        assert!(text(Usd::from_ticks(i64::MIN)).starts_with("-$"));
    }
}
