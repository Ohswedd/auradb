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
    /// MVCC: transactions currently holding a pinned snapshot (gauge).
    pub mvcc_active_transactions: AtomicU64,
    /// MVCC: age in seconds of the oldest pinned snapshot (gauge).
    pub mvcc_oldest_snapshot_age_seconds: AtomicU64,
    /// MVCC: estimate of retained stored versions (gauge).
    pub mvcc_retained_versions: AtomicU64,
    /// MVCC: total garbage-collection passes run (counter).
    pub mvcc_gc_runs_total: AtomicU64,
    /// MVCC: total versions reclaimed by GC (counter).
    pub mvcc_gc_reclaimed_versions_total: AtomicU64,
    /// MVCC: total bytes reclaimed by GC (counter).
    pub mvcc_gc_reclaimed_bytes_total: AtomicU64,
    /// MVCC: total transactions reaped for exceeding the idle timeout (counter).
    pub mvcc_transaction_timeouts_total: AtomicU64,
    /// MVCC: total transaction commit conflicts (counter).
    pub mvcc_conflicts_total: AtomicU64,
    /// Cluster: whether cluster (Raft) mode is enabled (gauge, 0/1).
    pub cluster_enabled: AtomicU64,
    /// Cluster: node role as a code (gauge: 0 follower, 1 candidate, 2 leader).
    pub node_role: AtomicU64,
    /// Raft: current term (gauge).
    pub raft_current_term: AtomicU64,
    /// Raft: commit index (gauge).
    pub raft_commit_index: AtomicU64,
    /// Raft: applied index (gauge).
    pub raft_applied_index: AtomicU64,
    /// Raft: last log index (gauge).
    pub raft_log_last_index: AtomicU64,
    /// Raft: cumulative leader changes (counter).
    pub raft_leader_changes_total: AtomicU64,
    /// Raft: cumulative votes granted (counter).
    pub raft_votes_granted_total: AtomicU64,
    /// Raft: cumulative AppendEntries sent (counter).
    pub raft_append_entries_sent_total: AtomicU64,
    /// Raft: cumulative AppendEntries received (counter).
    pub raft_append_entries_received_total: AtomicU64,
    /// Raft: replication lag in entries (gauge).
    pub raft_replication_lag_entries: AtomicU64,
    /// Replication: cumulative apply errors (counter).
    pub replication_apply_errors_total: AtomicU64,
    /// Multi-node preview: number of peers with an outbound connection (gauge).
    pub peer_connected: AtomicU64,
    /// Multi-node preview: max peer replication lag in entries (gauge).
    pub peer_replication_lag_entries: AtomicU64,
    /// Raft: cumulative elections started (counter).
    pub raft_elections_total: AtomicU64,
    /// Raft: cumulative election timeouts (counter).
    pub raft_election_timeouts_total: AtomicU64,
    /// Raft: cumulative AppendEntries rejected by a follower (counter).
    pub raft_append_entries_failures_total: AtomicU64,
    /// Raft: most recent heartbeat round-trip latency in ms (gauge).
    pub raft_heartbeat_latency_ms: AtomicU64,
    /// Cluster: whether a quorum is reachable (gauge, 0/1).
    pub cluster_quorum_available: AtomicU64,
    /// Multi-node preview: snapshots this node has sent as a leader (counter).
    pub cluster_snapshots_sent_total: AtomicU64,
    /// Multi-node preview: snapshots this node has installed as a follower
    /// (counter).
    pub cluster_snapshots_installed_total: AtomicU64,
    /// Multi-node preview: snapshot installs this node rejected (counter).
    pub cluster_snapshots_rejected_total: AtomicU64,
    /// Per-request latency.
    pub request_latency: Histogram,
    /// Query execution latency.
    pub query_latency: Histogram,
    /// Storage (mutation commit) latency.
    pub storage_latency: Histogram,
    /// Raft apply latency.
    pub raft_apply_latency: Histogram,
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

    /// Refresh the cluster/Raft gauges and counters from a status snapshot.
    ///
    /// The Raft node owns the live counters; the server calls this to mirror them
    /// into the exported registry. `role_code` is 0 follower / 1 candidate / 2
    /// leader. The `*_total` values are absolute cumulative counts.
    #[allow(clippy::too_many_arguments)]
    pub fn set_cluster(
        &self,
        enabled: bool,
        role_code: u64,
        term: u64,
        commit_index: u64,
        applied_index: u64,
        last_log_index: u64,
        replication_lag_entries: u64,
        leader_changes_total: u64,
        votes_granted_total: u64,
        append_entries_sent_total: u64,
        append_entries_received_total: u64,
        apply_errors_total: u64,
    ) {
        Metrics::gauge_set(&self.cluster_enabled, enabled as u64);
        Metrics::gauge_set(&self.node_role, role_code);
        Metrics::gauge_set(&self.raft_current_term, term);
        Metrics::gauge_set(&self.raft_commit_index, commit_index);
        Metrics::gauge_set(&self.raft_applied_index, applied_index);
        Metrics::gauge_set(&self.raft_log_last_index, last_log_index);
        Metrics::gauge_set(&self.raft_replication_lag_entries, replication_lag_entries);
        Metrics::gauge_set(&self.raft_leader_changes_total, leader_changes_total);
        Metrics::gauge_set(&self.raft_votes_granted_total, votes_granted_total);
        Metrics::gauge_set(
            &self.raft_append_entries_sent_total,
            append_entries_sent_total,
        );
        Metrics::gauge_set(
            &self.raft_append_entries_received_total,
            append_entries_received_total,
        );
        Metrics::gauge_set(&self.replication_apply_errors_total, apply_errors_total);
    }

    /// Mirror the multi-node preview's peer/Raft counters into the registry.
    /// A no-op for single-node clusters (which report no peers).
    #[allow(clippy::too_many_arguments)]
    pub fn set_peer_metrics(
        &self,
        peers_connected: u64,
        peer_replication_lag_entries: u64,
        elections_total: u64,
        election_timeouts_total: u64,
        append_entries_failures_total: u64,
        heartbeat_latency_ms: u64,
        quorum_available: bool,
        snapshots_sent_total: u64,
        snapshots_installed_total: u64,
        snapshots_rejected_total: u64,
    ) {
        Metrics::gauge_set(&self.cluster_snapshots_sent_total, snapshots_sent_total);
        Metrics::gauge_set(
            &self.cluster_snapshots_installed_total,
            snapshots_installed_total,
        );
        Metrics::gauge_set(
            &self.cluster_snapshots_rejected_total,
            snapshots_rejected_total,
        );
        Metrics::gauge_set(&self.peer_connected, peers_connected);
        Metrics::gauge_set(
            &self.peer_replication_lag_entries,
            peer_replication_lag_entries,
        );
        Metrics::gauge_set(&self.raft_elections_total, elections_total);
        Metrics::gauge_set(&self.raft_election_timeouts_total, election_timeouts_total);
        Metrics::gauge_set(
            &self.raft_append_entries_failures_total,
            append_entries_failures_total,
        );
        Metrics::gauge_set(&self.raft_heartbeat_latency_ms, heartbeat_latency_ms);
        Metrics::gauge_set(&self.cluster_quorum_available, quorum_available as u64);
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
            mvcc_active_transactions: self.mvcc_active_transactions.load(Ordering::Relaxed),
            mvcc_oldest_snapshot_age_seconds: self
                .mvcc_oldest_snapshot_age_seconds
                .load(Ordering::Relaxed),
            mvcc_retained_versions: self.mvcc_retained_versions.load(Ordering::Relaxed),
            mvcc_gc_runs_total: self.mvcc_gc_runs_total.load(Ordering::Relaxed),
            mvcc_gc_reclaimed_versions_total: self
                .mvcc_gc_reclaimed_versions_total
                .load(Ordering::Relaxed),
            mvcc_gc_reclaimed_bytes_total: self
                .mvcc_gc_reclaimed_bytes_total
                .load(Ordering::Relaxed),
            mvcc_transaction_timeouts_total: self
                .mvcc_transaction_timeouts_total
                .load(Ordering::Relaxed),
            mvcc_conflicts_total: self.mvcc_conflicts_total.load(Ordering::Relaxed),
            cluster_enabled: self.cluster_enabled.load(Ordering::Relaxed),
            node_role: self.node_role.load(Ordering::Relaxed),
            raft_current_term: self.raft_current_term.load(Ordering::Relaxed),
            raft_commit_index: self.raft_commit_index.load(Ordering::Relaxed),
            raft_applied_index: self.raft_applied_index.load(Ordering::Relaxed),
            raft_log_last_index: self.raft_log_last_index.load(Ordering::Relaxed),
            raft_leader_changes_total: self.raft_leader_changes_total.load(Ordering::Relaxed),
            raft_votes_granted_total: self.raft_votes_granted_total.load(Ordering::Relaxed),
            raft_append_entries_sent_total: self
                .raft_append_entries_sent_total
                .load(Ordering::Relaxed),
            raft_append_entries_received_total: self
                .raft_append_entries_received_total
                .load(Ordering::Relaxed),
            raft_replication_lag_entries: self.raft_replication_lag_entries.load(Ordering::Relaxed),
            replication_apply_errors_total: self
                .replication_apply_errors_total
                .load(Ordering::Relaxed),
            peer_connected: self.peer_connected.load(Ordering::Relaxed),
            peer_replication_lag_entries: self.peer_replication_lag_entries.load(Ordering::Relaxed),
            raft_elections_total: self.raft_elections_total.load(Ordering::Relaxed),
            raft_election_timeouts_total: self.raft_election_timeouts_total.load(Ordering::Relaxed),
            raft_append_entries_failures_total: self
                .raft_append_entries_failures_total
                .load(Ordering::Relaxed),
            raft_heartbeat_latency_ms: self.raft_heartbeat_latency_ms.load(Ordering::Relaxed),
            cluster_quorum_available: self.cluster_quorum_available.load(Ordering::Relaxed),
            cluster_snapshots_sent_total: self.cluster_snapshots_sent_total.load(Ordering::Relaxed),
            cluster_snapshots_installed_total: self
                .cluster_snapshots_installed_total
                .load(Ordering::Relaxed),
            cluster_snapshots_rejected_total: self
                .cluster_snapshots_rejected_total
                .load(Ordering::Relaxed),
            request_latency: self.request_latency.snapshot(),
            query_latency: self.query_latency.snapshot(),
            storage_latency: self.storage_latency.snapshot(),
            raft_apply_latency: self.raft_apply_latency.snapshot(),
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
    /// MVCC: transactions currently holding a pinned snapshot.
    pub mvcc_active_transactions: u64,
    /// MVCC: age in seconds of the oldest pinned snapshot.
    pub mvcc_oldest_snapshot_age_seconds: u64,
    /// MVCC: estimate of retained stored versions.
    pub mvcc_retained_versions: u64,
    /// MVCC: total garbage-collection passes run.
    pub mvcc_gc_runs_total: u64,
    /// MVCC: total versions reclaimed by GC.
    pub mvcc_gc_reclaimed_versions_total: u64,
    /// MVCC: total bytes reclaimed by GC.
    pub mvcc_gc_reclaimed_bytes_total: u64,
    /// MVCC: total transactions reaped for exceeding the idle timeout.
    pub mvcc_transaction_timeouts_total: u64,
    /// MVCC: total transaction commit conflicts.
    pub mvcc_conflicts_total: u64,
    /// Cluster: whether cluster mode is enabled (0/1).
    pub cluster_enabled: u64,
    /// Cluster: node role code (0 follower, 1 candidate, 2 leader).
    pub node_role: u64,
    /// Raft: current term.
    pub raft_current_term: u64,
    /// Raft: commit index.
    pub raft_commit_index: u64,
    /// Raft: applied index.
    pub raft_applied_index: u64,
    /// Raft: last log index.
    pub raft_log_last_index: u64,
    /// Raft: cumulative leader changes.
    pub raft_leader_changes_total: u64,
    /// Raft: cumulative votes granted.
    pub raft_votes_granted_total: u64,
    /// Raft: cumulative AppendEntries sent.
    pub raft_append_entries_sent_total: u64,
    /// Raft: cumulative AppendEntries received.
    pub raft_append_entries_received_total: u64,
    /// Raft: replication lag in entries.
    pub raft_replication_lag_entries: u64,
    /// Replication: cumulative apply errors.
    pub replication_apply_errors_total: u64,
    /// Multi-node preview: peers with an outbound connection.
    pub peer_connected: u64,
    /// Multi-node preview: max peer replication lag in entries.
    pub peer_replication_lag_entries: u64,
    /// Raft: cumulative elections started.
    pub raft_elections_total: u64,
    /// Raft: cumulative election timeouts.
    pub raft_election_timeouts_total: u64,
    /// Raft: cumulative AppendEntries rejected by a follower.
    pub raft_append_entries_failures_total: u64,
    /// Raft: most recent heartbeat round-trip latency in ms.
    pub raft_heartbeat_latency_ms: u64,
    /// Cluster: whether a quorum is reachable (0/1).
    pub cluster_quorum_available: u64,
    /// Cluster: snapshots sent to followers as a leader.
    pub cluster_snapshots_sent_total: u64,
    /// Cluster: snapshots installed from a leader as a follower.
    pub cluster_snapshots_installed_total: u64,
    /// Cluster: snapshot installs rejected (validation failure).
    pub cluster_snapshots_rejected_total: u64,
    /// Request latency histogram.
    pub request_latency: HistogramSnapshot,
    /// Query latency histogram.
    pub query_latency: HistogramSnapshot,
    /// Storage latency histogram.
    pub storage_latency: HistogramSnapshot,
    /// Raft apply latency histogram.
    pub raft_apply_latency: HistogramSnapshot,
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
        counter("auradb_mvcc_gc_runs_total", self.mvcc_gc_runs_total);
        counter(
            "auradb_mvcc_gc_reclaimed_versions_total",
            self.mvcc_gc_reclaimed_versions_total,
        );
        counter(
            "auradb_mvcc_gc_reclaimed_bytes_total",
            self.mvcc_gc_reclaimed_bytes_total,
        );
        counter(
            "auradb_mvcc_transaction_timeouts_total",
            self.mvcc_transaction_timeouts_total,
        );
        counter("auradb_mvcc_conflicts_total", self.mvcc_conflicts_total);
        counter(
            "auradb_raft_leader_changes_total",
            self.raft_leader_changes_total,
        );
        counter(
            "auradb_raft_votes_granted_total",
            self.raft_votes_granted_total,
        );
        counter(
            "auradb_raft_append_entries_sent_total",
            self.raft_append_entries_sent_total,
        );
        counter(
            "auradb_raft_append_entries_received_total",
            self.raft_append_entries_received_total,
        );
        counter(
            "auradb_replication_apply_errors_total",
            self.replication_apply_errors_total,
        );
        counter("auradb_raft_elections_total", self.raft_elections_total);
        counter(
            "auradb_raft_election_timeouts_total",
            self.raft_election_timeouts_total,
        );
        counter(
            "auradb_raft_append_entries_failures_total",
            self.raft_append_entries_failures_total,
        );
        let mut gauge = |name: &str, value: u64| {
            out.push_str(&format!("# TYPE {name} gauge\n{name} {value}\n"));
        };
        gauge("auradb_peer_connected", self.peer_connected);
        gauge(
            "auradb_peer_replication_lag_entries",
            self.peer_replication_lag_entries,
        );
        gauge(
            "auradb_raft_heartbeat_latency_ms",
            self.raft_heartbeat_latency_ms,
        );
        gauge(
            "auradb_cluster_quorum_available",
            self.cluster_quorum_available,
        );
        gauge(
            "auradb_cluster_snapshots_sent_total",
            self.cluster_snapshots_sent_total,
        );
        gauge(
            "auradb_cluster_snapshots_installed_total",
            self.cluster_snapshots_installed_total,
        );
        gauge(
            "auradb_cluster_snapshots_rejected_total",
            self.cluster_snapshots_rejected_total,
        );
        gauge("auradb_cluster_enabled", self.cluster_enabled);
        gauge("auradb_node_role", self.node_role);
        gauge("auradb_raft_current_term", self.raft_current_term);
        gauge("auradb_raft_commit_index", self.raft_commit_index);
        gauge("auradb_raft_applied_index", self.raft_applied_index);
        gauge("auradb_raft_log_last_index", self.raft_log_last_index);
        gauge(
            "auradb_raft_replication_lag_entries",
            self.raft_replication_lag_entries,
        );
        gauge("auradb_active_connections", self.active_connections);
        gauge("auradb_active_transactions", self.active_transactions);
        gauge("auradb_active_cursors", self.active_cursors);
        gauge(
            "auradb_mvcc_active_transactions",
            self.mvcc_active_transactions,
        );
        gauge(
            "auradb_mvcc_oldest_snapshot_age_seconds",
            self.mvcc_oldest_snapshot_age_seconds,
        );
        gauge("auradb_mvcc_retained_versions", self.mvcc_retained_versions);
        for (label, h) in [
            ("request", &self.request_latency),
            ("query", &self.query_latency),
            ("storage", &self.storage_latency),
            ("raft_apply", &self.raft_apply_latency),
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

    #[test]
    fn metrics_export_includes_raft_metrics() {
        let m = Metrics::new();
        m.set_cluster(true, 2, 5, 10, 9, 11, 1, 3, 7, 100, 95, 0);
        let snap = m.snapshot();
        let prom = snap.render_prometheus();
        for name in [
            "auradb_cluster_enabled",
            "auradb_node_role",
            "auradb_raft_current_term",
            "auradb_raft_commit_index",
            "auradb_raft_applied_index",
            "auradb_raft_log_last_index",
            "auradb_raft_leader_changes_total",
            "auradb_raft_votes_granted_total",
            "auradb_raft_append_entries_sent_total",
            "auradb_raft_append_entries_received_total",
            "auradb_raft_replication_lag_entries",
            "auradb_replication_apply_errors_total",
            "auradb_raft_apply_latency_us",
        ] {
            assert!(prom.contains(name), "missing raft metric {name}");
        }
        assert!(prom.contains("auradb_cluster_enabled 1"));
        assert!(prom.contains("auradb_node_role 2"));
        assert!(prom.contains("auradb_raft_commit_index 10"));
    }

    #[test]
    fn metrics_export_includes_mvcc_metrics() {
        let m = Metrics::new();
        Metrics::gauge_set(&m.mvcc_active_transactions, 3);
        Metrics::add(&m.mvcc_gc_reclaimed_versions_total, 42);
        Metrics::incr(&m.mvcc_transaction_timeouts_total);
        Metrics::incr(&m.mvcc_conflicts_total);
        let snap = m.snapshot();
        let prom = snap.render_prometheus();
        for name in [
            "auradb_mvcc_active_transactions",
            "auradb_mvcc_oldest_snapshot_age_seconds",
            "auradb_mvcc_retained_versions",
            "auradb_mvcc_gc_runs_total",
            "auradb_mvcc_gc_reclaimed_versions_total",
            "auradb_mvcc_gc_reclaimed_bytes_total",
            "auradb_mvcc_transaction_timeouts_total",
            "auradb_mvcc_conflicts_total",
        ] {
            assert!(prom.contains(name), "missing metric {name}");
        }
        assert!(prom.contains("auradb_mvcc_active_transactions 3"));
        assert!(prom.contains("auradb_mvcc_gc_reclaimed_versions_total 42"));
        let json = snap.to_json();
        assert!(json.contains("mvcc_conflicts_total"));
    }
}
