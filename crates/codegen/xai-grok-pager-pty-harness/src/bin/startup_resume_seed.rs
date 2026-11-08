// Seeds the GROK_HOME that `scripts/bench-startup.sh` times a `--continue`
// boot from.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use xai_grok_pager_pty_harness::{ContentController, PtyExitPoll, PtyHarness};

const WELCOME_SENTINEL: &str = "Quit";
const CTRL_Q: &[u8] = b"\x11";
const EXIT_WAIT: Duration = Duration::from_secs(60);

#[derive(Parser, Debug)]
#[command(
    about = "Seed a session with real turns and copy its GROK_HOME out, for a timed --continue boot.",
    long_about = None
)]
struct Args {
    /// Pager binary that drives the seed turns.
    #[arg(long)]
    binary: PathBuf,
    /// Workspace the session belongs to. The timed `--continue` boot must start here.
    #[arg(long)]
    cwd: PathBuf,
    /// Empty or missing directory that receives the seeded GROK_HOME.
    #[arg(long)]
    grok_home_out: PathBuf,
    #[arg(long, default_value_t = 50)]
    rows: u16,
    #[arg(long, default_value_t = 120)]
    cols: u16,
    #[arg(long, default_value_t = 12)]
    seed_turns: usize,
    /// Words per seeded agent response; the knob that sets the resume log size.
    #[arg(long, default_value_t = 400)]
    seed_response_words: usize,
    /// Deadline for each wait (welcome, turn idle).
    #[arg(long, default_value_t = 120_000)]
    timeout_ms: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.grok_home_out.exists()
        && std::fs::read_dir(&args.grok_home_out)
            .with_context(|| format!("read {}", args.grok_home_out.display()))?
            .next()
            .is_some()
    {
        bail!("{} is not empty", args.grok_home_out.display());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("build seeder runtime")?;
    runtime.block_on(run(&args))
}

async fn run(args: &Args) -> Result<()> {
    let content = ContentController::start()
        .await
        .context("start mock inference server")?;
    seed_session(args, &content, Duration::from_millis(args.timeout_ms))
        .context("seed the session to resume")?;
    // The sandbox is removed when `content` drops, so copy it out first.
    copy_dir(content.sandbox().grok_home(), &args.grok_home_out)
        .context("copy the seeded GROK_HOME out")?;
    eprintln!(
        "seeded {} turns into {}",
        args.seed_turns,
        args.grok_home_out.display()
    );
    Ok(())
}

fn seed_session(args: &Args, content: &ContentController, timeout: Duration) -> Result<()> {
    let mut seeder = PtyHarness::spawn_with_content_in_dir(
        &args.binary,
        args.rows,
        args.cols,
        content,
        &[],
        Some(&args.cwd),
    )
    .context("spawn the seeding pager")?;
    seeder
        .wait_for_text(WELCOME_SENTINEL, timeout)
        .context("seeding pager reached the welcome screen")?;

    for turn in 0..args.seed_turns {
        content.set_response(seed_response(turn, args.seed_response_words));
        seeder
            .inject_keys(format!("seed turn {turn}\r").as_bytes())
            .with_context(|| format!("submit seed turn {turn}"))?;
        seeder
            .wait_for_turn_idle(timeout)
            .with_context(|| format!("seed turn {turn} settled"))?;
    }
    quit_pager(&mut seeder).context("quit the seeding pager")
}

fn quit_pager(harness: &mut PtyHarness) -> Result<()> {
    harness.inject_keys(CTRL_Q)?;
    harness.update(Duration::from_millis(200));
    harness.inject_keys(CTRL_Q)?;
    match harness.wait_exit_code(EXIT_WAIT)? {
        PtyExitPoll::Running => harness.quit(),
        PtyExitPoll::Exited(_) | PtyExitPoll::PendingStatus => Ok(()),
    }
}

// Every word is unique to its turn, so no turns replay the same text.
fn seed_response(turn: usize, words: usize) -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(words * 10);
    for word in 0..words {
        if word > 0 {
            text.push(' ');
        }
        let _ = write!(text, "t{turn}w{word}");
    }
    text
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
    for entry in std::fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)
                .with_context(|| format!("copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}
