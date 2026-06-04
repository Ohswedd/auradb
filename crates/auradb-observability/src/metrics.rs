//! A lightweight, dependency-free metrics registry: atomic counters, gauges,
//! and fixed-bucket latency histograms with JSON and Prometheus-text export.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

/// Fixed microsecond latency buckets (upper bounds).
const BUCKETS_US: [u64; 12] = [
    50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 25_000, 50_000, 100_000, 1_000_000,
];

/// A fixed-bucket latency histogram recording microsecond observations.
#[derive(Debug, Default)]
pub struct Histogram {
    buckets: [AtomicU64; 12],
    sum_us: AtomicU64,
    count: AtomicU64,
}

impl Histogram {
    /// Record a duration in microseconds.
    pub fn record_us(&self, value_us: u64) {
        self.sum_us.fetch_add(value_us, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        for (i, bound) in BUCKETS_US.iter().enumerate() {
            if value_us <= *bound {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
                break;
            }
        }
    }

    /// Snapshot the histogram.
    pub fn snapshot(&self) -> HistogramSnapshot {
        HistogramSnapshot {
            buckets: BUCKETS_US
                .iter()
                .enumerate()
                .map(|(i, bound)| (*bound, self.buckets[i].load(Ordering::Relaxed)))
                .collect(),
            sum_us: self.sum_us.load(Ordering::Relaxed),
            count: self.count.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time snapshot of a [`Histogram`].
#[derive(Debug, Clone, Serialize)]
pub struct HistogramSnapshot {
    /// `(upper_bound_us, cumulative_count_in_bucket)` pairs.
    pub buckets: Vec<(u64, u64)>,
    /// Sum of all recorded microsecond values.
    pub sum_us: u64,
    /// Number of observations.
    pub count: u64,
}

impl HistogramSnapshot {
    /// Mean latency in microseconds, or 0 when there are no observations.
    pub fn mean_us(&self) -> u64 {
        self.sum_us.checked_div(self.count).unwrap_or(0)
    }
}

/// The server metrics registry.
#[derive(Debug, Default)]
pub struct Metrics {
    /// Total requests handled.
    pub requests_total: AtomicU64,
    /// Total error responses.
    pub errors_total: AtomicU64,
    /// Total read queries executed.
    pub queries_total: AtomicU64,
    /// Total mutations executed.
    pub mutations_total: AtomicU64,
    /// Total failed authentication attempts (invalid or missing credentials).
    pub auth_failures_total: AtomicU64,
    /// Bytes read from the wire.
    pub bytes_read: AtomicU64,
    /// Bytes written to the wire.
    pub bytes_written: AtomicU64,
    /// Currently active connections (gauge).
    pub active_connections: AtomicU64,
    /// Currently open transactions (gauge).
    pub active_transactions: AtomicU64,
    /// Currently open cursors (gauge).
    pub active_cursors: AtomicU64,
    /// Per-request latency.
    pub request_latency: Histogram,
    /// Query execution latency.
    pub query_latency: Histogram,
    /// Storage (mutation commit) latency.
    pub storage_latency: Histogram,
}

impl Metrics {
    /// Create a fresh registry.
    pub fn new() -> Self {
        Metrics::default()
    }

    /// Increment a counter by one.
    pub fn incr(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Add to a counter.
    pub fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    /// Increment a gauge.
    pub fn gauge_inc(gauge: &AtomicU64) {
        gauge.fetch_add(1, Ordering::Relaxed);
    }

    /// Set a gauge to an absolute value.
    pub fn gauge_set(gauge: &AtomicU64, value: u64) {
        gauge.store(value, Ordering::Relaxed);
    }

    /// Decrement a gauge (saturating at zero).
    pub fn gauge_dec(gauge: &AtomicU64) {
        let _ = gauge.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_sub(1))
        });
    }

    /// Capture a snapshot of all metrics.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            errors_total: self.errors_total.load(Ordering::Relaxed),
            queries_total: self.queries_total.load(Ordering::Relaxed),
            mutations_total: self.mutations_total.load(Ordering::Relaxed),
            auth_failures_total: self.auth_failures_total.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
            active_transactions: self.active_transactions.load(Ordering::Relaxed),
            active_cursors: self.active_cursors.load(Ordering::Relaxed),
            request_latency: self.request_latency.snapshot(),
            query_latency: self.query_latency.snapshot(),
            storage_latency: self.storage_latency.snapshot(),
        }
    }
}

/// A serializable snapshot of the metrics registry.
#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    /// Total requests handled.
    pub requests_total: u64,
    /// Total error responses.
    pub errors_total: u64,
    /// Total read queries.
    pub queries_total: u64,
    /// Total mutations.
    pub mutations_total: u64,
    /// Total failed authentication attempts.
    pub auth_failures_total: u64,
    /// Bytes read.
    pub bytes_read: u64,
    /// Bytes written.
    pub bytes_written: u64,
    /// Active connections.
    pub active_connections: u64,
    /// Active transactions.
    pub active_transactions: u64,
    /// Active cursors.
    pub active_cursors: u64,
    /// Request latency histogram.
    pub request_latency: HistogramSnapshot,
    /// Query latency histogram.
    pub query_latency: HistogramSnapshot,
    /// Storage latency histogram.
    pub storage_latency: HistogramSnapshot,
}

impl MetricsSnapshot {
    /// Render in a minimal Prometheus text exposition format.
    pub fn render_prometheus(&self) -> String {
        let mut out = String::new();
        let mut counter = |name: &str, value: u64| {
            out.push_str(&format!("# TYPE {name} counter\n{name} {value}\n"));
        };
        counter("auradb_requests_total", self.requests_total);
        counter("auradb_errors_total", self.errors_total);
        counter("auradb_queries_total", self.queries_total);
        counter("auradb_mutations_total", self.mutations_total);
        counter("auradb_auth_failures_total", self.auth_failures_total);
        counter("auradb_bytes_read_total", self.bytes_read);
        counter("auradb_bytes_written_total", self.bytes_written);
        let mut gauge = |name: &str, value: u64| {
            out.push_str(&format!("# TYPE {name} gauge\n{name} {value}\n"));
        };
        gauge("auradb_active_connections", self.active_connections);
        gauge("auradb_active_transactions", self.active_transactions);
        gauge("auradb_active_cursors", self.active_cursors);
        for (label, h) in [
            ("request", &self.request_latency),
            ("query", &self.query_latency),
            ("storage", &self.storage_latency),
        ] {
            let name = format!("auradb_{label}_latency_us");
            out.push_str(&format!("# TYPE {name} summary\n"));
            out.push_str(&format!("{name}_sum {}\n", h.sum_us));
            out.push_str(&format!("{name}_count {}\n", h.count));
        }
        out
    }

    /// Render as a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_and_gauges() {
        let m = Metrics::new();
        Metrics::incr(&m.requests_total);
        Metrics::add(&m.bytes_read, 100);
        Metrics::gauge_inc(&m.active_connections);
        Metrics::gauge_inc(&m.active_connections);
        Metrics::gauge_dec(&m.active_connections);
        let snap = m.snapshot();
        assert_eq!(snap.requests_total, 1);
        assert_eq!(snap.bytes_read, 100);
        assert_eq!(snap.active_connections, 1);
    }

    #[test]
    fn gauge_saturates_at_zero() {
        let m = Metrics::new();
        Metrics::gauge_dec(&m.active_cursors);
        assert_eq!(m.snapshot().active_cursors, 0);
    }

    #[test]
    fn histogram_records() {
        let m = Metrics::new();
        m.query_latency.record_us(120);
        m.query_latency.record_us(3000);
        let snap = m.snapshot();
        assert_eq!(snap.query_latency.count, 2);
        assert_eq!(snap.query_latency.mean_us(), (120 + 3000) / 2);
    }

    #[test]
    fn prometheus_and_json_render() {
        let m = Metrics::new();
        Metrics::incr(&m.requests_total);
        let snap = m.snapshot();
        assert!(snap.render_prometheus().contains("auradb_requests_total 1"));
        assert!(snap.to_json().contains("requests_total"));
    }
}
