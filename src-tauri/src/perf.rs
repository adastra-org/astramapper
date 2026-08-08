//! Timings and counters for the work the app actually does.
//!
//! # Why this exists
//!
//! Phase 4 is measurement-led and every number in `docs/phases.md` was produced
//! by a throwaway benchmark or an `#[ignore]`d gate against a local install.
//! That is fine for deciding what to build and useless for the question a user
//! or a bug report asks: *why was that slow, on this machine, just now.* A
//! scratch harness cannot answer it because it is not running.
//!
//! So the same spans the benches measured are recorded in the app, always, and
//! read back over IPC.
//!
//! # Not `tracing`
//!
//! `tracing` plus a subscriber is the idiomatic answer and it is the wrong size
//! here. What is wanted is a dozen named spans with percentiles, readable from
//! the WebView; what that would bring is a span tree, a filter language, a
//! subscriber registry and two more crates in the tree. This module is the part
//! that gets used.
//!
//! # Always on
//!
//! Recording is a `Mutex` acquisition and a few adds on a path already doing
//! archive I/O, so there is no feature flag and no sampling: instrumentation
//! that has to be switched on is instrumentation that is off when the report
//! comes in. Set `ASTRAMAPPER_PERF=1` to also echo each span to stderr, which
//! is for watching a sequence live rather than for the aggregates.
//!
//! # What a name means
//!
//! One name is one kind of work, and the **outcome is part of the name** rather
//! than a field — `world_map (hit)` and `world_map (miss)` are different work
//! with different distributions, and averaging them together produces a number
//! that describes neither.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

/// How many recent durations a span keeps for percentiles.
///
/// Enough that p95 means something, small enough that a span called ten
/// thousand times costs a fixed 2 KB rather than growing without bound. The
/// count, total, min and max are exact over all calls regardless; only the
/// percentiles are over the tail.
const WINDOW: usize = 256;

#[derive(Debug, Default)]
struct Entry {
    calls: u64,
    total: Duration,
    min: Option<Duration>,
    max: Duration,
    /// Bytes this span moved, when it moves any. Zero means "not a byte span".
    bytes: u64,
    /// The last `WINDOW` durations, oldest first once it wraps.
    recent: Vec<Duration>,
    at: usize,
}

impl Entry {
    fn record(&mut self, elapsed: Duration, bytes: u64) {
        self.calls += 1;
        self.total += elapsed;
        self.min = Some(self.min.map_or(elapsed, |m| m.min(elapsed)));
        self.max = self.max.max(elapsed);
        self.bytes += bytes;
        if self.recent.len() < WINDOW {
            self.recent.push(elapsed);
        } else {
            self.recent[self.at] = elapsed;
            self.at = (self.at + 1) % WINDOW;
        }
    }
}

fn table() -> &'static Mutex<HashMap<&'static str, Entry>> {
    static TABLE: OnceLock<Mutex<HashMap<&'static str, Entry>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn echoing() -> bool {
    static ECHO: OnceLock<bool> = OnceLock::new();
    *ECHO.get_or_init(|| {
        std::env::var("ASTRAMAPPER_PERF").is_ok_and(|v| v != "0" && !v.is_empty())
    })
}

/// Record one completed span.
///
/// A poisoned lock is ignored rather than propagated, the same rule the cache
/// follows: a panic in one thread must not take instrumentation down with it,
/// and least of all must it turn a measurement into a second failure.
pub fn record(name: &'static str, elapsed: Duration, bytes: u64) {
    if echoing() {
        if bytes > 0 {
            eprintln!("[perf] {name} {:.2}ms {bytes}B", elapsed.as_secs_f64() * 1000.0);
        } else {
            eprintln!("[perf] {name} {:.2}ms", elapsed.as_secs_f64() * 1000.0);
        }
    }
    let mut guard = match table().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.entry(name).or_default().record(elapsed, bytes);
}

/// Time `body`, record it under `name`, and hand back what it returned.
pub fn span<T>(name: &'static str, body: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let out = body();
    record(name, started.elapsed(), 0);
    out
}

/// Time `body` and record how many bytes the result carries.
///
/// Only the `Ok` side is measured for size; a failure has no payload and its
/// duration still counts, because a command that fails slowly is exactly the
/// thing worth seeing.
pub fn span_bytes<T, E>(
    name: &'static str,
    body: impl FnOnce() -> Result<T, E>,
    size: impl FnOnce(&T) -> usize,
) -> Result<T, E> {
    let started = Instant::now();
    let out = body();
    let bytes = out.as_ref().map(|value| size(value) as u64).unwrap_or(0);
    record(name, started.elapsed(), bytes);
    out
}

/// A span measured by hand, for work that does not fit inside one closure.
pub struct Timer {
    name: &'static str,
    started: Instant,
}

impl Timer {
    pub fn start(name: &'static str) -> Self {
        Self { name, started: Instant::now() }
    }

    /// Record and consume. Dropping without calling this records nothing, on
    /// purpose: a timer abandoned down an early-return path measured a
    /// different thing than the one it was named for.
    pub fn stop(self) {
        record(self.name, self.started.elapsed(), 0);
    }

    pub fn stop_with_bytes(self, bytes: u64) {
        record(self.name, self.started.elapsed(), bytes);
    }
}

/// One span's aggregates, in the shape the WebView reads.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpanReport {
    pub name: String,
    pub calls: u64,
    pub total_ms: f64,
    pub mean_ms: f64,
    pub min_ms: f64,
    /// Median over the retained window, not over all calls.
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub bytes: u64,
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The percentile at `fraction` over a sorted slice, nearest-rank.
fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = ((sorted.len() as f64) * fraction).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

/// Every span recorded so far, slowest total first — the order that answers
/// "where did the time go" without the reader having to sort it themselves.
pub fn report() -> Vec<SpanReport> {
    let guard = match table().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut rows: Vec<SpanReport> = guard
        .iter()
        .map(|(name, entry)| {
            let mut sorted = entry.recent.clone();
            sorted.sort_unstable();
            SpanReport {
                name: (*name).to_string(),
                calls: entry.calls,
                total_ms: millis(entry.total),
                mean_ms: if entry.calls == 0 {
                    0.0
                } else {
                    millis(entry.total) / entry.calls as f64
                },
                min_ms: millis(entry.min.unwrap_or_default()),
                p50_ms: millis(percentile(&sorted, 0.50)),
                p95_ms: millis(percentile(&sorted, 0.95)),
                max_ms: millis(entry.max),
                bytes: entry.bytes,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.total_ms
            .partial_cmp(&a.total_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows
}

/// Throw the counters away, so a measurement can start from a known point.
pub fn reset() {
    let mut guard = match table().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.clear();
}

#[tauri::command]
pub fn perf_report() -> Vec<SpanReport> {
    report()
}

#[tauri::command]
pub fn perf_reset() {
    reset();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is global and `reset` clears all of it, so the tests take
    /// turns. Without this they race each other's counters and fail by
    /// coincidence, which is a worse bug than any they could catch.
    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn a_span_records_its_duration_and_returns_the_body() {
        let _turn = exclusive();
        reset();
        let out = span("test.returns", || 41 + 1);
        assert_eq!(out, 42);

        let rows = report();
        let row = rows.iter().find(|r| r.name == "test.returns").expect("row");
        assert_eq!(row.calls, 1);
        assert_eq!(row.bytes, 0);
    }

    #[test]
    fn calls_accumulate_rather_than_replace() {
        let _turn = exclusive();
        reset();
        for _ in 0..5 {
            record("test.accumulates", Duration::from_millis(10), 3);
        }
        let rows = report();
        let row = rows.iter().find(|r| r.name == "test.accumulates").expect("row");
        assert_eq!(row.calls, 5);
        assert_eq!(row.bytes, 15);
        assert!((row.total_ms - 50.0).abs() < 0.001);
        assert!((row.mean_ms - 10.0).abs() < 0.001);
    }

    #[test]
    fn min_and_max_span_every_call_not_just_the_window() {
        let _turn = exclusive();
        reset();
        record("test.extremes", Duration::from_millis(1), 0);
        // Push the first sample out of the retained window.
        for _ in 0..WINDOW {
            record("test.extremes", Duration::from_millis(10), 0);
        }
        record("test.extremes", Duration::from_millis(99), 0);

        let rows = report();
        let row = rows.iter().find(|r| r.name == "test.extremes").expect("row");
        assert_eq!(row.calls, WINDOW as u64 + 2);
        assert!((row.min_ms - 1.0).abs() < 0.001, "min survives the window");
        assert!((row.max_ms - 99.0).abs() < 0.001);
    }

    /// The window is what bounds memory, so it has to actually bound it.
    #[test]
    fn the_retained_window_does_not_grow_without_bound() {
        let _turn = exclusive();
        reset();
        for _ in 0..(WINDOW * 4) {
            record("test.bounded", Duration::from_millis(1), 0);
        }
        let guard = table().lock().expect("lock");
        assert_eq!(guard["test.bounded"].recent.len(), WINDOW);
        assert_eq!(guard["test.bounded"].calls, (WINDOW * 4) as u64);
    }

    #[test]
    fn percentiles_come_off_the_window() {
        let _turn = exclusive();
        reset();
        for value in 1..=100u64 {
            record("test.percentiles", Duration::from_millis(value), 0);
        }
        let rows = report();
        let row = rows.iter().find(|r| r.name == "test.percentiles").expect("row");
        assert!((row.p50_ms - 50.0).abs() < 0.001, "p50 was {}", row.p50_ms);
        assert!((row.p95_ms - 95.0).abs() < 0.001, "p95 was {}", row.p95_ms);
    }

    #[test]
    fn the_report_is_ordered_by_where_the_time_went() {
        let _turn = exclusive();
        reset();
        record("test.small", Duration::from_millis(1), 0);
        record("test.large", Duration::from_millis(500), 0);
        record("test.medium", Duration::from_millis(50), 0);

        let names: Vec<String> = report().into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["test.large", "test.medium", "test.small"]);
    }

    #[test]
    fn a_failing_span_still_records_its_time_and_no_bytes() {
        let _turn = exclusive();
        reset();
        let out: Result<Vec<u8>, &str> =
            span_bytes("test.fails", || Err("nope"), |v: &Vec<u8>| v.len());
        assert!(out.is_err());

        let rows = report();
        let row = rows.iter().find(|r| r.name == "test.fails").expect("row");
        assert_eq!(row.calls, 1);
        assert_eq!(row.bytes, 0);
    }

    #[test]
    fn an_empty_report_is_empty_rather_than_a_zero_row() {
        let _turn = exclusive();
        reset();
        assert!(report().iter().all(|r| !r.name.starts_with("test.empty")));
    }
}
