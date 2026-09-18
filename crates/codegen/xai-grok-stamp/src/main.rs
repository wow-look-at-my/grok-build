//! `xai-grok-stamp <binary> <version>` — writes the release number into a
//! linked binary, then reads it back.
//!
//! Exit code 1 on anything that would leave the binary reporting a version the
//! release does not carry. A publish whose binary and site disagree about the
//! number is the failure this guards.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path, version] = match args.as_slice() {
        [path, version] => [path.clone(), version.clone()],
        _ => {
            eprintln!("usage: xai-grok-stamp <binary> <version>");
            return ExitCode::FAILURE;
        }
    };

    let mut bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("cannot read {path}: {err}");
            return ExitCode::FAILURE;
        }
    };

    let at = match xai_grok_stamp::stamp(&mut bytes, &version) {
        Ok(at) => at,
        Err(err) => {
            eprintln!("cannot stamp {path}: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = std::fs::write(&path, &bytes) {
        eprintln!("cannot write {path}: {err}");
        return ExitCode::FAILURE;
    }

    // Reads the file back off disk, so a short write is caught here rather than
    // by a user running the shipped binary.
    match std::fs::read(&path).map(|written| xai_grok_stamp::read_stamp(&written)) {
        Ok(Ok(Some(got))) if got == version => {
            println!("stamped {path} at offset {at}: {got}");
            ExitCode::SUCCESS
        }
        Ok(Ok(got)) => {
            eprintln!("{path} reads back {got:?} after stamping {version}");
            ExitCode::FAILURE
        }
        Ok(Err(err)) => {
            eprintln!("cannot read the stamp back from {path}: {err}");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("cannot reopen {path}: {err}");
            ExitCode::FAILURE
        }
    }
}
