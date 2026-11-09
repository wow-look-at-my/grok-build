//! Reads `$GROK_HOME/pager.json` once at startup. A legacy `pager.toml` is read when it is the only file.
use super::config::AppearanceConfig;
use std::io;
use std::path::{Path, PathBuf};
use tokio::sync::watch;
/// The appearance config from `pager.json`. `.changed()` never fires.
pub struct ConfigWatcher {
    rx: watch::Receiver<AppearanceConfig>,
    load_error: Option<String>,
    #[allow(dead_code)]
    state: WatcherState,
}
enum WatcherState {
    /// No background task.
    Static {
        /// Keep sender alive so channel doesn't close
        _tx: watch::Sender<AppearanceConfig>,
    },
}
impl ConfigWatcher {
    /// Read the appearance file. A missing file gives the defaults.
    pub async fn start() -> io::Result<Self> {
        Self::start_static()
    }
    pub fn current(&self) -> watch::Ref<'_, AppearanceConfig> {
        self.rx.borrow()
    }
    /// Never completes.
    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.rx.changed().await
    }
    /// Why the appearance file did not load, with its path. `None` when it loaded or does not exist.
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }
    /// The appearance file to read: `pager.json`, or a legacy `pager.toml` when only that exists.
    fn pager_config_path() -> PathBuf {
        xai_grok_config::format::resolve_config_file(
            &crate::util::grok_home(),
            xai_grok_config::format::PAGER_STEM,
        )
    }
    fn start_static() -> io::Result<Self> {
        let (config, load_error) = match xai_grok_config::user_grok_home() {
            Some(_) => match load_appearance(&Self::pager_config_path()) {
                Ok(config) => (config, None),
                Err(e) => (AppearanceConfig::default(), Some(e)),
            },
            None => (AppearanceConfig::default(), None),
        };
        let (tx, rx) = watch::channel(config);
        Ok(Self {
            rx,
            load_error,
            state: WatcherState::Static { _tx: tx },
        })
    }
}
/// Load the appearance file at `path`. An absent file is the defaults.
fn load_appearance(path: &Path) -> Result<AppearanceConfig, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(AppearanceConfig::default()),
        Err(e) => {
            tracing::error!(file = %path.display(), error = %e, "could not read the appearance file; using the default appearance");
            return Err(format!("{}: {e}", path.display()));
        }
    };
    super::config::parse_appearance_config(path, &text)
        .map(AppearanceConfig::from)
        .map_err(|e| {
            tracing::error!(file = %path.display(), error = %e, "the appearance file does not parse; using the default appearance");
            format!("{}: {e}", path.display())
        })
}
/// True when a notify `Remove` fired but the pager.json slot is still present.
#[cfg(test)]
fn pager_slot_survived_remove(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}
/// Slot present *and* the follow dest still names a file. A dangling slot is
/// referent deletion (defaults), not a tmp+rename replacement.
#[cfg(test)]
fn pager_referent_still_present(path: &std::path::Path) -> bool {
    if !pager_slot_survived_remove(path) {
        return false;
    }
    match xai_grok_config::fs_atomic::resolve_atomic_destination(path) {
        Ok(dest) => std::fs::metadata(&dest)
            .map(|m| m.is_file())
            .unwrap_or(false),
        Err(_) => false,
    }
}
/// Accept the slot name, the bound dest name, or the legacy `pager.toml` that a migration renames.
/// Dest basename need not be `pager.json`.
#[cfg(test)]
fn pager_event_is_watched_config(
    event_paths: &[std::path::PathBuf],
    slot: &std::path::Path,
    dest: Option<&std::path::Path>,
) -> bool {
    let legacy = slot.with_extension("toml");
    event_paths.iter().any(|path| {
        let Some(name) = path.file_name() else {
            return false;
        };
        slot.file_name() == Some(name)
            || legacy.file_name() == Some(name)
            || dest.and_then(|d| d.file_name()) == Some(name)
    })
}
#[cfg(test)]
mod tests {
    use super::{
        load_appearance, pager_event_is_watched_config, pager_referent_still_present,
        pager_slot_survived_remove,
    };
    #[test]
    fn pager_reload_matches_differently_named_dest() {
        let slot = std::path::Path::new("/home/u/.grok/pager.json");
        let dest = std::path::Path::new("/dotfiles/appearance.json");
        assert!(pager_event_is_watched_config(
            &[dest.to_path_buf()],
            slot,
            Some(dest),
        ));
        assert!(pager_event_is_watched_config(
            &[slot.to_path_buf()],
            slot,
            Some(dest),
        ));
        assert!(
            pager_event_is_watched_config(
                &[std::path::PathBuf::from("/home/u/.grok/pager.toml")],
                slot,
                Some(dest),
            ),
            "the legacy name changes during a migration"
        );
        assert!(!pager_event_is_watched_config(
            &[std::path::PathBuf::from("/dotfiles/other.json")],
            slot,
            Some(dest),
        ));
    }
    #[test]
    fn load_reads_json_and_reports_a_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pager.json");
        assert!(
            !load_appearance(&path).unwrap().disable_plugins,
            "an absent file is the defaults"
        );
        std::fs::write(
            &path,
            r#"{"disable_plugins": true, "animation": {"fps": null}}"#,
        )
        .unwrap();
        assert!(load_appearance(&path).unwrap().disable_plugins);
        std::fs::write(&path, "{ broken").unwrap();
        let err = load_appearance(&path).unwrap_err();
        assert!(
            err.contains("pager.json"),
            "the error names the file: {err}"
        );
    }
    #[test]
    fn load_reads_a_legacy_toml_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pager.toml");
        std::fs::write(&path, "disable_plugins = true\n").unwrap();
        assert!(load_appearance(&path).unwrap().disable_plugins);
    }
    #[test]
    fn remove_reloads_when_symlink_slot_still_exists() {
        let dir = tempfile::tempdir().unwrap();
        let dest_dir = dir.path().join("dotfiles");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let dest = dest_dir.join("pager.json");
        std::fs::write(&dest, "{\"x\": 1}").unwrap();
        let slot = dir.path().join("pager.json");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dest, &slot).unwrap();
        #[cfg(not(unix))]
        std::fs::copy(&dest, &slot).unwrap();
        assert!(pager_slot_survived_remove(&slot));
        let tmp = dest_dir.join(".tmp");
        std::fs::write(&tmp, "{\"x\": 2}").unwrap();
        std::fs::rename(&tmp, &dest).unwrap();
        assert!(
            pager_slot_survived_remove(&slot),
            "slot must survive referent replace"
        );
        #[cfg(unix)]
        assert!(
            std::fs::symlink_metadata(&slot)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::remove_file(&slot).unwrap();
        assert!(
            !pager_slot_survived_remove(&slot),
            "gone slot is a real delete"
        );
    }
    #[test]
    fn dangling_slot_is_referent_deletion_not_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let dest_dir = dir.path().join("dotfiles");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let dest = dest_dir.join("pager.json");
        std::fs::write(&dest, "{\"x\": 1}").unwrap();
        let slot = dir.path().join("pager.json");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dest, &slot).unwrap();
        #[cfg(not(unix))]
        std::fs::copy(&dest, &slot).unwrap();
        assert!(pager_referent_still_present(&slot));
        std::fs::remove_file(&dest).unwrap();
        #[cfg(unix)]
        {
            assert!(
                pager_slot_survived_remove(&slot),
                "dangling symlink slot still exists"
            );
            assert!(
                !pager_referent_still_present(&slot),
                "deleted referent must not look like a replacement"
            );
        }
        #[cfg(not(unix))]
        assert!(!pager_slot_survived_remove(&slot));
    }
}
