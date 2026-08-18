use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use prometheus::{IntCounter, IntGauge, Opts, Registry, TextEncoder};

/// Health fails if no successful push has landed within this many seconds.
pub const HEALTH_MAX_STALE_SECS: u64 = 90;

#[derive(Clone)]
pub struct PusherMetrics {
    registry: Registry,
    pushes_succeeded: IntCounter,
    pushes_failed: IntCounter,
    fetch_failures: IntCounter,
    consecutive_failures: IntGauge,
    last_success_unix: IntGauge,
    signer_balance_lamports: IntGauge,
    last_success_unix_raw: Arc<AtomicI64>,
    consecutive_failures_raw: Arc<AtomicU64>,
}

impl PusherMetrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let pushes_succeeded = counter(
            "autara_pusher_pushes_succeeded_total",
            "Successful oracle price push transactions",
        );
        let pushes_failed = counter(
            "autara_pusher_pushes_failed_total",
            "Failed oracle price push attempts (tx send/timeout)",
        );
        let fetch_failures = counter(
            "autara_pusher_fetch_failures_total",
            "Price fetch failures after Pyth and DIA fallbacks",
        );
        let consecutive_failures = gauge(
            "autara_pusher_consecutive_failures",
            "Consecutive failed push or fetch cycles",
        );
        let last_success_unix = gauge(
            "autara_pusher_last_success_unixtime",
            "Unix timestamp of the last successful push",
        );
        let signer_balance_lamports = gauge(
            "autara_pusher_signer_balance_lamports",
            "Lamport balance of the pusher signer",
        );

        for metric in [
            Box::new(pushes_succeeded.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(pushes_failed.clone()),
            Box::new(fetch_failures.clone()),
            Box::new(consecutive_failures.clone()),
            Box::new(last_success_unix.clone()),
            Box::new(signer_balance_lamports.clone()),
        ] {
            registry.register(metric).expect("unique pusher metric");
        }

        Self {
            registry,
            pushes_succeeded,
            pushes_failed,
            fetch_failures,
            consecutive_failures,
            last_success_unix,
            signer_balance_lamports,
            last_success_unix_raw: Arc::new(AtomicI64::new(0)),
            consecutive_failures_raw: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn record_success(&self) {
        self.pushes_succeeded.inc();
        self.consecutive_failures_raw.store(0, Ordering::Release);
        self.consecutive_failures.set(0);
        let now = now_unix();
        self.last_success_unix_raw.store(now, Ordering::Release);
        self.last_success_unix.set(now);
    }

    pub fn record_push_failure(&self) {
        self.pushes_failed.inc();
        self.bump_consecutive_failures();
    }

    pub fn record_fetch_failure(&self) {
        self.fetch_failures.inc();
        self.bump_consecutive_failures();
    }

    pub fn set_signer_balance(&self, lamports: u64) {
        self.signer_balance_lamports.set(lamports as i64);
    }

    fn bump_consecutive_failures(&self) {
        let next = self.consecutive_failures_raw.fetch_add(1, Ordering::AcqRel) + 1;
        self.consecutive_failures.set(next as i64);
    }

    fn health_status(&self) -> StatusCode {
        let last = self.last_success_unix_raw.load(Ordering::Acquire);
        if last <= 0 {
            return StatusCode::SERVICE_UNAVAILABLE;
        }
        let age = now_unix().saturating_sub(last) as u64;
        if age <= HEALTH_MAX_STALE_SECS {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

pub async fn start_metrics_server(addr: SocketAddr, metrics: PusherMetrics) -> io::Result<()> {
    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics_handler))
        .with_state(metrics);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "Pusher observability server listening");
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            tracing::error!(%error, "Pusher observability server stopped");
        }
    });
    Ok(())
}

async fn health(State(metrics): State<PusherMetrics>) -> impl IntoResponse {
    (metrics.health_status(), "pusher")
}

async fn metrics_handler(State(metrics): State<PusherMetrics>) -> impl IntoResponse {
    let mut body = String::new();
    match TextEncoder::new().encode_utf8(&metrics.registry.gather(), &mut body) {
        Ok(()) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
            body,
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to encode metrics: {error}"),
        )
            .into_response(),
    }
}

fn counter(name: &str, help: &str) -> IntCounter {
    IntCounter::with_opts(Opts::new(name, help)).expect("valid metric definition")
}

fn gauge(name: &str, help: &str) -> IntGauge {
    IntGauge::with_opts(Opts::new(name, help)).expect("valid metric definition")
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_unavailable_until_success() {
        let metrics = PusherMetrics::new();
        assert_eq!(metrics.health_status(), StatusCode::SERVICE_UNAVAILABLE);
        metrics.record_success();
        assert_eq!(metrics.health_status(), StatusCode::OK);
    }

    #[test]
    fn consecutive_failures_reset_on_success() {
        let metrics = PusherMetrics::new();
        metrics.record_push_failure();
        metrics.record_fetch_failure();
        assert_eq!(metrics.consecutive_failures_raw.load(Ordering::Acquire), 2);
        metrics.record_success();
        assert_eq!(metrics.consecutive_failures_raw.load(Ordering::Acquire), 0);
    }
}
