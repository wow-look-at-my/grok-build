//! Writes a release number into an already-linked grok binary.
//!
//! The number comes from buildhost, which only assigns it once the build has
//! passed. Reading it with `option_env!` at compile time is what forced the
//! release to be created before anything was built.

use xai_grok_version::{STAMP_MAGIC, STAMP_PAYLOAD_LEN, STAMP_SLOT_LEN};

#[derive(Debug, PartialEq, Eq)]
pub enum StampError {
    /// No slot. The binary was linked without `xai-grok-version`, or something
    /// dropped the `#[used]` static.
    SlotMissing,
    /// More than one slot. Which one the running binary reads is then a guess,
    /// so this fails instead of patching an arbitrary one.
    SlotAmbiguous(usize),
    /// The slot is at the very end of the file and is cut short.
    SlotTruncated,
    /// The version does not fit the reserved payload.
    VersionTooLong { len: usize, max: usize },
    /// An empty version reads as "unstamped" once written, so it is refused.
    VersionEmpty,
}

impl std::fmt::Display for StampError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SlotMissing => write!(f, "no version stamp slot in this binary"),
            Self::SlotAmbiguous(n) => {
                write!(f, "{n} version stamp slots in this binary, expected 1")
            }
            Self::SlotTruncated => {
                write!(f, "the version stamp slot runs past the end of the file")
            }
            Self::VersionTooLong { len, max } => {
                write!(f, "version is {len} bytes, the slot holds {max}")
            }
            Self::VersionEmpty => write!(f, "version is empty"),
        }
    }
}

impl std::error::Error for StampError {}

/// Writes `version` into the slot in `binary`, in place.
///
/// The length byte goes in first in source order but the whole slot is written
/// together, so a reader never sees a length without its payload.
pub fn stamp(binary: &mut [u8], version: &str) -> Result<usize, StampError> {
    if version.is_empty() {
        return Err(StampError::VersionEmpty);
    }
    if version.len() > STAMP_PAYLOAD_LEN {
        return Err(StampError::VersionTooLong {
            len: version.len(),
            max: STAMP_PAYLOAD_LEN,
        });
    }
    let at = find_slot(binary)?;
    let payload = at + STAMP_MAGIC.len() + 1;
    binary[at + STAMP_MAGIC.len()] = version.len() as u8;
    binary[payload..payload + STAMP_PAYLOAD_LEN].fill(0);
    binary[payload..payload + version.len()].copy_from_slice(version.as_bytes());
    Ok(at)
}

/// Reads back what the slot holds, the way the shipped binary reads it.
pub fn read_stamp(binary: &[u8]) -> Result<Option<String>, StampError> {
    let at = find_slot(binary)?;
    let len = usize::from(binary[at + STAMP_MAGIC.len()]);
    if len == 0 || len > STAMP_PAYLOAD_LEN {
        return Ok(None);
    }
    let payload = at + STAMP_MAGIC.len() + 1;
    Ok(String::from_utf8(binary[payload..payload + len].to_vec()).ok())
}

/// Offset of the one slot in `binary`.
fn find_slot(binary: &[u8]) -> Result<usize, StampError> {
    let hits: Vec<usize> = binary
        .windows(STAMP_MAGIC.len())
        .enumerate()
        .filter(|(_, w)| *w == &STAMP_MAGIC[..])
        .map(|(i, _)| i)
        .collect();
    match hits.len() {
        0 => Err(StampError::SlotMissing),
        1 => {
            let at = hits[0];
            if at + STAMP_SLOT_LEN > binary.len() {
                return Err(StampError::SlotTruncated);
            }
            Ok(at)
        }
        n => Err(StampError::SlotAmbiguous(n)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slot surrounded by other bytes, the way it sits in a real binary.
    fn buffer_with_slot() -> Vec<u8> {
        let mut buf = vec![0xAA; 32];
        buf.extend_from_slice(&STAMP_MAGIC[..]);
        buf.extend_from_slice(&[0u8; 1 + STAMP_PAYLOAD_LEN]);
        buf.extend_from_slice(&[0xBB; 32]);
        buf
    }

    #[test]
    fn a_stamped_slot_reads_back_the_version() {
        let mut buf = buffer_with_slot();
        assert_eq!(read_stamp(&buf), Ok(None), "starts unstamped");
        stamp(&mut buf, "0.1.220-alpha.7").unwrap();
        assert_eq!(
            read_stamp(&buf).unwrap().as_deref(),
            Some("0.1.220-alpha.7")
        );
    }

    /// The bytes on either side of the slot are the rest of the binary. Writing
    /// outside the slot corrupts it.
    #[test]
    fn stamping_touches_nothing_outside_the_slot() {
        let mut buf = buffer_with_slot();
        let len = buf.len();
        stamp(&mut buf, "v9").unwrap();
        assert!(buf[..32].iter().all(|b| *b == 0xAA));
        assert!(buf[len - 32..].iter().all(|b| *b == 0xBB));
        assert_eq!(buf.len(), len);
    }

    /// A second stamp must fully replace the first, not leave its tail behind.
    #[test]
    fn restamping_leaves_no_tail_of_the_longer_previous_version() {
        let mut buf = buffer_with_slot();
        stamp(&mut buf, "0.1.220-alpha.7").unwrap();
        stamp(&mut buf, "v9").unwrap();
        assert_eq!(read_stamp(&buf).unwrap().as_deref(), Some("v9"));
    }

    #[test]
    fn a_binary_with_no_slot_is_refused() {
        assert_eq!(
            stamp(&mut vec![0u8; 512], "v1"),
            Err(StampError::SlotMissing)
        );
    }

    /// Two slots mean the running binary reads one of them and the stamper
    /// cannot tell which.
    #[test]
    fn two_slots_are_refused() {
        let mut buf = buffer_with_slot();
        buf.extend_from_slice(&buffer_with_slot());
        assert_eq!(stamp(&mut buf, "v1"), Err(StampError::SlotAmbiguous(2)));
    }

    #[test]
    fn a_version_past_the_payload_is_refused() {
        let mut buf = buffer_with_slot();
        let long = "9".repeat(STAMP_PAYLOAD_LEN + 1);
        assert_eq!(
            stamp(&mut buf, &long),
            Err(StampError::VersionTooLong {
                len: STAMP_PAYLOAD_LEN + 1,
                max: STAMP_PAYLOAD_LEN,
            }),
        );
    }

    /// An empty version would write a zero length, which reads as unstamped —
    /// a silent no-op on the release path.
    #[test]
    fn an_empty_version_is_refused() {
        let mut buf = buffer_with_slot();
        assert_eq!(stamp(&mut buf, ""), Err(StampError::VersionEmpty));
        assert_eq!(read_stamp(&buf), Ok(None));
    }

    /// A slot cut short by the end of the file would have the write run past it.
    #[test]
    fn a_truncated_slot_is_refused() {
        let mut buf = Vec::from(&STAMP_MAGIC[..]);
        buf.push(0);
        assert_eq!(stamp(&mut buf, "v1"), Err(StampError::SlotTruncated));
    }
}
