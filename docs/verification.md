# Verification

- `cargo fmt --all` before pushing. The `fmt` job in `ci.yml` runs `cargo fmt --all --check` and fails the build on any unformatted file. It is its own job, because rustfmt parses and never compiles. It answers in seconds rather than waiting on the cold build.
- `cargo check -p <touched-crate>` before pushing.
- `Lint (workspace)` is `--lib --bins`, so it compiles no `#[cfg(test)]` module, and `Build the dependencies` is `cargo test --locked --workspace --no-run`, which compiles all of them. An import clippy calls unused can still be the one a test needs. Removing `std::sync::Mutex` from `xai-grok-telemetry`'s `debug_log.rs` passed the lint and broke the dependency build steps earlier in the same job. Mirror both commands, not whichever one was red.
- `cargo test -p <touched-crate>` for the crate you changed.
- Prefer committing real tests that drive the shipped code (not mocks of the unit under test, not hand-built expected objects).
- **A web session cannot link the workspace.** `target/` reaches ~16 GB after a `cargo check` of the pager, against a ~12 GB session disk allowance. So `cargo build -p xai-grok-pager-bin` runs the container out of space. Check the crate, run that crate's tests, push, and let CI produce the binary.
- **Do not run `cargo test -p xai-grok-shell` in a web session.** Its test binary runs the disk out the same way. Run `cargo check -p xai-grok-shell --tests`, push, and read the shell tests' result from CI's `Build & test`.
- `protoc` is missing from the image and the `bin/protoc` dotslash shim cannot run either, so any build that reaches `xai-grok-tools-api` dies in its build script. Run `apt-get install -y protobuf-compiler` first.
- `mold` is missing too, and the repo's cargo config passes `-fuse-ld=mold`. Every build script then fails to link with `collect2: fatal error: cannot find 'ld'`, on `proc-macro2` and `libc` — which reads as a broken C toolchain and is not one. Run `apt-get install -y mold`.
- **A local clippy run cannot measure the whole denied set, on macOS.** Code behind `#[cfg(target_os = "linux")]` is never compiled here, so clippy never reads it and a lint denied at workspace level reports nothing for it. The parent-death checks in `xai-tty-utils` and `xai-grok-workspace` are that shape (`getppid()` returning a `pid_t`, compared against a captured `u32`), and `clippy::cast_sign_loss` rejected them in CI while `cargo clippy --workspace --lib --bins` exited 0 on the Mac. A per-crate exception count generated on a Mac is therefore a floor, not a total. A HOST clippy run is what that floor comes from - `--target x86_64-unknown-linux-gnu` reads those files and is a complete measurement:

```
CC_x86_64_unknown_linux_gnu=ci/zig-linux-cc.sh \
  cargo clippy --locked --workspace --lib --bins --keep-going \
    --target x86_64-unknown-linux-gnu
```

Cross-clippy links nothing, but ring and aws-lc compile C in their build scripts and need a compiler for the TARGET. `ci/zig-linux-cc.sh` is `ci/zig-target-cc.sh`'s trick pointed at Linux: zig ships the cross toolchain. Cc-rs's own target flags are dropped because zig spells them differently. Run it through rustup's pinned 1.94.1 (`PATH=/opt/homebrew/opt/rustup/bin:$PATH`): a Homebrew `cargo` is a different rustfmt and a different clippy. It reported a file clean that CI's Rustfmt job then rejected over import order.
- `Lint (workspace)` passes `--keep-going` for that reason. A denied lint is a compile error, which stops the crate that hit it and leaves every crate behind it unlinted. On a graph where several crates have Linux-only code, one CI cycle per crate is the alternative.
- The runner's clippy is not the clippy on a development machine, and the lint tables differ between the two. Where a lint's verdict matters, read CI rather than concluding from a local run.
