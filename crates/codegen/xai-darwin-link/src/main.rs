//! Stands in for the macOS linker during a Linux cross build.
//!
//! Compiling for `aarch64-apple-darwin` on Linux works. Linking does not: the
//! Apple frameworks and `libSystem` come from an SDK, and every cross-linker
//! that reads that SDK also rewrites the search paths rustc passes, which
//! drops each build script's own static library out of the link.
//!
//! So this binary records the link instead of performing it. rustc calls it as
//! the linker, and it copies every input the command names into one bundle and
//! writes the argument list beside them. A macOS runner replays that list with
//! its own `cc` (`ci/darwin-relink.sh`), which is a step of seconds against a
//! full build of tens of minutes.
//!
//! Paths inside the bundle are written as `@BUNDLE@`, and the output as
//! `@OUT@`, because the replay host mounts the bundle somewhere else.

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Names the directory the bundle is written to.
const BUNDLE_ENV: &str = "GROK_DARWIN_LINK_BUNDLE";
/// Stands for the bundle directory on the replay host.
const BUNDLE_TOKEN: &str = "@BUNDLE@";
/// Stands for the output path on the replay host.
const OUT_TOKEN: &str = "@OUT@";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xai-darwin-link: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let bundle = PathBuf::from(
        std::env::var_os(BUNDLE_ENV)
            .ok_or_else(|| format!("{BUNDLE_ENV} is unset, so there is nowhere to record"))?,
    );
    let args = expand_response_files(std::env::args_os().skip(1).collect())?;
    let recorded = record(&args, &bundle)?;
    // rustc reads the output file after the linker returns. An empty file is
    // enough: nothing on this host runs it, and the replay writes the real one.
    write_file(&recorded.output, b"")?;
    Ok(())
}

/// What the recording produced.
#[derive(Debug)]
struct Recorded {
    /// The path rustc asked the linker to write.
    output: PathBuf,
}

/// Read `@file` arguments into the list they stand for.
///
/// rustc falls back to a response file when a command line grows past what the
/// system takes, and this link names hundreds of rlibs.
fn expand_response_files(args: Vec<OsString>) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        let arg = arg
            .into_string()
            .map_err(|a| format!("linker argument is not UTF-8: {}", a.to_string_lossy()))?;
        let Some(path) = arg.strip_prefix('@') else {
            out.push(arg);
            continue;
        };
        let body = std::fs::read_to_string(path)
            .map_err(|e| format!("could not read the response file '{path}': {e}"))?;
        out.extend(body.lines().map(unquote_response_arg));
    }
    Ok(out)
}

/// Undo the quoting rustc writes into a response file.
fn unquote_response_arg(line: &str) -> String {
    let line = line.trim_end_matches('\r');
    let Some(inner) = line.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return line.to_string();
    };
    inner.replace("\\\"", "\"").replace("\\\\", "\\")
}

/// Copy every input the command names into the bundle and write the argument
/// list beside them.
fn record(args: &[String], bundle: &Path) -> Result<Recorded, String> {
    let inputs = bundle.join("inputs");
    let libs = bundle.join("libs");
    create_dir(&inputs)?;
    create_dir(&libs)?;
    let mut out_args: Vec<String> = Vec::with_capacity(args.len() + 2);
    let mut output: Option<PathBuf> = None;
    let mut copied_libs = HashSet::new();
    let mut seq = 0usize;
    let mut i = 0usize;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        // The output path: the replay host names its own.
        if arg == "-o" {
            let value = args.get(i).ok_or("-o has no value")?;
            i += 1;
            output = Some(PathBuf::from(value));
            out_args.push("-o".to_string());
            out_args.push(OUT_TOKEN.to_string());
            continue;
        }
        // A search directory: the libraries inside it are what matter, and
        // they are collected into one directory the replay can name.
        if let Some(dir) = search_dir(arg, args.get(i)) {
            if arg == "-L" {
                i += 1;
            }
            copy_libraries(&dir, &libs, &mut copied_libs)?;
            continue;
        }
        // An input file: copy it, because rustc deletes its own temporaries as
        // soon as this process returns.
        let path = Path::new(arg);
        if path.is_file() {
            seq += 1;
            let name = format!("{seq:05}-{}", file_name(path));
            std::fs::copy(path, inputs.join(&name))
                .map_err(|e| format!("could not copy the linker input '{arg}': {e}"))?;
            out_args.push(format!("{BUNDLE_TOKEN}/inputs/{name}"));
            continue;
        }
        out_args.push(arg.to_string());
    }
    out_args.push("-L".to_string());
    out_args.push(format!("{BUNDLE_TOKEN}/libs"));
    let output = output.ok_or("the link command names no output (-o)")?;
    // One argument per line, and a trailing newline: a reader that stops at the
    // last newline would otherwise drop the final argument.
    write_file(
        &bundle.join("args"),
        format!("{}\n", out_args.join("\n")).as_bytes(),
    )?;
    write_file(&bundle.join("output-name"), file_name(&output).as_bytes())?;
    Ok(Recorded { output })
}

/// The directory a `-L` argument names, in either spelling.
fn search_dir(arg: &str, next: Option<&String>) -> Option<PathBuf> {
    if arg == "-L" {
        return next.map(PathBuf::from);
    }
    arg.strip_prefix("-L").map(PathBuf::from)
}

/// Copy the static and dynamic libraries out of one search directory.
///
/// A name already copied is kept: the first `-L` wins, which is the order the
/// linker itself searches.
fn copy_libraries(
    dir: &Path,
    libs: &Path,
    copied: &mut HashSet<String>,
) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // A search path that does not exist is one the real link also skips.
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = file_name(&path);
        if !is_library(&name) {
            continue;
        }
        if !copied.insert(name.clone()) {
            continue;
        }
        std::fs::copy(&path, libs.join(&name))
            .map_err(|e| format!("could not copy the library '{}': {e}", path.display()))?;
    }
    Ok(())
}

/// Whether a file in a search directory is a library the link can name.
fn is_library(name: &str) -> bool {
    name.starts_with("lib") && (name.ends_with(".a") || name.ends_with(".dylib"))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string())
}

fn create_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|e| format!("could not create '{}': {e}", path.display()))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = std::fs::File::create(path)
        .map_err(|e| format!("could not create '{}': {e}", path.display()))?;
    file.write_all(bytes)
        .map_err(|e| format!("could not write '{}': {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "xai-darwin-link-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn read_args(bundle: &Path) -> Vec<String> {
        std::fs::read_to_string(bundle.join("args"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The whole contract in one link: inputs are copied, `-L` directories
    /// collapse into the bundle's own, the output is a token, and every flag
    /// the linker needs survives untouched.
    #[test]
    fn a_recorded_link_is_replayable_from_the_bundle_alone() {
        let work = temp_dir("record");
        let objects = work.join("objects");
        let search = work.join("search");
        std::fs::create_dir_all(&objects).unwrap();
        std::fs::create_dir_all(&search).unwrap();
        let object = objects.join("symbols.o");
        std::fs::write(&object, b"object").unwrap();
        std::fs::write(search.join("libaws_lc.a"), b"archive").unwrap();
        std::fs::write(search.join("notes.txt"), b"not a library").unwrap();
        let bundle = work.join("bundle");
        let out = work.join("grok");
        let args: Vec<String> = vec![
            object.display().to_string(),
            "-L".into(),
            search.display().to_string(),
            "-laws_lc".into(),
            "-framework".into(),
            "AppKit".into(),
            "-o".into(),
            out.display().to_string(),
            "-Wl,-dead_strip".into(),
        ];
        let recorded = record(&args, &bundle).expect("record");
        assert_eq!(recorded.output, out);

        let got = read_args(&bundle);
        assert_eq!(
            got,
            vec![
                "@BUNDLE@/inputs/00001-symbols.o",
                "-laws_lc",
                "-framework",
                "AppKit",
                "-o",
                "@OUT@",
                "-Wl,-dead_strip",
                "-L",
                "@BUNDLE@/libs",
            ],
            "recorded args"
        );
        assert!(bundle.join("inputs/00001-symbols.o").is_file());
        assert!(bundle.join("libs/libaws_lc.a").is_file());
        assert!(
            !bundle.join("libs/notes.txt").exists(),
            "only libraries are collected"
        );
        assert_eq!(
            std::fs::read_to_string(bundle.join("output-name")).unwrap(),
            "grok"
        );
        let _ = std::fs::remove_dir_all(&work);
    }

    /// rustc deletes its temporary object directory the moment the linker
    /// returns, so a bundle that only referenced those paths would be empty by
    /// the time the macOS job read it.
    #[test]
    fn inputs_are_copied_rather_than_referenced() {
        let work = temp_dir("copy");
        let object = work.join("temp.o");
        std::fs::write(&object, b"object").unwrap();
        let bundle = work.join("bundle");
        let args = vec![
            object.display().to_string(),
            "-o".into(),
            work.join("grok").display().to_string(),
        ];
        record(&args, &bundle).expect("record");
        std::fs::remove_file(&object).unwrap();
        assert_eq!(
            std::fs::read(bundle.join("inputs/00001-temp.o")).unwrap(),
            b"object",
            "the bundle must survive the original being deleted"
        );
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn the_inline_spelling_of_a_search_path_is_read_too() {
        let work = temp_dir("inline-l");
        let search = work.join("search");
        std::fs::create_dir_all(&search).unwrap();
        std::fs::write(search.join("libring.a"), b"archive").unwrap();
        let bundle = work.join("bundle");
        let args = vec![
            format!("-L{}", search.display()),
            "-o".into(),
            work.join("grok").display().to_string(),
        ];
        record(&args, &bundle).expect("record");
        assert!(bundle.join("libs/libring.a").is_file());
        assert_eq!(read_args(&bundle), vec!["-o", "@OUT@", "-L", "@BUNDLE@/libs"]);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn a_response_file_expands_into_its_arguments() {
        let work = temp_dir("response");
        let file = work.join("args.txt");
        std::fs::write(&file, "-lobjc\n\"-Wl,-dead_strip\"\n").unwrap();
        let expanded = expand_response_files(vec![
            OsString::from("-ObjC"),
            OsString::from(format!("@{}", file.display())),
        ])
        .expect("expand");
        assert_eq!(expanded, vec!["-ObjC", "-lobjc", "-Wl,-dead_strip"]);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn a_link_with_no_output_is_refused() {
        let work = temp_dir("no-out");
        let err = record(&["-lobjc".to_string()], &work.join("bundle")).unwrap_err();
        assert!(err.contains("names no output"), "{err}");
        let _ = std::fs::remove_dir_all(&work);
    }
}
