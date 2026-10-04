//! Microphone capture on Linux via a subprocess recorder.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::mpsc as async_mpsc;

use super::pipe::{self, READ_CHUNK};
use crate::error::VoiceError;

/// Grace before a start is accepted (mirrors the `cpal` backend's open handshake).
const START_GRACE: Duration = Duration::from_millis(300);

/// Poll interval while waiting out [`START_GRACE`].
const START_POLL: Duration = Duration::from_millis(15);

/// Upper bound on the `pw-record --help` capability probe so a wedged binary can't hang `/voice` or Doctor.
const PW_HELP_TIMEOUT: Duration = Duration::from_secs(2);

/// A system audio recorder that can stream raw PCM16 mono to stdout.
#[derive(Clone, Copy, Debug)]
enum Recorder {
    /// PipeWire's `pw-record`.
    PwRecord,
    /// PulseAudio's `parec`.
    Parec,
    /// ALSA's `arecord` (alsa-utils).
    Arecord,
}

impl Recorder {
    fn program(self) -> &'static str {
        match self {
            Recorder::PwRecord => "pw-record",
            Recorder::Parec => "parec",
            Recorder::Arecord => "arecord",
        }
    }

    /// Args that emit signed 16-bit little-endian mono PCM at `rate` Hz to stdout.
    /// (`pw-record`/`pw-cat` and `arecord` take an explicit `-` stdout target; `parec` writes raw to stdout by default.)
    fn args(self, rate: u32) -> Vec<String> {
        let rate = rate.to_string();
        match self {
            Recorder::PwRecord => vec![
                // Without `--raw`, `pw-record` treats `--format`/`--rate`/`--channels`.
                "--raw".into(),
                "--rate".into(),
                rate,
                "--channels".into(),
                "1".into(),
                "--format".into(),
                "s16".into(),
                "-".into(),
            ],
            Recorder::Parec => vec![
                "--raw".into(),
                "--format=s16le".into(),
                format!("--rate={rate}"),
                "--channels=1".into(),
            ],
            Recorder::Arecord => vec![
                "-q".into(),
                "-t".into(),
                "raw".into(),
                "-f".into(),
                "S16_LE".into(),
                "-c".into(),
                "1".into(),
                "-r".into(),
                rate,
                "-".into(),
            ],
        }
    }
}

/// The order routes capture through the user's audio server rather than a raw ALSA `hw:` device. It stays as a last
/// resort so a misjudged or wedged probe (a false negative) can never block an otherwise-working `pw-record`. The spawn
/// in [`spawn_working_recorder`] is the source of truth; the probe only decides ordering.
fn candidate_recorders(
    available: impl Fn(&str) -> bool,
    pw_record_supports_raw: impl Fn() -> bool,
) -> Vec<Recorder> {
    let pw_available = available("pw-record");
    let pw_leads = pw_available && pw_record_supports_raw();

    let mut recorders = Vec::with_capacity(3);
    if pw_leads {
        recorders.push(Recorder::PwRecord);
    }
    if available("parec") {
        recorders.push(Recorder::Parec);
    }
    if available("arecord") {
        recorders.push(Recorder::Arecord);
    }
    if pw_available && !pw_leads {
        recorders.push(Recorder::PwRecord);
    }
    recorders
}

/// Whether `name` resolves to an executable regular file on any `PATH` entry (so a stray non-executable file can't shadow a working recorder).
fn binary_on_path(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        dir.join(name)
            .metadata()
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

/// Scans stdout and stderr since usage text lands on either. A probe that can't run, or outlives [`PW_HELP_TIMEOUT`],
/// counts as "no `--raw`". That only demotes `pw-record`, never blocks it (see [`candidate_recorders`]).
fn pw_record_supports_raw() -> bool {
    let mut cmd = Command::new("pw-record");
    cmd.arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    xai_tty_utils::detach_std_command(&mut cmd);
    #[allow(clippy::disallowed_methods)] // short-lived probe, bounded and reaped below
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };

    // Bound the probe so a wedged `pw-record --help` can't hang `/voice` or Doctor `--help` prints a few lines then exits.
    let deadline = Instant::now() + PW_HELP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => thread::sleep(START_POLL),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }

    let mut help = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut help);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_end(&mut help);
    }
    String::from_utf8_lossy(&help).contains("--raw")
}

/// [`candidate_recorders`], or a `VoiceError` naming what to install when `PATH` has none.
fn require_recorders(
    available: impl Fn(&str) -> bool,
    pw_record_supports_raw: impl Fn() -> bool,
) -> Result<Vec<Recorder>, VoiceError> {
    let recorders = candidate_recorders(&available, &pw_record_supports_raw);
    if recorders.is_empty() {
        return Err(VoiceError::Config(
            "no microphone recorder found on PATH: install pipewire (pw-record), \
             pulseaudio-utils (parec), or alsa-utils (arecord)"
                .into(),
        ));
    }
    Ok(recorders)
}

/// First recorder that spawns and survives [`START_GRACE`].
/// The walk also covers a `pw-record` the `--raw` probe misjudged, and failures are joined for the `/voice` toast.
fn spawn_working_recorder(sample_rate: u32) -> Result<(Recorder, Child), VoiceError> {
    let recorders = require_recorders(binary_on_path, pw_record_supports_raw)?;
    first_success(&recorders, |recorder| {
        try_spawn(recorder, sample_rate).inspect_err(|failure| {
            tracing::debug!(recorder = recorder.program(), %failure, "recorder failed to start");
        })
    })
    .map_err(|failures| {
        VoiceError::Config(format!("could not start a microphone recorder: {failures}"))
    })
}

/// First candidate `try_one` accepts, else all failures joined.
/// Separate from spawning so the fallback order is unit-testable without real recorders.
fn first_success<T>(
    candidates: &[Recorder],
    mut try_one: impl FnMut(Recorder) -> Result<T, String>,
) -> Result<(Recorder, T), String> {
    let mut failures = Vec::new();
    for &recorder in candidates {
        match try_one(recorder) {
            Ok(value) => return Ok((recorder, value)),
            Err(failure) => failures.push(failure),
        }
    }
    Err(failures.join("; "))
}

/// Spawn one recorder and confirm it survives [`START_GRACE`].
/// On early exit the error carries the child's stderr (e.g. `unrecognized option '--raw'`) for the toast and fallback.
fn try_spawn(recorder: Recorder, sample_rate: u32) -> Result<Child, String> {
    let mut cmd = Command::new(recorder.program());
    cmd.args(recorder.args(sample_rate))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // setsid detach via the sanctioned helper (workspace subprocess rule) The recorder writes to a pipe.
    xai_tty_utils::detach_std_command(&mut cmd);
    #[allow(clippy::disallowed_methods)] // recorder owned by the capture handle, killed on stop
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start {}: {e}", recorder.program()))?;

    match wait_through_grace(&mut child) {
        // `try_wait` in `wait_through_grace` already reaped the child.
        Ok(StartOutcome::ExitedEarly(status)) => {
            let mut stderr = String::new();
            if let Some(mut err) = child.stderr.take() {
                let _ = err.read_to_string(&mut stderr);
            }
            let stderr = stderr.trim();
            Err(format!(
                "{} exited immediately ({status}){}",
                recorder.program(),
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                },
            ))
        }
        Ok(StartOutcome::StillRunning) => Ok(child),
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(format!("failed to poll {}: {e}", recorder.program()))
        }
    }
}

/// Whether a just-spawned recorder is still running after [`START_GRACE`].
enum StartOutcome {
    /// Exited within the grace window: a failed start (rejected flag, no device, dead audio server).
    ExitedEarly(std::process::ExitStatus),
    /// Still alive at the deadline: started cleanly.
    StillRunning,
}

fn wait_through_grace(child: &mut Child) -> std::io::Result<StartOutcome> {
    let deadline = Instant::now() + START_GRACE;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(StartOutcome::ExitedEarly(status));
        }
        if Instant::now() >= deadline {
            return Ok(StartOutcome::StillRunning);
        }
        thread::sleep(START_POLL);
    }
}

/// Stop handle for the recorder subprocess (owns the child and reader thread).
pub use super::pipe::ChildCaptureHandle as CaptureHandle;

/// Spawn subprocess capture; PCM16 LE chunks are forwarded to `pcm_tx`.
pub fn spawn_pcm_capture(
    sample_rate: u32,
    pcm_tx: async_mpsc::Sender<Vec<u8>>,
) -> Result<CaptureHandle, VoiceError> {
    let (recorder, mut child) = spawn_working_recorder(sample_rate)?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(VoiceError::Config(format!(
            "{} produced no stdout",
            recorder.program()
        )));
    };

    pipe::drain_stderr(&mut child, recorder.program());

    let stop = Arc::new(AtomicBool::new(false));
    let stop_reader = Arc::clone(&stop);
    let device = recorder.program();
    let reader = thread::spawn(move || pipe::forward_pcm(stdout, pcm_tx, stop_reader, device));

    tracing::info!(
        recorder = recorder.program(),
        sample_rate,
        "voice capture stream (subprocess)"
    );

    Ok(CaptureHandle::new(child, stop, reader))
}

/// The recorder capture would try first, without recording ([`crate::probe::input_device_info`]). The
/// capability-aware ordering reports a too-old `pw-record` behind `parec`/`arecord` when those exist.
pub fn input_device_info() -> Result<crate::probe::InputDeviceInfo, VoiceError> {
    let recorders = require_recorders(binary_on_path, pw_record_supports_raw)?;
    // `require_recorders` returns a non-empty list on `Ok`; degrade to an error rather than panic if that ever changes
    let recorder = *recorders
        .first()
        .ok_or_else(|| VoiceError::Config("no microphone recorder found on PATH".into()))?;
    Ok(crate::probe::InputDeviceInfo {
        name: recorder.program().to_string(),
        detail: "system recorder; uses the audio server's default input".to_string(),
    })
}

/// Record mono PCM16 LE for a fixed duration (probe / diagnostics).
pub fn capture_pcm_for_duration(
    sample_rate: u32,
    seconds: u32,
) -> Result<(Vec<u8>, u32), VoiceError> {
    let (recorder, mut child) = spawn_working_recorder(sample_rate)?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(VoiceError::Config(format!(
            "{} produced no stdout",
            recorder.program()
        )));
    };
    pipe::drain_stderr(&mut child, recorder.program());

    let duration = Duration::from_secs(seconds.max(1) as u64);
    let deadline = Instant::now() + duration;

    // The read would otherwise run past the requested duration.
    let child = Arc::new(Mutex::new(child));
    let watchdog_child = Arc::clone(&child);
    thread::spawn(move || {
        thread::sleep(duration);
        let mut child = watchdog_child.lock();
        let _ = child.kill();
    });

    let mut pcm = Vec::new();
    let mut chunks = 0u32;
    let mut buf = vec![0u8; READ_CHUNK];
    // Small slack past the deadline: the kill's EOF (`Ok(0)`) is the intended exit; the time check is a backstop against a pathological pipe
    while Instant::now() < deadline + Duration::from_secs(1) {
        match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                chunks += 1;
                if let Some(read) = buf.get(..n) {
                    pcm.extend_from_slice(read);
                }
            }
            Err(_) => break,
        }
    }

    {
        let mut child = child.lock();
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok((pcm, chunks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arecord_args_are_raw_s16_mono() {
        let args = Recorder::Arecord.args(16_000);
        assert!(args.contains(&"S16_LE".to_string()));
        assert!(args.contains(&"raw".to_string()));
        // mono
        let c = args.iter().position(|a| a == "-c").unwrap();
        assert_eq!(args.get(c + 1).map(String::as_str), Some("1"));
        // rate
        let r = args.iter().position(|a| a == "-r").unwrap();
        assert_eq!(args.get(r + 1).map(String::as_str), Some("16000"));
        // stdout target
        assert_eq!(args.last().unwrap(), "-");
    }

    #[test]
    fn parec_and_pw_args_carry_rate_format_and_mono() {
        let parec = Recorder::Parec.args(24_000);
        assert!(parec.contains(&"--raw".to_string()));
        assert!(parec.contains(&"--format=s16le".to_string()));
        assert!(parec.contains(&"--rate=24000".to_string()));
        assert!(parec.contains(&"--channels=1".to_string()));

        let pw = Recorder::PwRecord.args(48_000);
        // Raw mode is required: without it pw-record wraps stdout in a libsndfile container.
        assert!(pw.contains(&"--raw".to_string()));
        let r = pw.iter().position(|a| a == "--rate").unwrap();
        assert_eq!(pw.get(r + 1).map(String::as_str), Some("48000"));
        let f = pw.iter().position(|a| a == "--format").unwrap();
        assert_eq!(pw.get(f + 1).map(String::as_str), Some("s16"));
        let c = pw.iter().position(|a| a == "--channels").unwrap();
        assert_eq!(pw.get(c + 1).map(String::as_str), Some("1"));
        assert_eq!(pw.last().unwrap(), "-"); // stdout target
    }

    fn config_message(err: VoiceError) -> String {
        match err {
            VoiceError::Config(message) => message,
            other => panic!("expected VoiceError::Config, got {other:?}"),
        }
    }

    #[test]
    fn recorder_preference_is_pipewire_then_pulse_then_alsa() {
        let all = candidate_recorders(|_| true, || true);
        assert!(matches!(
            all.as_slice(),
            [Recorder::PwRecord, Recorder::Parec, Recorder::Arecord]
        ));

        let no_pw = candidate_recorders(|p| p != "pw-record", || true);
        assert!(matches!(
            no_pw.as_slice(),
            [Recorder::Parec, Recorder::Arecord]
        ));

        let alsa_only = candidate_recorders(|p| p == "arecord", || true);
        assert!(matches!(alsa_only.as_slice(), [Recorder::Arecord]));
    }

    #[test]
    fn old_pipewire_is_demoted_below_pulse_and_alsa() {
        // pw-record present but `--raw` unsupported: it ranks last so parec is tried first.
        let all_present_no_raw = candidate_recorders(|_| true, || false);
        assert!(matches!(
            all_present_no_raw.as_slice(),
            [Recorder::Parec, Recorder::Arecord, Recorder::PwRecord]
        ));
    }

    #[test]
    fn sole_pipewire_stays_a_candidate_when_probe_says_no_raw() {
        // The only recorder on PATH is a pw-record the probe rejects.
        let only_pw = require_recorders(|p| p == "pw-record", || false).unwrap();
        assert!(matches!(only_pw.as_slice(), [Recorder::PwRecord]));
    }

    #[test]
    fn no_recorder_on_path_is_an_error() {
        let err = require_recorders(|_| false, || true).unwrap_err();
        assert!(config_message(err).contains("no microphone recorder"));
    }

    #[test]
    fn first_success_returns_first_ok_and_skips_later_candidates() {
        let candidates = [Recorder::PwRecord, Recorder::Parec, Recorder::Arecord];
        let (recorder, value) = first_success(&candidates, |r| match r {
            Recorder::PwRecord => Err("pw-record exited immediately".to_string()),
            Recorder::Parec => Ok(7u32),
            Recorder::Arecord => panic!("arecord should not be reached after parec succeeds"),
        })
        .expect("parec succeeds");
        assert!(matches!(recorder, Recorder::Parec));
        assert_eq!(value, 7);
    }

    #[test]
    fn first_success_reports_every_failure_when_all_fail() {
        let candidates = [Recorder::PwRecord, Recorder::Parec];
        let err = first_success::<()>(&candidates, |r| Err(format!("{} failed", r.program())))
            .unwrap_err();
        assert!(err.contains("pw-record failed"));
        assert!(err.contains("parec failed"));
    }
}
