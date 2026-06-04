//! # auradb-observability
//!
//! Tracing setup and a lightweight metrics registry ([`Metrics`]). Metrics are
//! dependency-free atomic counters, gauges, and fixed-bucket latency histograms
//! with JSON and Prometheus-text export. No external collector is required to
//! run the server.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod metrics;

use std::sync::Once;

pub use metrics::{Histogram, HistogramSnapshot, Metrics, MetricsSnapshot};

static INIT: Once = Once::new();

/// Initialize the global tracing subscriber.
///
/// `level` is an env-filter directive such as `info` or `auradb=debug`. When
/// `json` is true, logs are emitted as structured JSON; otherwise as
/// human-readable text. Safe to call more than once (subsequent calls are
/// no-ops), which keeps tests and the CLI simple.
pub fn init_tracing(level: &str, json: bool) {
    INIT.call_once(|| {
        use tracing_subscriber::{fmt, EnvFilter};
        let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
        let builder = fmt().with_env_filter(filter);
        if json {
            let _ = builder.json().try_init();
        } else {
            let _ = builder.try_init();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        init_tracing("info", false);
        init_tracing("debug", true);
        // No panic on repeated init.
    }
}
