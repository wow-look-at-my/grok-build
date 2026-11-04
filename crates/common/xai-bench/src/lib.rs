//! A wall-clock benchmark harness with the subset of the criterion API that
//! the workspace benches use.

use std::fmt::Display;
use std::hint::black_box;
use std::marker::PhantomData;
use std::time::{Duration, Instant};

pub mod measurement {
    /// The only measurement this harness has: elapsed wall-clock time.
    pub struct WallTime;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Measure,
    Smoke,
}

#[derive(Clone, Copy, Debug)]
struct Settings {
    sample_size: usize,
    warm_up_time: Duration,
    measurement_time: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sample_size: 100,
            warm_up_time: Duration::from_secs(3),
            measurement_time: Duration::from_secs(5),
        }
    }
}

/// The harness. `Criterion::default()` reads the process arguments.
pub struct Criterion {
    mode: Mode,
    filter: Option<String>,
    settings: Settings,
}

impl Default for Criterion {
    fn default() -> Self {
        Self::from_args(std::env::args().skip(1))
    }
}

impl Criterion {
    fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        let mut mode = Mode::Smoke;
        let mut filter = None;
        for arg in args {
            if arg == "--bench" {
                mode = Mode::Measure;
            } else if !arg.starts_with('-') && filter.is_none() {
                filter = Some(arg);
            }
        }
        Self {
            mode,
            filter,
            settings: Settings::default(),
        }
    }

    pub fn bench_function<F>(&mut self, id: &str, f: F) -> &mut Self
    where
        F: FnMut(&mut Bencher),
    {
        let settings = self.settings;
        self.run(id, settings, None, f);
        self
    }

    pub fn bench_with_input<I: ?Sized, F>(
        &mut self,
        id: impl IntoBenchmarkId,
        input: &I,
        mut f: F,
    ) -> &mut Self
    where
        F: FnMut(&mut Bencher, &I),
    {
        let settings = self.settings;
        let id = id.into_benchmark_id().0;
        self.run(&id, settings, None, |b| f(b, input));
        self
    }

    pub fn benchmark_group(&mut self, name: impl Into<String>) -> BenchmarkGroup<'_> {
        let settings = self.settings;
        BenchmarkGroup {
            criterion: self,
            name: name.into(),
            settings,
            throughput: None,
            _measurement: PhantomData,
        }
    }

    fn selected(&self, id: &str) -> bool {
        self.filter.as_deref().is_none_or(|f| id.contains(f))
    }

    fn run<F>(&mut self, id: &str, settings: Settings, throughput: Option<Throughput>, mut f: F)
    where
        F: FnMut(&mut Bencher),
    {
        if !self.selected(id) {
            return;
        }
        match self.mode {
            Mode::Smoke => {
                let mut b = Bencher::new(1);
                f(&mut b);
                println!("Testing {id} ... Success");
            }
            Mode::Measure => {
                let samples = measure(settings, &mut f);
                println!("{}", report(id, &samples, throughput));
            }
        }
    }
}

/// Warms up, then takes `sample_size` samples of an equal iteration count. The
/// result is the time per iteration of each sample, in nanoseconds.
fn measure<F: FnMut(&mut Bencher)>(settings: Settings, f: &mut F) -> Vec<u128> {
    let mut iters: u64 = 1;
    let mut spent = Duration::ZERO;
    let mut done: u64 = 0;
    let warm_up_start = Instant::now();
    while warm_up_start.elapsed() < settings.warm_up_time {
        let mut b = Bencher::new(iters);
        f(&mut b);
        spent += b.elapsed;
        done += iters;
        iters = iters.saturating_mul(2);
    }
    let per_iter = (spent.as_nanos() / u128::from(done.max(1))).max(1);
    let sample_count = settings.sample_size.max(1);
    let budget = settings.measurement_time.as_nanos() / sample_count as u128;
    let iters = u64::try_from((budget / per_iter).max(1)).unwrap_or(u64::MAX);

    (0..sample_count)
        .map(|_| {
            let mut b = Bencher::new(iters);
            f(&mut b);
            b.elapsed.as_nanos() / u128::from(iters)
        })
        .collect()
}

fn report(id: &str, samples: &[u128], throughput: Option<Throughput>) -> String {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let min = sorted.first().copied().unwrap_or(0);
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0);
    let mean = sorted.iter().sum::<u128>() / (sorted.len().max(1) as u128);
    let mut line = format!(
        "{id:<40} time: [{} {} {}]",
        format_nanos(min),
        format_nanos(median),
        format_nanos(mean)
    );
    if let Some(t) = throughput {
        let (count, unit) = match t {
            Throughput::Bytes(n) => (n, "B/s"),
            Throughput::Elements(n) => (n, "elem/s"),
        };
        let per_sec = u128::from(count) * 1_000_000_000 / median.max(1);
        line.push_str(&format!(" thrpt: {per_sec} {unit}"));
    }
    line
}

fn format_nanos(n: u128) -> String {
    match n {
        0..1_000 => format!("{n} ns"),
        1_000..1_000_000 => format!("{}.{:02} µs", n / 1_000, n % 1_000 / 10),
        1_000_000..1_000_000_000 => format!("{}.{:02} ms", n / 1_000_000, n % 1_000_000 / 10_000),
        _ => format!(
            "{}.{:02} s",
            n / 1_000_000_000,
            n % 1_000_000_000 / 10_000_000
        ),
    }
}

pub struct BenchmarkGroup<'a, M = measurement::WallTime> {
    criterion: &'a mut Criterion,
    name: String,
    settings: Settings,
    throughput: Option<Throughput>,
    _measurement: PhantomData<M>,
}

impl<M> BenchmarkGroup<'_, M> {
    pub fn sample_size(&mut self, n: usize) -> &mut Self {
        self.settings.sample_size = n;
        self
    }

    pub fn warm_up_time(&mut self, d: Duration) -> &mut Self {
        self.settings.warm_up_time = d;
        self
    }

    pub fn measurement_time(&mut self, d: Duration) -> &mut Self {
        self.settings.measurement_time = d;
        self
    }

    pub fn throughput(&mut self, t: Throughput) -> &mut Self {
        self.throughput = Some(t);
        self
    }

    /// Accepted for source compatibility. Every sample uses the same iteration
    /// count, which is what `SamplingMode::Flat` asks for.
    pub fn sampling_mode(&mut self, _mode: SamplingMode) -> &mut Self {
        self
    }

    pub fn bench_function<F>(&mut self, id: impl IntoBenchmarkId, f: F) -> &mut Self
    where
        F: FnMut(&mut Bencher),
    {
        let id = format!("{}/{}", self.name, id.into_benchmark_id().0);
        self.criterion.run(&id, self.settings, self.throughput, f);
        self
    }

    pub fn bench_with_input<I: ?Sized, F>(
        &mut self,
        id: impl IntoBenchmarkId,
        input: &I,
        mut f: F,
    ) -> &mut Self
    where
        F: FnMut(&mut Bencher, &I),
    {
        let id = format!("{}/{}", self.name, id.into_benchmark_id().0);
        self.criterion
            .run(&id, self.settings, self.throughput, |b| f(b, input));
        self
    }

    pub fn finish(self) {}
}

/// Times the routine a benchmark gives it.
pub struct Bencher {
    iters: u64,
    elapsed: Duration,
}

impl Bencher {
    fn new(iters: u64) -> Self {
        Self {
            iters,
            elapsed: Duration::ZERO,
        }
    }

    pub fn iter<O, R: FnMut() -> O>(&mut self, mut routine: R) {
        let start = Instant::now();
        for _ in 0..self.iters {
            black_box(routine());
        }
        self.elapsed = start.elapsed();
    }

    /// Like [`Bencher::iter`], but the outputs are dropped outside the timed region.
    pub fn iter_with_large_drop<O, R: FnMut() -> O>(&mut self, mut routine: R) {
        let mut outputs = Vec::with_capacity(usize::try_from(self.iters).unwrap_or(0));
        let start = Instant::now();
        for _ in 0..self.iters {
            outputs.push(black_box(routine()));
        }
        self.elapsed = start.elapsed();
        drop(outputs);
    }

    /// Only `routine` is timed. Every iteration gets a fresh input from `setup`,
    /// whatever the batch size.
    pub fn iter_batched<I, O, S, R>(&mut self, mut setup: S, mut routine: R, _size: BatchSize)
    where
        S: FnMut() -> I,
        R: FnMut(I) -> O,
    {
        let mut elapsed = Duration::ZERO;
        for _ in 0..self.iters {
            let input = setup();
            let start = Instant::now();
            let output = black_box(routine(black_box(input)));
            elapsed += start.elapsed();
            drop(output);
        }
        self.elapsed = elapsed;
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Throughput {
    Bytes(u64),
    Elements(u64),
}

#[derive(Clone, Copy, Debug)]
pub enum BatchSize {
    SmallInput,
    LargeInput,
    PerIteration,
    NumBatches(u64),
    NumIterations(u64),
}

#[derive(Clone, Copy, Debug)]
pub enum SamplingMode {
    Auto,
    Linear,
    Flat,
}

pub struct BenchmarkId(String);

impl BenchmarkId {
    pub fn new(function_name: impl Into<String>, parameter: impl Display) -> Self {
        Self(format!("{}/{parameter}", function_name.into()))
    }

    pub fn from_parameter(parameter: impl Display) -> Self {
        Self(parameter.to_string())
    }
}

pub trait IntoBenchmarkId {
    fn into_benchmark_id(self) -> BenchmarkId;
}

impl IntoBenchmarkId for BenchmarkId {
    fn into_benchmark_id(self) -> BenchmarkId {
        self
    }
}

impl IntoBenchmarkId for &str {
    fn into_benchmark_id(self) -> BenchmarkId {
        BenchmarkId(self.to_owned())
    }
}

impl IntoBenchmarkId for String {
    fn into_benchmark_id(self) -> BenchmarkId {
        BenchmarkId(self)
    }
}

/// `criterion_group!(name, target, ...)` defines `fn name()` that runs each target.
#[macro_export]
macro_rules! criterion_group {
    ($name:ident, $($target:path),+ $(,)?) => {
        pub fn $name() {
            let mut criterion = $crate::Criterion::default();
            $($target(&mut criterion);)+
        }
    };
}

/// `criterion_main!(group, ...)` defines `main` that runs each group.
#[macro_export]
macro_rules! criterion_main {
    ($($group:path),+ $(,)?) => {
        fn main() {
            $($group();)+
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn args(list: &[&str]) -> Criterion {
        Criterion::from_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn mode_follows_the_bench_flag() {
        assert_eq!(args(&[]).mode, Mode::Smoke);
        assert_eq!(args(&["--bench"]).mode, Mode::Measure);
        assert_eq!(args(&["--bench", "scan"]).filter.as_deref(), Some("scan"));
    }

    #[test]
    fn smoke_mode_runs_each_routine_once() {
        let calls = Cell::new(0);
        let mut c = args(&[]);
        c.bench_function("one", |b| b.iter(|| calls.set(calls.get() + 1)));
        let mut g = c.benchmark_group("g");
        g.bench_with_input(BenchmarkId::new("f", 3), &3, |b, n| {
            b.iter(|| calls.set(calls.get() + n))
        });
        g.finish();
        assert_eq!(calls.get(), 4);
    }

    #[test]
    fn filter_skips_unmatched_ids() {
        let calls = Cell::new(0);
        let mut c = args(&["keep"]);
        c.bench_function("keep_me", |b| b.iter(|| calls.set(calls.get() + 1)));
        c.bench_function("drop_me", |b| b.iter(|| calls.set(calls.get() + 10)));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn group_ids_join_with_a_slash() {
        let seen = std::cell::RefCell::new(Vec::new());
        let mut c = args(&["g/f/7"]);
        let mut g = c.benchmark_group("g");
        g.bench_function(BenchmarkId::new("f", 7), |b| {
            b.iter(|| seen.borrow_mut().push("hit"))
        });
        g.bench_function(BenchmarkId::from_parameter(8), |b| {
            b.iter(|| seen.borrow_mut().push("miss"))
        });
        assert_eq!(*seen.borrow(), vec!["hit"]);
    }

    #[test]
    fn iter_batched_times_only_the_routine_and_sets_up_every_iteration() {
        let setups = Cell::new(0);
        let mut b = Bencher::new(5);
        b.iter_batched(
            || {
                setups.set(setups.get() + 1);
                std::thread::sleep(Duration::from_millis(20));
            },
            |()| (),
            BatchSize::SmallInput,
        );
        assert_eq!(setups.get(), 5);
        assert!(b.elapsed < Duration::from_millis(50), "{:?}", b.elapsed);
    }

    #[test]
    fn measure_takes_the_requested_sample_count() {
        let settings = Settings {
            sample_size: 7,
            warm_up_time: Duration::from_millis(5),
            measurement_time: Duration::from_millis(20),
        };
        let samples = measure(settings, &mut |b: &mut Bencher| b.iter(|| black_box(1 + 1)));
        assert_eq!(samples.len(), 7);
    }

    #[test]
    fn report_formats_time_and_throughput() {
        let line = report("id", &[2_000, 1_000, 3_000], Some(Throughput::Elements(10)));
        assert!(line.contains("time: [1.00 µs 2.00 µs 2.00 µs]"), "{line}");
        assert!(line.contains("thrpt: 5000000 elem/s"), "{line}");
    }
}
