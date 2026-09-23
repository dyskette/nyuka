//! The counters behind the two objectives (ADR-0019).
//!
//! In-process, reset per window, and summarized into the log stream by the
//! scheduler. ADR-0014 keeps telemetry as JSON on stdout with a `50m × 10`
//! rotation, so a monthly rate cannot be computed by reading a month of raw
//! lines — they are gone. A summary line survives as history.
//!
//! # Two rates, because one would be ignored
//!
//! A job fails for reasons inside this system — the disk filled, a bug — and
//! for reasons outside it: the site removed the chapter, or blocked us. Only
//! the first kind has an action. Counting both in one number means the first
//! time a popular source has a bad week the objective goes red and stays red,
//! and the next person to look at it learns the number means nothing.
//!
//! So both are counted and reported separately. The gap between the rates is
//! its own signal: a widening one means the configured sources are rotting.

use std::sync::Mutex;
use std::time::Duration;

use nyuka_domain::DomainError;
use nyuka_domain::model::JobKind;

/// How many duration samples one window keeps.
///
/// Bounded because this is memory that grows with traffic. At the volumes
/// ADR-0019 assumes — a few thousand jobs a month — an hour never reaches
/// this, so the percentile is exact in practice. When it is reached, the
/// snapshot says so rather than reporting a silently truncated percentile as
/// though it were authoritative.
pub const MAX_SAMPLES: usize = 4096;

/// Priority for work someone is waiting on.
///
/// ADR-0019's second objective measures user-requested downloads only, and
/// this is how they are told apart from ones a follow check discovered.
/// A constant rather than a literal at each call site, because the objective
/// silently changes meaning if the two drift.
pub const PRIORITY_INTERACTIVE: i16 = 0;

/// Priority for work discovered by the scheduler. Nobody is waiting.
pub const PRIORITY_BACKGROUND: i16 = 5;

/// How a job outcome is counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    /// A failure inside this system. Counts against SLO-1.
    ServiceFault,
    /// A failure outside it. Counted, reported, excluded from SLO-1.
    SourceFault,
}

/// Classifies a failure (ADR-0019).
///
/// The default for anything unrecognised is `ServiceFault`, which is the
/// right way to be wrong: a new error variant that is actually a source's
/// problem shows up as a missed objective and gets classified, whereas the
/// reverse would hide a real fault.
pub fn classify(error: &DomainError) -> Outcome {
    match error {
        // The site was down, blocked us, or removed the chapter. Nothing in
        // this codebase changes that.
        DomainError::Source { .. } => Outcome::SourceFault,

        // Not a failure at all: ADR-0004 requires the capability check at
        // install time, so reaching this during a job means the check was
        // bypassed — which is a bug and will surface as `Internal`.
        DomainError::UnsupportedCapability(_) => Outcome::SourceFault,

        DomainError::Storage(_)
        | DomainError::InsufficientStorage
        | DomainError::Internal(_)
        | DomainError::Invalid(_)
        | DomainError::Conflict(_)
        | DomainError::NotFound => Outcome::ServiceFault,
    }
}

/// One window's summary.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct Snapshot {
    pub window_secs: u64,
    pub jobs_succeeded: u64,
    pub jobs_failed_service: u64,
    pub jobs_failed_source: u64,

    pub ttfp_samples: usize,
    pub ttfp_p50_ms: u64,
    pub ttfp_p95_ms: u64,
    pub ttfp_max_ms: u64,
    /// True when the sample hit [`MAX_SAMPLES`].
    ///
    /// A percentile from a capped sample is a lower bound. Saying so is the
    /// difference between a number that is approximate and a number that is
    /// wrong while reading as authoritative.
    pub ttfp_truncated: bool,
}

impl Snapshot {
    /// Outcomes that count toward SLO-1.
    pub fn attributable_total(&self) -> u64 {
        self.jobs_succeeded + self.jobs_failed_service
    }

    /// The success rate SLO-1 is defined on, or `None` when the window had no
    /// attributable outcomes.
    ///
    /// `None` rather than 1.0: an idle hour is not a perfect hour, and
    /// averaging a fabricated 100% across a month would make a real dip
    /// disappear.
    pub fn service_success_rate(&self) -> Option<f64> {
        let total = self.attributable_total();
        (total > 0).then(|| self.jobs_succeeded as f64 / total as f64)
    }
}

#[derive(Debug, Default)]
struct Window {
    succeeded: u64,
    failed_service: u64,
    failed_source: u64,
    /// Milliseconds, unsorted. Sorted once when the snapshot is taken.
    ttfp: Vec<u64>,
    truncated: bool,
}

/// The process-wide counters.
#[derive(Debug, Default)]
pub struct JobMetrics {
    window: Mutex<Window>,
}

impl JobMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a job that finished.
    pub fn record(&self, outcome: Outcome) {
        let mut window = self.window.lock().expect("metrics lock");
        match outcome {
            Outcome::Succeeded => window.succeeded += 1,
            Outcome::ServiceFault => window.failed_service += 1,
            Outcome::SourceFault => window.failed_source += 1,
        }
    }

    /// Records the time from a user's request to a readable chapter.
    ///
    /// Only user-requested downloads reach here; see [`PRIORITY_INTERACTIVE`].
    pub fn record_time_to_first_page(&self, elapsed: Duration) {
        let mut window = self.window.lock().expect("metrics lock");
        if window.ttfp.len() >= MAX_SAMPLES {
            window.truncated = true;
            return;
        }
        window
            .ttfp
            .push(elapsed.as_millis().min(u64::MAX as u128) as u64);
    }

    /// Takes the window's summary and starts a new one.
    ///
    /// Taking and resetting are one operation on purpose: doing them
    /// separately leaves a gap in which a job can be counted into a window
    /// that has already been reported, and that job is then never counted at
    /// all.
    pub fn take(&self, window_secs: u64) -> Snapshot {
        let taken = {
            let mut window = self.window.lock().expect("metrics lock");
            std::mem::take(&mut *window)
        };

        let mut samples = taken.ttfp;
        samples.sort_unstable();

        Snapshot {
            window_secs,
            jobs_succeeded: taken.succeeded,
            jobs_failed_service: taken.failed_service,
            jobs_failed_source: taken.failed_source,
            ttfp_samples: samples.len(),
            ttfp_p50_ms: percentile(&samples, 50),
            ttfp_p95_ms: percentile(&samples, 95),
            ttfp_max_ms: samples.last().copied().unwrap_or(0),
            ttfp_truncated: taken.truncated,
        }
    }
}

/// The nearest-rank percentile of a sorted slice.
///
/// Nearest-rank rather than interpolated: with tens of samples an hour,
/// interpolation invents a value between two real measurements and reads as
/// more precise than the data supports.
fn percentile(sorted: &[u64], p: u64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    // ceil(p/100 * n), clamped into the slice.
    let rank = ((p * sorted.len() as u64).div_ceil(100)).max(1) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

/// Whether a job is one a person is waiting for.
pub fn is_interactive(kind: JobKind, priority: i16) -> bool {
    matches!(kind, JobKind::DownloadChapter) && priority <= PRIORITY_INTERACTIVE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_failure_does_not_count_against_the_objective() {
        for error in [
            DomainError::Source {
                message: "404".into(),
                retryable: false,
            },
            DomainError::UnsupportedCapability("canvas".into()),
        ] {
            assert_eq!(
                classify(&error),
                Outcome::SourceFault,
                "nothing in this codebase changes {error}"
            );
        }
    }

    #[test]
    fn a_failure_inside_this_system_does() {
        for error in [
            DomainError::Storage("connection refused".into()),
            DomainError::InsufficientStorage,
            DomainError::Internal("bug".into()),
            DomainError::Invalid("unrunnable payload".into()),
        ] {
            assert_eq!(classify(&error), Outcome::ServiceFault, "{error}");
        }
    }

    #[test]
    fn the_two_rates_are_kept_apart() {
        let metrics = JobMetrics::new();
        for _ in 0..97 {
            metrics.record(Outcome::Succeeded);
        }
        metrics.record(Outcome::ServiceFault);
        for _ in 0..50 {
            metrics.record(Outcome::SourceFault);
        }

        let snapshot = metrics.take(3600);
        assert_eq!(snapshot.jobs_succeeded, 97);
        assert_eq!(snapshot.jobs_failed_service, 1);
        assert_eq!(snapshot.jobs_failed_source, 50);

        // The point of the split: fifty dead-source jobs must not move the
        // number the objective is defined on.
        assert_eq!(snapshot.attributable_total(), 98);
        let rate = snapshot.service_success_rate().expect("a rate");
        assert!(
            rate > 0.98,
            "source faults leaked into the attributable rate: {rate}"
        );
    }

    /// An idle hour is not a perfect hour, and averaging a fabricated 100%
    /// across a month would make a real dip disappear.
    #[test]
    fn an_empty_window_has_no_rate_rather_than_a_perfect_one() {
        let snapshot = JobMetrics::new().take(3600);
        assert_eq!(snapshot.service_success_rate(), None);
        assert_eq!(snapshot.attributable_total(), 0);
    }

    /// A source fault alone must not manufacture a rate either: there is
    /// still nothing attributable to divide by.
    #[test]
    fn a_window_of_only_source_faults_has_no_rate() {
        let metrics = JobMetrics::new();
        metrics.record(Outcome::SourceFault);
        let snapshot = metrics.take(3600);
        assert_eq!(snapshot.jobs_failed_source, 1);
        assert_eq!(snapshot.service_success_rate(), None);
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&sorted, 50), 50);
        assert_eq!(percentile(&sorted, 95), 95);
        assert_eq!(percentile(&sorted, 100), 100);
    }

    #[test]
    fn a_single_sample_is_every_percentile() {
        let sorted = vec![42];
        assert_eq!(percentile(&sorted, 50), 42);
        assert_eq!(percentile(&sorted, 95), 42);
    }

    #[test]
    fn an_empty_sample_reports_zero_rather_than_panicking() {
        assert_eq!(percentile(&[], 95), 0);
    }

    #[test]
    fn durations_are_summarized_in_order() {
        let metrics = JobMetrics::new();
        // Deliberately out of order: the snapshot sorts, the caller does not.
        for ms in [900, 100, 500, 50_000, 300] {
            metrics.record_time_to_first_page(Duration::from_millis(ms));
        }

        let snapshot = metrics.take(3600);
        assert_eq!(snapshot.ttfp_samples, 5);
        assert_eq!(snapshot.ttfp_max_ms, 50_000);
        assert_eq!(snapshot.ttfp_p50_ms, 500);
        assert!(!snapshot.ttfp_truncated);
    }

    /// A percentile from a capped sample is a lower bound, and the snapshot
    /// has to say so or it reads as authoritative.
    #[test]
    fn a_truncated_sample_says_so() {
        let metrics = JobMetrics::new();
        for _ in 0..(MAX_SAMPLES + 10) {
            metrics.record_time_to_first_page(Duration::from_millis(1));
        }

        let snapshot = metrics.take(3600);
        assert_eq!(snapshot.ttfp_samples, MAX_SAMPLES);
        assert!(
            snapshot.ttfp_truncated,
            "a silently capped percentile is worse than no percentile"
        );
    }

    /// Taking a snapshot starts a fresh window, or every hour would report
    /// the running total since boot.
    #[test]
    fn taking_a_snapshot_resets_the_window() {
        let metrics = JobMetrics::new();
        metrics.record(Outcome::Succeeded);
        metrics.record_time_to_first_page(Duration::from_millis(10));

        let first = metrics.take(3600);
        assert_eq!(first.jobs_succeeded, 1);
        assert_eq!(first.ttfp_samples, 1);

        let second = metrics.take(3600);
        assert_eq!(second.jobs_succeeded, 0);
        assert_eq!(second.ttfp_samples, 0);
        assert!(!second.ttfp_truncated, "the truncation flag resets too");
    }

    /// SLO-2 measures user-requested downloads. Including follow-discovered
    /// ones would measure the scheduler's pacing rather than a person's wait.
    #[test]
    fn only_user_requested_downloads_are_timed() {
        assert!(is_interactive(
            JobKind::DownloadChapter,
            PRIORITY_INTERACTIVE
        ));
        assert!(
            !is_interactive(JobKind::DownloadChapter, PRIORITY_BACKGROUND),
            "nobody is waiting for a follow sweep's download"
        );
        assert!(
            !is_interactive(JobKind::CheckFollow, PRIORITY_INTERACTIVE),
            "only a download has a first page"
        );
    }

    /// The constants are what keeps the two call sites from drifting apart.
    #[test]
    fn interactive_outranks_background() {
        const { assert!(PRIORITY_INTERACTIVE < PRIORITY_BACKGROUND) };
    }

    #[test]
    fn a_snapshot_serializes_with_the_documented_field_names() {
        let json = serde_json::to_value(Snapshot {
            window_secs: 3600,
            jobs_succeeded: 412,
            jobs_failed_service: 1,
            jobs_failed_source: 37,
            ttfp_samples: 38,
            ttfp_p50_ms: 18_400,
            ttfp_p95_ms: 52_100,
            ttfp_max_ms: 71_200,
            ttfp_truncated: false,
        })
        .expect("serialize");

        // ADR-0019 publishes these names and the runbook's jq recipes read
        // them. Renaming one silently breaks a query nobody runs until an
        // incident.
        for field in [
            "window_secs",
            "jobs_succeeded",
            "jobs_failed_service",
            "jobs_failed_source",
            "ttfp_p50_ms",
            "ttfp_p95_ms",
            "ttfp_truncated",
        ] {
            assert!(json.get(field).is_some(), "{field} is missing");
        }
    }
}
