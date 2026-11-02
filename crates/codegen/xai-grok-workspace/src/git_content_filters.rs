//! Repo-local content-filter pins for unattended git.

use std::collections::BTreeSet;
use std::path::Path;

use gix::bstr::ByteSlice;

pub(crate) fn filter_command_driver(name: &str) -> Option<&str> {
    // `get` is None on a non-char boundary; a byte slice would panic.
    let head = name.get(..7)?;
    if !head.eq_ignore_ascii_case("filter.") {
        return None;
    }
    let rest = name.get(7..)?;
    let (driver, prop) = rest.rsplit_once('.')?;
    if !driver.is_empty()
        && (prop.eq_ignore_ascii_case("clean")
            || prop.eq_ignore_ascii_case("smudge")
            || prop.eq_ignore_ascii_case("process"))
    {
        Some(driver)
    } else {
        None
    }
}

fn filter_driver_name_is_pinnable(driver: &str) -> bool {
    let mut chars = driver.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn local_includeif_unsupported(name: &str) -> bool {
    let Some(head) = name.get(..10) else {
        return false;
    };
    if !head.eq_ignore_ascii_case("includeif.") {
        return false;
    }
    let condition = name.get(10..).unwrap_or("").split(':').next().unwrap_or("");
    !matches!(
        condition.to_ascii_lowercase().as_str(),
        "gitdir" | "gitdir/i" | "onbranch"
    )
}

fn path_unreadable(path: &Path) -> bool {
    // Directories open on Linux, so require a readable regular file after following symlinks.
    match std::fs::File::open(path) {
        Ok(f) => match f.metadata() {
            Ok(meta) => !meta.is_file(),
            Err(_) => true,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

/// Local/worktree only (include/includeIf). `None` on read errors; empty if not a repo.
pub(crate) fn read_local_git_config_entries(cwd: &Path) -> Option<Vec<(String, String)>> {
    let repo = match crate::git_repo::discover_with_filter_drivers(cwd) {
        Ok(repo) => repo,
        Err(crate::git_repo::DiscoverError::NotARepo(_)) => return Some(Vec::new()),
        Err(crate::git_repo::DiscoverError::Failed(_)) => return None,
    };
    // The config snapshot still holds the global levels when the local file is unreadable.
    let git_dir = repo.git_dir();
    let common = repo.common_dir();
    if path_unreadable(&common.join("config"))
        || path_unreadable(&git_dir.join("config"))
        || path_unreadable(&git_dir.join("config.worktree"))
    {
        return None;
    }
    let config = repo.config_snapshot();
    let mut out = Vec::new();
    for section in config.plumbing().sections() {
        if !matches!(
            section.meta().source,
            gix::config::Source::Local | gix::config::Source::Worktree
        ) {
            continue;
        }
        let header = section.header();
        // A non-UTF-8 name cannot be matched. Dropping that entry would hide a filter Git can still run.
        let section_name = header.name().to_str().ok()?.to_ascii_lowercase();
        let prefix = match header.subsection_name() {
            Some(sub) => format!("{section_name}.{}", sub.to_str().ok()?),
            None => section_name,
        };
        for (key, value) in section_entries(section).ok()? {
            let name = format!("{prefix}.{}", key.to_ascii_lowercase());
            // Only these `includeIf` conditions are read here.
            if local_includeif_unsupported(&name) {
                return None;
            }
            // A bare key is boolean true.
            let value = match value {
                Some(value) => match value.to_str() {
                    Ok(value) => value.to_owned(),
                    Err(_) if filter_command_driver(&name).is_some() => return None,
                    Err(_) => continue,
                },
                None if filter_command_driver(&name).is_some() => return None,
                None => "true".to_owned(),
            };
            out.push((name, value));
        }
    }
    Some(out)
}

/// Every key of `section` in file order, with `None` for a bare key that has no `=`.
fn section_entries(
    section: &gix::config::file::Section<'_>,
) -> Result<Vec<(String, Option<gix::bstr::BString>)>, gix::config::parse::Error> {
    use gix::config::parse::Event;
    let text = section.to_bstring();
    let events = gix::config::parse::Events::from_bytes(text.as_ref(), None)?;
    let mut out = Vec::new();
    let mut pending: Option<(String, bool)> = None;
    let mut partial = gix::bstr::BString::default();
    for event in events.into_iter() {
        match event {
            Event::SectionValueName(key) => {
                pending = Some((key.to_string(), false));
                partial.clear();
            }
            Event::KeyValueSeparator => {
                if let Some((_, explicit)) = pending.as_mut() {
                    *explicit = true;
                }
            }
            Event::Value(value) => {
                if let Some((key, explicit)) = pending.take() {
                    let value = explicit
                        .then(|| gix::config::value::normalize_bstr(value.as_ref()).into_owned());
                    out.push((key, value));
                }
            }
            Event::ValueNotDone(value) => partial.extend_from_slice(value.as_ref()),
            Event::ValueDone(value) => {
                partial.extend_from_slice(value.as_ref());
                if let Some((key, _)) = pending.take() {
                    let joined = std::mem::take(&mut partial);
                    out.push((
                        key,
                        Some(gix::config::value::normalize_bstring(joined).into_owned()),
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

pub(crate) fn local_content_filter_drivers(cwd: &Path) -> Option<Vec<String>> {
    let entries = read_local_git_config_entries(cwd)?;
    let mut drivers = BTreeSet::new();
    for (name, value) in entries {
        if value.trim().is_empty() {
            continue;
        }
        let Some(driver) = filter_command_driver(&name) else {
            continue;
        };
        // Hostile names cannot be expressed as `git -c filter.<name>.*` (splits on first `=`).
        if !filter_driver_name_is_pinnable(driver) {
            return None;
        }
        drivers.insert(driver.to_owned());
    }
    Some(drivers.into_iter().collect())
}

pub(crate) fn content_filter_config_pins(cwd: &Path) -> Option<Vec<String>> {
    let drivers = local_content_filter_drivers(cwd)?;
    let mut pins = Vec::with_capacity(drivers.len() * 4);
    for driver in drivers {
        // required=false avoids status erroring into still invoking a required filter.
        pins.push(format!("filter.{driver}.clean="));
        pins.push(format!("filter.{driver}.smudge="));
        pins.push(format!("filter.{driver}.process="));
        pins.push(format!("filter.{driver}.required=false"));
    }
    Some(pins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_command_driver_does_not_panic_on_multibyte() {
        assert!(filter_command_driver("alias.éxxx").is_none());
    }

    #[test]
    fn pins_unsupported_includeif_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        crate::test_git::init(tmp.path());
        std::fs::write(
            tmp.path().join(".git/config"),
            "[core]\n\trepositoryformatversion = 0\n\
             [includeIf \"hasconfig:remote.*.url:https://example.com/**\"]\n\
             \tpath = ../filters.gitconfig\n",
        )
        .unwrap();
        assert!(content_filter_config_pins(tmp.path()).is_none());
    }

    #[test]
    fn pins_valueless_boolean_key_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        crate::test_git::init(tmp.path());
        std::fs::write(
            tmp.path().join(".git/config"),
            "[core]\n\trepositoryformatversion = 0\n\tignorecase\n\
             [filter \"lfs\"]\n\tprocess = git-lfs filter-process\n",
        )
        .unwrap();
        let pins = content_filter_config_pins(tmp.path()).expect("bare boolean is readable");
        assert!(pins.iter().any(|p| p == "filter.lfs.process="), "{pins:?}");
    }

    #[test]
    fn pins_valueless_filter_command_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        crate::test_git::init(tmp.path());
        std::fs::write(
            tmp.path().join(".git/config"),
            "[core]\n\trepositoryformatversion = 0\n\
             [filter \"pwn\"]\n\tclean\n",
        )
        .unwrap();
        assert!(content_filter_config_pins(tmp.path()).is_none());
    }

    #[test]
    fn pins_unreadable_config_are_none() {
        let tmp = tempfile::tempdir().unwrap();
        crate::test_git::init(tmp.path());
        let cfg = tmp.path().join(".git/config");
        std::fs::remove_file(&cfg).unwrap();
        std::fs::create_dir(&cfg).unwrap();
        assert!(content_filter_config_pins(tmp.path()).is_none());
    }

    #[test]
    fn pins_safe_lfs_process_driver() {
        let tmp = tempfile::tempdir().unwrap();
        crate::test_git::init(tmp.path());
        std::fs::write(
            tmp.path().join(".git/config"),
            "[core]\n\trepositoryformatversion = 0\n\
             [filter \"lfs\"]\n\tprocess = git-lfs filter-process\n",
        )
        .unwrap();
        let pins = content_filter_config_pins(tmp.path()).expect("readable");
        assert_eq!(
            pins,
            vec![
                "filter.lfs.clean=".to_owned(),
                "filter.lfs.smudge=".to_owned(),
                "filter.lfs.process=".to_owned(),
                "filter.lfs.required=false".to_owned(),
            ]
        );
    }

    #[test]
    fn pins_non_utf8_filter_value_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        crate::test_git::init(tmp.path());
        let mut cfg =
            b"[core]\n\trepositoryformatversion = 0\n[filter \"pwn\"]\n\tclean = ".to_vec();
        cfg.extend_from_slice(&[0xff, 0xfe]);
        cfg.push(b'\n');
        std::fs::write(tmp.path().join(".git/config"), cfg).unwrap();
        assert!(content_filter_config_pins(tmp.path()).is_none());
    }

    #[test]
    fn pins_mixed_case_driver_keeps_spelling() {
        let tmp = tempfile::tempdir().unwrap();
        crate::test_git::init(tmp.path());
        std::fs::write(
            tmp.path().join(".git/config"),
            "[core]\n\trepositoryformatversion = 0\n\
             [filter \"Pwn\"]\n\tclean = /tmp/pwn\n",
        )
        .unwrap();
        let pins = content_filter_config_pins(tmp.path()).expect("readable");
        assert!(pins.iter().any(|p| p == "filter.Pwn.clean="), "{pins:?}");
        assert!(!pins.iter().any(|p| p.starts_with("filter.pwn.")));
    }

    #[test]
    fn pins_hostile_driver_names_fail_closed() {
        for section in ["pwn=x", "pwn x", "pwn.x"] {
            let tmp = tempfile::tempdir().unwrap();
            crate::test_git::init(tmp.path());
            std::fs::write(
                tmp.path().join(".git/config"),
                format!(
                    "[core]\n\trepositoryformatversion = 0\n\
                     [filter \"{section}\"]\n\tclean = /tmp/pwn\n"
                ),
            )
            .unwrap();
            assert!(
                content_filter_config_pins(tmp.path()).is_none(),
                "section={section:?}"
            );
        }
    }
}
