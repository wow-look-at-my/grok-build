//! Sliding-window output-rate meter and the floor policy that acts on it.
//!
//! One meter serves both consumers: the tokens/sec the client renders and the
//! mid-stream abort that resamples a collapsed response. A second
//! implementation for the display would let the number on screen disagree with
//! the number the gate acted on.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Trailing window the rate is measured over.
pub const DEFAULT_WINDOW_SECS: u64 = 10;

/// Shortest span the meter will divide by. Below it a single chunk's arrival
/// jitter dominates and the quotient is noise, not a rate.
pub const MIN_DISPLAY_SPAN: Duration = Duration::from_millis(1500);

/// How long the rate must stay under the floor before the request is
/// reissued.
pub const DEFAULT_SUSTAINED_SECS: u64 = 10;

/// A floor on output tokens/sec. A response whose rate stays under it for
/// `sustained_secs` is abandoned and reissued.
///
/// Some inference engines drop from 100+ tok/s to under 5 tok/s mid-response
/// and stay there until the request ends. The stream is healthy by every other
/// measure — chunks keep arriving, so the idle timeout never fires — and the
/// only cure is a new request.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct OutputRateFloorPolicy {
    /// Reissue below this many output tokens per second. Zero disables the
    /// gate.
    #[serde(default)]
    pub min_tokens_per_sec: f64,
    /// Trailing window the rate is measured over.
    #[serde(default = "default_window_secs")]
    pub window_secs: u64,
    /// How long the measured rate must stay under the floor before the
    /// request is reissued. A brief dip — a pause to think, a slow tool-call
    /// argument — is not a collapsed engine, and reissuing over one throws
    /// away good generation.
    #[serde(default = "default_sustained_secs")]
    pub sustained_secs: u64,
    /// Reissue budget per model call before the response is accepted at
    /// whatever rate it runs at.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Reissue an attempt that has produced no output this many seconds
    /// after the request was sent. Zero disables the check. It shares
    /// `max_retries` with the floor.
    #[serde(default)]
    pub ttft_timeout_secs: u64,
}

fn default_window_secs() -> u64 {
    DEFAULT_WINDOW_SECS
}

fn default_sustained_secs() -> u64 {
    DEFAULT_SUSTAINED_SECS
}

fn default_max_retries() -> u32 {
    OutputRateFloorPolicy::DEFAULT_MAX_RETRIES
}

impl Default for OutputRateFloorPolicy {
    fn default() -> Self {
        Self {
            min_tokens_per_sec: 0.0,
            window_secs: DEFAULT_WINDOW_SECS,
            sustained_secs: DEFAULT_SUSTAINED_SECS,
            max_retries: Self::DEFAULT_MAX_RETRIES,
            ttft_timeout_secs: 0,
        }
    }
}

impl OutputRateFloorPolicy {
    /// The meter reports no rate until [`MIN_DISPLAY_SPAN`] of stream, so a
    /// shorter window never reads and the gate never fires.
    pub const MIN_WINDOW_SECS: u64 = 2;
    pub const MIN_SUSTAINED_SECS: u64 = 1;
    pub const DEFAULT_MAX_RETRIES: u32 = 2;
    /// A `max_retries` of this value never runs out.
    pub const UNLIMITED_RETRIES: u32 = u32::MAX;

    /// The policy with each tunable raised to the least value that still
    /// works. There is no upper limit.
    pub fn clamped(self) -> Self {
        let min_tokens_per_sec = if self.min_tokens_per_sec.is_finite() {
            self.min_tokens_per_sec.max(0.0)
        } else {
            0.0
        };
        Self {
            min_tokens_per_sec,
            window_secs: self.window_secs.max(Self::MIN_WINDOW_SECS),
            sustained_secs: self.sustained_secs.max(Self::MIN_SUSTAINED_SECS),
            ..self
        }
    }

    /// Whether `spent` reissues leave any of this policy's budget.
    pub fn has_retries_left(&self, spent: u32) -> bool {
        self.max_retries == Self::UNLIMITED_RETRIES || spent < self.max_retries
    }

    /// Whether this policy gates anything: the rate floor, the
    /// time-to-first-token limit, or both.
    pub fn is_armed(&self) -> bool {
        self.floor_armed() || self.ttft_armed()
    }

    /// Whether the rate floor is set. A zero or negative floor is its off
    /// switch.
    pub fn floor_armed(&self) -> bool {
        self.min_tokens_per_sec > 0.0
    }

    /// Whether the time-to-first-token limit is set.
    pub fn ttft_armed(&self) -> bool {
        self.ttft_timeout_secs > 0
    }

    /// The time-to-first-token limit, when one is set.
    pub fn ttft_timeout(&self) -> Option<Duration> {
        self.ttft_armed()
            .then(|| Duration::from_secs(self.ttft_timeout_secs))
    }

    /// The measurement window as a `Duration`.
    pub fn window(&self) -> Duration {
        Duration::from_secs(self.window_secs.max(1))
    }

    /// The sustained-breach duration as a `Duration`.
    pub fn sustained(&self) -> Duration {
        Duration::from_secs(self.sustained_secs.max(1))
    }
}

/// How the measured rate stands against the configured floor. One reduction
/// so the indicator's color, the slowdown log and the abort cannot disagree
/// about what "slow" means.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputRateHealth {
    /// No floor configured, or the rate is comfortably above it.
    Healthy,
    /// Above the floor but inside [`NEAR_FLOOR_FACTOR`] of it — the reading a
    /// collapse passes through on its way down.
    Near,
    /// Under the floor. Sustained for a whole window, this is what the gate
    /// reissues the request over.
    Slow,
}

/// A rate under `floor * NEAR_FLOOR_FACTOR` is near the floor. Wide enough
/// that a real collapse shows amber before it shows red, narrow enough that
/// an ordinary healthy stream never does.
pub const NEAR_FLOOR_FACTOR: f64 = 1.5;

/// Classify `tokens_per_sec` against `floor`. An absent or unarmed floor is
/// always [`OutputRateHealth::Healthy`]: nothing was asked for, so nothing is
/// wrong.
pub fn classify_rate(tokens_per_sec: f64, floor: Option<f64>) -> OutputRateHealth {
    let Some(floor) = floor.filter(|f| *f > 0.0) else {
        return OutputRateHealth::Healthy;
    };
    if tokens_per_sec < floor {
        OutputRateHealth::Slow
    } else if tokens_per_sec < floor * NEAR_FLOOR_FACTOR {
        OutputRateHealth::Near
    } else {
        OutputRateHealth::Healthy
    }
}

/// Output bytes arriving over time, reduced to a trailing-window token rate.
///
/// Callers record BYTES, not tokens: the bytes/4 estimate truncates, so a
/// stream of short chunks estimated one at a time reads as zero tokens
/// whatever its real rate. The division happens once, over the window's whole
/// byte count.
#[derive(Debug, Clone)]
pub struct OutputRateMeter {
    window: Duration,
    /// `(arrival, bytes)` for each chunk still inside the window.
    samples: VecDeque<(Instant, u64)>,
    bytes_in_window: u64,
    /// First content chunk of this response. The span before it is prefill,
    /// which the idle timeout owns, so it is not part of any rate.
    first_chunk_at: Option<Instant>,
}

impl OutputRateMeter {
    pub fn new(window: Duration) -> Self {
        Self {
            window: window.max(Duration::from_secs(1)),
            samples: VecDeque::new(),
            bytes_in_window: 0,
            first_chunk_at: None,
        }
    }

    /// Record `bytes` of model output that arrived at `at`.
    pub fn record(&mut self, at: Instant, bytes: u64) {
        self.first_chunk_at.get_or_insert(at);
        self.samples.push_back((at, bytes));
        self.bytes_in_window = self.bytes_in_window.saturating_add(bytes);
        self.expire(at);
    }

    /// Discard everything that fell out of the trailing window at `now`.
    fn expire(&mut self, now: Instant) {
        let cutoff = now.checked_sub(self.window);
        let Some(cutoff) = cutoff else {
            return;
        };
        while let Some(&(at, bytes)) = self.samples.front() {
            if at >= cutoff {
                break;
            }
            self.samples.pop_front();
            self.bytes_in_window = self.bytes_in_window.saturating_sub(bytes);
        }
    }

    /// The span the current window covers at `now`: the whole window once the
    /// response has run that long, and the time since its first chunk before
    /// then.
    pub fn observed_span(&self, now: Instant) -> Duration {
        let Some(first) = self.first_chunk_at else {
            return Duration::ZERO;
        };
        now.saturating_duration_since(first).min(self.window)
    }

    /// Output tokens per second over the trailing window, or `None` until the
    /// response has streamed for [`MIN_DISPLAY_SPAN`].
    ///
    /// Silence counts: the span keeps growing while no chunk arrives, so a
    /// stream that stops reads as a falling rate rather than as its last
    /// healthy one.
    pub fn rate(&self, now: Instant) -> Option<f64> {
        self.rate_over(now, MIN_DISPLAY_SPAN)
    }

    /// [`Self::rate`] with an explicit minimum span.
    pub fn rate_over(&self, now: Instant, min_span: Duration) -> Option<f64> {
        let span = self.observed_span(now);
        if span < min_span {
            return None;
        }
        let bytes = self.bytes_since(now.checked_sub(self.window));
        let tokens = bytes as f64 / xai_token_estimation::BYTES_PER_TOKEN as f64;
        Some(tokens / span.as_secs_f64())
    }

    /// Bytes recorded at or after `cutoff` (all of them when the process has
    /// not been up as long as the window).
    fn bytes_since(&self, cutoff: Option<Instant>) -> u64 {
        let Some(cutoff) = cutoff else {
            return self.bytes_in_window;
        };
        self.samples
            .iter()
            .filter(|(at, _)| *at >= cutoff)
            .map(|(_, bytes)| *bytes)
            .sum()
    }

    /// Move the whole recorded timeline `gap` later, so a span the model spent
    /// not generating leaves no hole in the average. Everything the response
    /// produced is kept, at the age it had when the gap opened.
    pub fn skip(&mut self, gap: Duration) {
        if gap.is_zero() {
            return;
        }
        if let Some(first) = self.first_chunk_at
            && let Some(shifted) = first.checked_add(gap)
        {
            self.first_chunk_at = Some(shifted);
        }
        for (at, _) in &mut self.samples {
            if let Some(shifted) = at.checked_add(gap) {
                *at = shifted;
            }
        }
    }
}

impl Default for OutputRateMeter {
    fn default() -> Self {
        Self::new(Duration::from_secs(DEFAULT_WINDOW_SECS))
    }
}

/// What one [`OutputRateGate::tick`] found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RateTick {
    /// Nothing changed worth reporting.
    Quiet,
    /// The rate just fell under the floor.
    SlowdownStarted { tokens_per_sec: f64 },
    /// The rate came back above the floor on its own, after `slow_for`.
    SlowdownEnded {
        tokens_per_sec: f64,
        slow_for: Duration,
    },
    /// The rate has been under the floor for the policy's whole sustained
    /// duration. The caller abandons the response and reissues the request.
    /// Reported once per breach.
    Breached {
        tokens_per_sec: f64,
        slow_for: Duration,
    },
}

/// The meter plus the sustained-breach state machine: what the stream is
/// doing, and whether that is bad enough for long enough to reissue over.
///
/// Time to first token is outside all of it. The meter starts at the first
/// content chunk, so a long prefill neither depresses the rate nor counts
/// toward a breach — a slow queue is the idle timeout's business, not this
/// gate's.
#[derive(Debug, Clone)]
pub struct OutputRateGate {
    meter: OutputRateMeter,
    policy: Option<OutputRateFloorPolicy>,
    slow_since: Option<Instant>,
    /// Set once a breach is reported so one collapse cannot be reported twice
    /// while the caller unwinds the stream.
    breached: bool,
    /// Backend-hosted tool calls in flight. A parallel search opens several,
    /// and the stream is generating again only when the last one closes.
    paused_depth: u32,
    /// When the current pause opened. The meter is read as of this instant
    /// while it is set, so the pause neither decays the rate nor advances a
    /// breach.
    paused_since: Option<Instant>,
}

impl OutputRateGate {
    /// A gate measuring over `policy`'s window and judging against its floor.
    /// An absent or unarmed policy still measures — the rate is rendered
    /// whether or not anything gates it — and never breaches.
    pub fn new(policy: Option<OutputRateFloorPolicy>) -> Self {
        let window = policy.unwrap_or_default().window();
        let armed = policy.filter(OutputRateFloorPolicy::floor_armed);
        Self {
            meter: OutputRateMeter::new(window),
            policy: armed,
            slow_since: None,
            breached: false,
            paused_depth: 0,
            paused_since: None,
        }
    }

    /// Record `bytes` of model output that arrived at `at`.
    ///
    /// Output arriving is the end of any pause, whatever the backend has yet
    /// to say about its hosted call: the model is generating, so this is a
    /// stream the floor judges.
    pub fn record(&mut self, at: Instant, bytes: u64) {
        self.end_pause(at);
        self.meter.record(at, bytes);
    }

    /// A backend-hosted tool call started at `at`. The server runs it and the
    /// model generates nothing until it returns, so the span it takes is
    /// removed from the measurement rather than averaged in as silence.
    pub fn pause(&mut self, at: Instant) {
        self.paused_depth = self.paused_depth.saturating_add(1);
        self.paused_since.get_or_insert(at);
    }

    /// A backend-hosted tool call finished at `at`. The last one to finish
    /// closes the pause.
    pub fn resume(&mut self, at: Instant) {
        self.paused_depth = self.paused_depth.saturating_sub(1);
        if self.paused_depth == 0 {
            self.end_pause(at);
        }
    }

    /// Whether a hosted tool call is holding the measurement.
    pub fn is_paused(&self) -> bool {
        self.paused_since.is_some()
    }

    /// Close an open pause at `at`, moving the recorded timeline and the
    /// sustained-breach clock past the gap so neither counts it.
    fn end_pause(&mut self, at: Instant) {
        self.paused_depth = 0;
        let Some(since) = self.paused_since.take() else {
            return;
        };
        let gap = at.saturating_duration_since(since);
        self.meter.skip(gap);
        if let Some(slow_since) = self.slow_since
            && let Some(shifted) = slow_since.checked_add(gap)
        {
            self.slow_since = Some(shifted);
        }
    }

    /// The instant the meter is read at: frozen at the start of an open pause,
    /// so a hosted tool call neither decays the rendered rate nor advances a
    /// breach toward its sustained duration.
    fn measured_at(&self, now: Instant) -> Instant {
        match self.paused_since {
            Some(since) if since < now => since,
            _ => now,
        }
    }

    /// The current trailing-window rate, or `None` before there is enough
    /// stream to divide by.
    pub fn rate(&self, now: Instant) -> Option<f64> {
        self.meter.rate(self.measured_at(now))
    }

    /// The configured floor, when one is armed.
    pub fn floor(&self) -> Option<f64> {
        self.policy.map(|p| p.min_tokens_per_sec)
    }

    /// The window the rate is measured over.
    pub fn window(&self) -> Duration {
        self.meter.window
    }

    /// How long the rate has been under the floor, when it is.
    pub fn slow_for(&self, now: Instant) -> Option<Duration> {
        let now = self.measured_at(now);
        self.slow_since
            .map(|since| now.saturating_duration_since(since))
    }

    /// Advance the state machine. Call this on a timer, not only on arriving
    /// chunks: a stream that stops dead delivers nothing to record, and it is
    /// the tick that turns that silence into a falling rate.
    pub fn tick(&mut self, now: Instant) -> RateTick {
        let Some(policy) = self.policy else {
            return RateTick::Quiet;
        };
        let now = self.measured_at(now);
        let Some(rate) = self.meter.rate(now) else {
            return RateTick::Quiet;
        };
        if rate >= policy.min_tokens_per_sec {
            let ended = self.slow_since.map(|since| RateTick::SlowdownEnded {
                tokens_per_sec: rate,
                slow_for: now.saturating_duration_since(since),
            });
            self.slow_since = None;
            self.breached = false;
            return ended.unwrap_or(RateTick::Quiet);
        }
        let Some(since) = self.slow_since else {
            self.slow_since = Some(now);
            return RateTick::SlowdownStarted {
                tokens_per_sec: rate,
            };
        };
        let slow_for = now.saturating_duration_since(since);
        if !self.breached && slow_for >= policy.sustained() {
            self.breached = true;
            return RateTick::Breached {
                tokens_per_sec: rate,
                slow_for,
            };
        }
        RateTick::Quiet
    }

    /// Start the sustained-breach clock again at `now`, while the rate stays
    /// under the floor. A breach is reported a single time per slowdown.
    pub fn rearm(&mut self, now: Instant) {
        if self.slow_since.is_some() {
            self.slow_since = Some(self.measured_at(now));
        }
        self.breached = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4 bytes is one estimated token, so 40 bytes per 100 ms is 100 tok/s.
    #[test]
    fn rate_reads_a_healthy_stream() {
        let start = Instant::now();
        let mut meter = OutputRateMeter::new(Duration::from_secs(10));
        for i in 0..100 {
            meter.record(start + Duration::from_millis(i * 100), 40);
        }
        let now = start + Duration::from_millis(100 * 100);
        let rate = meter.rate(now).expect("ten seconds of stream");
        assert!((rate - 100.0).abs() < 1.0, "rate was {rate}");
    }

    /// Per-chunk estimation is what this avoids: 3-byte chunks each estimate
    /// to zero tokens, so a summed-estimate meter reports a healthy stream as
    /// completely stalled.
    #[test]
    fn short_chunks_are_not_rounded_away() {
        let start = Instant::now();
        let mut meter = OutputRateMeter::new(Duration::from_secs(10));
        for i in 0..1000 {
            meter.record(start + Duration::from_millis(i * 10), 3);
            assert_eq!(
                xai_token_estimation::estimate_tokens("abc"),
                0,
                "the per-chunk estimate this test exists for"
            );
        }
        let now = start + Duration::from_millis(10_000);
        let rate = meter.rate(now).expect("ten seconds of stream");
        // 3000 bytes / 4 = 750 tokens over 10 s.
        assert!((rate - 75.0).abs() < 1.0, "rate was {rate}");
    }

    #[test]
    fn a_short_stream_has_no_rate_yet() {
        let start = Instant::now();
        let mut meter = OutputRateMeter::new(Duration::from_secs(10));
        meter.record(start, 400);
        assert_eq!(meter.rate(start + Duration::from_millis(200)), None);
        assert!(meter.rate(start + MIN_DISPLAY_SPAN).is_some());
    }

    /// The window slides: a burst that fell out of it stops counting.
    #[test]
    fn an_expired_burst_leaves_the_window() {
        let start = Instant::now();
        let mut meter = OutputRateMeter::new(Duration::from_secs(10));
        meter.record(start, 40_000);
        let just_inside = meter
            .rate(start + Duration::from_secs(9))
            .expect("inside the window");
        assert!(just_inside > 1000.0, "rate was {just_inside}");
        let after = meter
            .rate(start + Duration::from_secs(11))
            .expect("the span is the whole window");
        assert_eq!(after, 0.0);
    }

    /// A collapsed stream: 4 bytes (one estimated token) per second.
    fn collapsed_policy() -> OutputRateFloorPolicy {
        OutputRateFloorPolicy {
            min_tokens_per_sec: 10.0,
            window_secs: 10,
            sustained_secs: 10,
            max_retries: 2,
            ttft_timeout_secs: 0,
        }
    }

    /// One 250 ms step of the sampler's loop: whatever arrived in that step is
    /// recorded, then the gate is ticked. Recording a whole timeline up front
    /// and ticking afterwards would let the meter count bytes that have not
    /// arrived yet, and every reading would come out healthy.
    ///
    /// `bytes_at` answers how many bytes arrive on step `i` (0 = nothing).
    /// Returns every non-quiet tick.
    fn drive(
        gate: &mut OutputRateGate,
        start: Instant,
        steps: u64,
        bytes_at: impl Fn(u64) -> u64,
    ) -> Vec<RateTick> {
        let mut seen = Vec::new();
        for i in 0..steps {
            let now = start + Duration::from_millis(i * 250);
            let bytes = bytes_at(i);
            if bytes > 0 {
                gate.record(now, bytes);
            }
            match gate.tick(now) {
                RateTick::Quiet => {}
                other => seen.push(other),
            }
        }
        seen
    }

    #[test]
    fn a_collapse_breaches_only_after_the_sustained_duration() {
        let policy = collapsed_policy();
        let start = Instant::now();
        let mut gate = OutputRateGate::new(Some(policy));

        // A stream that is collapsed from its first token: one byte a tick.
        let early = drive(&mut gate, start, 9 * 4, |_| 1);
        assert!(
            matches!(early.as_slice(), [RateTick::SlowdownStarted { .. }]),
            "expected only a slowdown start in the first nine seconds: {early:?}"
        );

        let late = drive(&mut gate, start + Duration::from_secs(9), 30 * 4, |_| 1);
        let breaches: Vec<_> = late
            .iter()
            .filter(|t| matches!(t, RateTick::Breached { .. }))
            .collect();
        assert_eq!(
            breaches.len(),
            1,
            "one collapse must report exactly one breach: {late:?}"
        );
        let RateTick::Breached { slow_for, .. } = breaches[0] else {
            unreachable!("filtered above")
        };
        assert!(
            *slow_for >= policy.sustained(),
            "breached after only {slow_for:?}"
        );
    }

    #[test]
    fn a_rearmed_gate_breaches_again_after_the_sustained_duration() {
        let policy = collapsed_policy();
        let start = Instant::now();
        let mut gate = OutputRateGate::new(Some(policy));
        let first = drive(&mut gate, start, 40 * 4, |_| 1);
        assert_eq!(
            first
                .iter()
                .filter(|t| matches!(t, RateTick::Breached { .. }))
                .count(),
            1,
            "{first:?}"
        );

        let rearmed_at = start + Duration::from_secs(40);
        gate.rearm(rearmed_at);
        let within = drive(&mut gate, rearmed_at, 9 * 4, |_| 1);
        assert!(
            within.is_empty(),
            "no breach inside the new sustained duration: {within:?}"
        );
        let after = drive(&mut gate, rearmed_at + Duration::from_secs(9), 4 * 4, |_| 1);
        assert!(
            matches!(after.as_slice(), [RateTick::Breached { .. }]),
            "one more breach after it: {after:?}"
        );
    }

    /// A dip that recovers is reported at both edges and never reissued over.
    /// The collapse here outlasts the window but not the sustained duration.
    #[test]
    fn a_recovered_dip_never_breaches() {
        let policy = OutputRateFloorPolicy {
            sustained_secs: 30,
            ..collapsed_policy()
        };
        let start = Instant::now();
        let mut gate = OutputRateGate::new(Some(policy));
        // Two seconds of healthy output, a sixteen-second collapse to one byte
        // a tick, then healthy output again.
        let seen = drive(&mut gate, start, 40 * 4, |i| match i {
            0..=7 => 400,
            8..=71 => 1,
            _ => 400,
        });
        assert!(
            seen.iter()
                .any(|t| matches!(t, RateTick::SlowdownStarted { .. })),
            "the stall must be reported: {seen:?}"
        );
        assert!(
            seen.iter()
                .any(|t| matches!(t, RateTick::SlowdownEnded { .. })),
            "the recovery must be reported: {seen:?}"
        );
        assert!(
            !seen.iter().any(|t| matches!(t, RateTick::Breached { .. })),
            "a dip shorter than the sustained duration is not a collapse: {seen:?}"
        );
    }

    /// A stream that stops dead delivers nothing to record. The tick is what
    /// turns that silence into a falling rate, so the gate still fires.
    #[test]
    fn silence_drives_the_rate_down() {
        let policy = collapsed_policy();
        let start = Instant::now();
        let mut gate = OutputRateGate::new(Some(policy));
        // Two seconds of healthy output, then the stream stops dead.
        let seen = drive(&mut gate, start, 40 * 4, |i| if i < 8 { 400 } else { 0 });
        assert!(
            seen.iter().any(|t| matches!(t, RateTick::Breached { .. })),
            "two seconds of output then silence must breach: {seen:?}"
        );
    }

    /// Time to first token is not part of the rate and not part of a breach.
    /// A minute of prefill followed by a healthy stream reports neither a
    /// slowdown nor a breach.
    #[test]
    fn a_long_prefill_is_not_a_slowdown() {
        let policy = collapsed_policy();
        let start = Instant::now();
        let mut gate = OutputRateGate::new(Some(policy));

        // Sixty seconds of nothing: the request is queued, not slow.
        let first_token = start + Duration::from_secs(60);
        for i in 0..4 {
            assert_eq!(
                gate.tick(start + Duration::from_secs(i * 15)),
                RateTick::Quiet,
                "prefill must not read as a rate at all"
            );
        }
        assert_eq!(gate.rate(first_token), None);

        let seen = drive(&mut gate, first_token, 20 * 4, |_| 400);
        assert!(
            seen.is_empty(),
            "a healthy stream after a long prefill is quiet: {seen:?}"
        );
    }

    /// A server-side tool call is the server's time, not the stream's. A
    /// minute of web search between two healthy bursts is neither a slowdown
    /// nor a breach.
    #[test]
    fn a_hosted_tool_call_is_not_a_slowdown() {
        let policy = collapsed_policy();
        let start = Instant::now();
        let mut gate = OutputRateGate::new(Some(policy));

        let before = drive(&mut gate, start, 8 * 4, |_| 400);
        assert!(
            before.is_empty(),
            "the opening burst is healthy: {before:?}"
        );
        let healthy = gate
            .rate(start + Duration::from_secs(8))
            .expect("eight seconds of stream");

        // The search runs for a minute and delivers nothing.
        let search_start = start + Duration::from_secs(8);
        gate.pause(search_start);
        for i in 0..240 {
            let now = search_start + Duration::from_millis(i * 250);
            assert_eq!(
                gate.tick(now),
                RateTick::Quiet,
                "a hosted call must not move the gate"
            );
        }
        let search_end = search_start + Duration::from_secs(60);
        assert_eq!(
            gate.rate(search_end),
            Some(healthy),
            "the rendered rate holds at what the stream was doing"
        );
        gate.resume(search_end);
        assert!(!gate.is_paused());

        // The burst before the search is still inside the window. The gap left
        // the timeline instead of being averaged in as silence.
        let after = drive(&mut gate, search_end, 8 * 4, |_| 400);
        assert!(
            after.is_empty(),
            "a healthy stream either side of a search is quiet: {after:?}"
        );
    }

    /// A parallel search opens several hosted calls. The stream is generating
    /// again only when the last one closes.
    #[test]
    fn parallel_hosted_calls_hold_the_pause_until_the_last_one_ends() {
        let start = Instant::now();
        let mut gate = OutputRateGate::new(Some(collapsed_policy()));
        drive(&mut gate, start, 8 * 4, |_| 400);
        let at = start + Duration::from_secs(8);
        gate.pause(at);
        gate.pause(at);
        gate.resume(at + Duration::from_secs(5));
        assert!(gate.is_paused(), "one call is still running");
        gate.resume(at + Duration::from_secs(9));
        assert!(!gate.is_paused());
        let after = drive(&mut gate, at + Duration::from_secs(9), 8 * 4, |_| 400);
        assert!(after.is_empty(), "the whole search span is gone: {after:?}");
    }

    /// The pause is not a way for a collapsed stream to hide. Output arriving
    /// ends it, whatever the backend has yet to say about its hosted call, and
    /// the collapse after it breaches on schedule.
    #[test]
    fn output_during_a_hosted_call_ends_the_pause() {
        let start = Instant::now();
        let mut gate = OutputRateGate::new(Some(collapsed_policy()));
        drive(&mut gate, start, 8 * 4, |_| 400);
        let at = start + Duration::from_secs(8);
        gate.pause(at);
        gate.record(at + Duration::from_secs(30), 400);
        assert!(!gate.is_paused(), "tokens mean the model is generating");

        let seen = drive(&mut gate, at + Duration::from_secs(30), 40 * 4, |i| {
            if i == 0 { 400 } else { 1 }
        });
        assert!(
            seen.iter().any(|t| matches!(t, RateTick::Breached { .. })),
            "a collapse after the search still breaches: {seen:?}"
        );
    }

    /// An unbalanced resume — one the gate never saw a matching start for —
    /// leaves the measurement alone rather than jumping the timeline.
    #[test]
    fn a_resume_without_a_pause_changes_nothing() {
        let start = Instant::now();
        let mut gate = OutputRateGate::new(Some(collapsed_policy()));
        drive(&mut gate, start, 8 * 4, |_| 400);
        let at = start + Duration::from_secs(8);
        let before = gate.rate(at);
        gate.resume(at + Duration::from_secs(30));
        assert!(!gate.is_paused());
        assert_eq!(gate.rate(at), before);
    }

    #[test]
    fn an_unarmed_policy_never_trips() {
        let policy = OutputRateFloorPolicy {
            min_tokens_per_sec: 0.0,
            ..OutputRateFloorPolicy::default()
        };
        assert!(!policy.is_armed());
        let start = Instant::now();
        let mut gate = OutputRateGate::new(Some(policy));
        assert_eq!(gate.floor(), None);
        assert!(drive(&mut gate, start, 60 * 4, |_| 1).is_empty());
        assert!(
            gate.rate(start + Duration::from_secs(10)).is_some(),
            "an ungated session still measures the rate it renders"
        );
    }

    #[test]
    fn classify_reads_the_three_bands() {
        assert_eq!(classify_rate(4.0, Some(10.0)), OutputRateHealth::Slow);
        assert_eq!(classify_rate(12.0, Some(10.0)), OutputRateHealth::Near);
        assert_eq!(classify_rate(15.0, Some(10.0)), OutputRateHealth::Healthy);
        assert_eq!(
            classify_rate(0.1, None),
            OutputRateHealth::Healthy,
            "no floor asked for, so nothing is wrong"
        );
        assert_eq!(classify_rate(0.1, Some(0.0)), OutputRateHealth::Healthy);
    }

    #[test]
    fn clamped_raises_unusable_values_and_caps_nothing() {
        let clamped = OutputRateFloorPolicy {
            min_tokens_per_sec: 9_000.0,
            window_secs: 0,
            sustained_secs: 0,
            max_retries: 99,
            ttft_timeout_secs: 99_999,
        }
        .clamped();
        assert_eq!(clamped.min_tokens_per_sec, 9_000.0);
        assert_eq!(clamped.window_secs, OutputRateFloorPolicy::MIN_WINDOW_SECS);
        assert_eq!(
            clamped.sustained_secs,
            OutputRateFloorPolicy::MIN_SUSTAINED_SECS
        );
        assert_eq!(clamped.max_retries, 99);
        assert_eq!(clamped.ttft_timeout_secs, 99_999);

        let large = OutputRateFloorPolicy {
            window_secs: 3_600,
            sustained_secs: 3_600,
            ..Default::default()
        }
        .clamped();
        assert_eq!(large.window_secs, 3_600);
        assert_eq!(large.sustained_secs, 3_600);

        let negative = OutputRateFloorPolicy {
            min_tokens_per_sec: -3.0,
            ..Default::default()
        }
        .clamped();
        assert_eq!(negative.min_tokens_per_sec, 0.0);

        let nan = OutputRateFloorPolicy {
            min_tokens_per_sec: f64::NAN,
            ..Default::default()
        }
        .clamped();
        assert_eq!(nan.min_tokens_per_sec, 0.0, "NaN disarms rather than trips");
        assert!(!nan.is_armed());
    }

    /// A time-to-first-token limit arms the policy on its own. The rate half
    /// stays off: the gate built from it never judges a rate.
    #[test]
    fn a_ttft_limit_alone_arms_the_policy_but_not_the_floor() {
        let policy = OutputRateFloorPolicy {
            min_tokens_per_sec: 0.0,
            ttft_timeout_secs: 120,
            ..Default::default()
        }
        .clamped();
        assert!(policy.is_armed());
        assert!(policy.ttft_armed());
        assert!(!policy.floor_armed());
        assert_eq!(policy.ttft_timeout(), Some(Duration::from_secs(120)));

        let start = Instant::now();
        let mut gate = OutputRateGate::new(Some(policy));
        assert_eq!(gate.floor(), None, "no floor to publish or breach");
        assert!(drive(&mut gate, start, 60 * 4, |_| 1).is_empty());
    }

    #[test]
    fn an_unlimited_budget_never_runs_out() {
        let unlimited = OutputRateFloorPolicy {
            max_retries: OutputRateFloorPolicy::UNLIMITED_RETRIES,
            ..Default::default()
        };
        assert!(unlimited.has_retries_left(0));
        assert!(unlimited.has_retries_left(u32::MAX));

        let two = OutputRateFloorPolicy {
            max_retries: 2,
            ..Default::default()
        };
        assert!(two.has_retries_left(1));
        assert!(!two.has_retries_left(2));

        let none = OutputRateFloorPolicy {
            max_retries: 0,
            ..Default::default()
        };
        assert!(!none.has_retries_left(0));
    }

    #[test]
    fn a_zero_ttft_limit_is_off() {
        let policy = OutputRateFloorPolicy::default();
        assert!(!policy.ttft_armed());
        assert_eq!(policy.ttft_timeout(), None);
        assert!(!policy.is_armed(), "the default policy gates nothing");
    }
}
