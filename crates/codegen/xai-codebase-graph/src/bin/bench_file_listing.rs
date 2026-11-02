#![allow(clippy::cast_possible_truncation)] // Hits predate the gate

//! Benchmark for comparing git CLI vs gix file listing.

#![deny(clippy::indexing_slicing)]

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use xai_codebase_graph::LanguageRegistry;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path_str = if let Some(p) = args.get(1) {
        p.clone()
    } else if let Ok(p) = std::env::var("BENCH_REPO_ROOT").or_else(|_| std::env::var("XAI_ROOT")) {
        p
    } else {
        eprintln!("Usage: bench_file_listing <path> [cli|gix|gix-index|both]");
        eprintln!("Or set BENCH_REPO_ROOT to a large checkout to bench against");
        std::process::exit(1);
    };
    let mode = args.get(2).map(|s| s.as_str()).unwrap_or("both");

    let root_path = Path::new(&path_str);
    let registry = LanguageRegistry::new();

    match mode {
        "cli" => {
            let start = Instant::now();
            let files = collect_files_cli(root_path, &registry);
            let elapsed = start.elapsed();
            println!("CLI: {} files in {:?}", files.len(), elapsed);
        }
        "gix" => {
            let start = Instant::now();
            let files = collect_files_gix(root_path, &registry);
            let elapsed = start.elapsed();
            println!("gix: {} files in {:?}", files.len(), elapsed);
        }
        "gix-index" => {
            let start = Instant::now();
            let files = collect_files_gix_index_only(root_path, &registry);
            let elapsed = start.elapsed();
            println!("gix (index only): {} files in {:?}", files.len(), elapsed);
        }
        _ => {
            // Run all three methods multiple times for comparison
            println!("Benchmarking file listing for: {}", root_path.display());
            println!();

            let iterations = 5;

            // Warm up
            let _ = collect_files_cli(root_path, &registry);
            let _ = collect_files_gix(root_path, &registry);
            let _ = collect_files_gix_index_only(root_path, &registry);

            // CLI benchmark
            let mut cli_times = Vec::with_capacity(iterations);
            let mut cli_count = 0;
            for _ in 0..iterations {
                let start = Instant::now();
                let files = collect_files_cli(root_path, &registry);
                cli_times.push(start.elapsed());
                cli_count = files.len();
            }

            // gix benchmark (with untracked)
            let mut gix_times = Vec::with_capacity(iterations);
            let mut gix_count = 0;
            for _ in 0..iterations {
                let start = Instant::now();
                let files = collect_files_gix(root_path, &registry);
                gix_times.push(start.elapsed());
                gix_count = files.len();
            }

            // gix index-only benchmark
            let mut gix_index_times = Vec::with_capacity(iterations);
            let mut gix_index_count = 0;
            for _ in 0..iterations {
                let start = Instant::now();
                let files = collect_files_gix_index_only(root_path, &registry);
                gix_index_times.push(start.elapsed());
                gix_index_count = files.len();
            }

            // Print results
            let cli_avg = cli_times.iter().sum::<std::time::Duration>() / iterations as u32;
            let gix_avg = gix_times.iter().sum::<std::time::Duration>() / iterations as u32;
            let gix_index_avg =
                gix_index_times.iter().sum::<std::time::Duration>() / iterations as u32;

            println!("Results ({} iterations):", iterations);
            println!(
                "  CLI:               {} files, avg {:?}",
                cli_count, cli_avg
            );
            println!(
                "  gix (+ untracked):  {} files, avg {:?}",
                gix_count, gix_avg
            );
            println!(
                "  gix (index only):   {} files, avg {:?}",
                gix_index_count, gix_index_avg
            );
            println!();

            let speedup = cli_avg.as_secs_f64() / gix_index_avg.as_secs_f64();
            if speedup > 1.0 {
                println!("gix (index only) is {:.2}x faster than CLI", speedup);
            } else {
                println!("CLI is {:.2}x faster than gix (index only)", 1.0 / speedup);
            }
        }
    }
}

/// Collect files using git CLI (original approach)
fn collect_files_cli(root_path: &Path, registry: &LanguageRegistry) -> Vec<std::path::PathBuf> {
    // Get tracked files
    let tracked_output = Command::new("git")
        .args(["ls-files"])
        .current_dir(root_path)
        .output();

    let tracked_output = match tracked_output {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };

    // Get untracked files
    let untracked_output = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(root_path)
        .output()
        .ok();

    let tracked_str = String::from_utf8_lossy(&tracked_output.stdout);
    let mut files: Vec<std::path::PathBuf> = tracked_str
        .lines()
        .filter(|line| registry.is_supported(Path::new(line)))
        .map(|line| root_path.join(line))
        .collect();

    if let Some(output) = untracked_output
        && output.status.success()
    {
        let untracked_str = String::from_utf8_lossy(&output.stdout);
        let untracked_files: Vec<std::path::PathBuf> = untracked_str
            .lines()
            .filter(|line| registry.is_supported(Path::new(line)))
            .map(|line| root_path.join(line))
            .collect();
        files.extend(untracked_files);
    }

    files
}

/// Collect tracked files from the index, then untracked files from a directory walk.
fn collect_files_gix(root_path: &Path, registry: &LanguageRegistry) -> Vec<std::path::PathBuf> {
    let Ok(repo) = gix::open(root_path) else {
        return vec![];
    };
    let Ok(index) = repo.index_or_empty() else {
        return vec![];
    };

    let mut files = index_files(&index, root_path, registry);

    // Get untracked files. The walk skips ignored files and does not enter nested repositories.
    let mut collect = gix::dir::walk::delegate::Collect::default();
    let walked = repo.dirwalk_options().ok().and_then(|options| {
        let options = options.emit_untracked(gix::dir::walk::EmissionMode::Matching);
        let no_patterns: [&gix::bstr::BStr; 0] = [];
        repo.dirwalk(
            &index,
            no_patterns,
            &AtomicBool::new(false),
            options,
            &mut collect,
        )
        .ok()
    });
    if walked.is_some() {
        for (entry, _) in collect.unorded_entries {
            if entry.status == gix::dir::entry::Status::Untracked
                && let Ok(path_str) = std::str::from_utf8(&entry.rela_path)
                && registry.is_supported(Path::new(path_str))
            {
                files.push(root_path.join(path_str));
            }
        }
    }

    files
}

/// Collect files using the gix index only (tracked files only, no untracked)
fn collect_files_gix_index_only(
    root_path: &Path,
    registry: &LanguageRegistry,
) -> Vec<std::path::PathBuf> {
    let Ok(repo) = gix::open(root_path) else {
        return vec![];
    };
    let Ok(index) = repo.index_or_empty() else {
        return vec![];
    };
    index_files(&index, root_path, registry)
}

fn index_files(
    index: &gix::index::State,
    root_path: &Path,
    registry: &LanguageRegistry,
) -> Vec<std::path::PathBuf> {
    index
        .entries()
        .iter()
        .filter_map(|entry| {
            let path_str = std::str::from_utf8(entry.path(index)).ok()?;
            if registry.is_supported(Path::new(path_str)) {
                Some(root_path.join(path_str))
            } else {
                None
            }
        })
        .collect()
}
