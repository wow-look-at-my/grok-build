//! Gitignore integration for AGENTS.md and skills discovery.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};

pub fn build_gitignore(repo_root: Option<&Path>) -> Option<Gitignore> {
    // No repo root means no gitignore rules to apply
    let root = repo_root?;
    let mut builder = GitignoreBuilder::new(root);

    let repo_gitignore = root.join(".gitignore");
    if repo_gitignore.exists() {
        let _ = builder.add(&repo_gitignore);
    }

    if let Some(global_path) = get_global_gitignore_path()
        && global_path.exists()
    {
        let _ = builder.add(&global_path);
    }

    builder.build().ok()
}

pub fn is_ignored(path: &Path, gitignore: Option<&Gitignore>, repo_root: Option<&Path>) -> bool {
    let Some(gi) = gitignore else {
        return false;
    };
    xai_grok_tools::gitignore::is_ignored(gi, path, repo_root)
}

fn get_global_gitignore_path() -> Option<PathBuf> {
    configured_excludes_file().or_else(|| xai_dirs::home_dir().map(|home| home.join(".gitignore")))
}

/// `core.excludesFile` from the system and user git config files, with `~` expanded.
fn configured_excludes_file() -> Option<PathBuf> {
    use gix::config::{File, Source, file::Metadata, file::init, path::interpolate};

    let home = gix::path::env::home_dir();
    let files = [Source::System, Source::Git, Source::User]
        .into_iter()
        .filter_map(|source| {
            let path = source.storage_location(&mut gix::path::env::var)?;
            path.is_file().then(|| Metadata {
                path: Some(path.into_owned()),
                source,
                level: 0,
                trust: gix::sec::Trust::Full,
            })
        });
    let options = init::Options {
        includes: gix::config::file::includes::Options::follow_without_conditional(home.as_deref()),
        ..Default::default()
    };
    let config = File::from_paths_metadata(files, options).ok()??;
    let context = interpolate::Context {
        home_dir: home.as_deref(),
        ..Default::default()
    };
    config
        .path("core.excludesFile")?
        .interpolate(context)
        .ok()
        .map(std::borrow::Cow::into_owned)
}
