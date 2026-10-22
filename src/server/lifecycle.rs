use std::{
    future::{Future, IntoFuture},
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use anyhow::{Context, Result};

use super::{AppState, router};
use crate::{collector::Collector, config::Config, store::Store};

const HTTP_SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(15);

pub async fn run(config: Config, store: Store, collector: Collector) -> Result<()> {
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, config.port)))
            .await
            .context("Bind local HTTP listener")?;
    let address = listener.local_addr()?;
    let mut config = config;
    config.port = address.port();
    let state = AppState::new(config, store, collector.clone())?;
    let shutdown = state.shutdown.clone();
    eprintln!(
        "Observer: http://{address}{}",
        if state.config.demo {
            " (synthetic demo; forwarding disabled)"
        } else {
            ""
        }
    );
    let server = axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown.clone().cancelled_owned())
        .into_future();
    tokio::pin!(server);
    let finished = tokio::select! {
        result = server.as_mut() => Some(result),
        _ = shutdown_signal() => None,
    };
    shutdown.cancel();
    arm_shutdown_watchdog();
    if let Some(result) = finished {
        collector.shutdown().await;
        return result.context("Run local HTTP server");
    }
    match drain_after_http_grace(server.as_mut(), collector.shutdown(), HTTP_SHUTDOWN_GRACE).await {
        Some(result) => result.context("Run local HTTP server"),
        None => anyhow::bail!(
            "HTTP shutdown grace expired; queued records were drained, but in-flight captures may be lost"
        ),
    }
}

/// This thread remains armed through runtime teardown. A clean process exit
/// ends it; an executor or blocking database task cannot disable the deadline.
fn arm_shutdown_watchdog() {
    if let Err(failure) = spawn_deadline_watchdog(SHUTDOWN_DEADLINE, || {
        eprintln!(
            "Observer shutdown exceeded 15 seconds; forcing exit. Pending captures may be lost."
        );
        std::process::exit(2);
    }) {
        eprintln!(
            "Cannot start the shutdown watchdog ({failure}); forcing exit. Pending captures may be lost."
        );
        std::process::exit(2);
    }
}

pub(super) fn spawn_deadline_watchdog(
    deadline: Duration,
    on_deadline: impl FnOnce() + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("observer-shutdown".into())
        .spawn(move || {
            std::thread::sleep(deadline);
            on_deadline();
        })
}

pub(super) async fn drain_after_http_grace<S, D>(
    server: S,
    drain: D,
    grace: Duration,
) -> Option<S::Output>
where
    S: Future,
    D: Future<Output = ()>,
{
    let finished = tokio::time::timeout(grace, server).await.ok();
    if finished.is_none() {
        eprintln!(
            "HTTP shutdown grace ({:.1}s) expired; draining queued records. In-flight captures may be lost.",
            grace.as_secs_f64()
        );
    }
    drain.await;
    finished
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = terminate.recv() => {},
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(windows)]
    {
        match tokio::signal::windows::ctrl_break() {
            Ok(mut signal) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = signal.recv() => {},
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
