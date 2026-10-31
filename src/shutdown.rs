//! Orderly stop: the signals that request it and a bounded wait for running requests.
use std::{future::Future, time::Duration};
use tokio_util::sync::CancellationToken;

/// How long running requests may finish after a stop request. A generation can last many
/// minutes; waiting for it would turn `docker stop` (SIGKILL after 10 s) into a kill that
/// skips cleanup and leaves the API socket behind.
pub const GRACE: Duration = Duration::from_secs(3);

/// Resolves on Ctrl-C, and on Unix also on SIGTERM (`docker stop`, systemd), so that
/// pairing reservations and API sockets are cleaned up on every orderly stop.
pub async fn signal() {
    #[cfg(unix)]
    if let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
        return;
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// Runs `server` until it ends. After `stop` is cancelled it gets `grace` to finish, then it
/// is dropped, which closes the remaining connections. `None` means the grace period ran out.
pub async fn with_grace<F: Future>(
    stop: &CancellationToken,
    grace: Duration,
    server: F,
) -> Option<F::Output> {
    tokio::pin!(server);
    tokio::select! {
        output = &mut server => Some(output),
        _ = async {
            stop.cancelled().await;
            tokio::time::sleep(grace).await;
        } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn finished_servers_are_not_cut_short_and_stuck_ones_are_after_the_grace_period() {
        let stop = CancellationToken::new();
        assert_eq!(
            with_grace(&stop, Duration::from_millis(50), async { 7 }).await,
            Some(7)
        );
        // Without a stop request a long-running server is simply left alone.
        assert!(
            tokio::time::timeout(
                Duration::from_millis(150),
                with_grace(
                    &stop,
                    Duration::from_millis(20),
                    std::future::pending::<()>()
                ),
            )
            .await
            .is_err()
        );
        stop.cancel();
        let started = Instant::now();
        let outcome = with_grace(
            &stop,
            Duration::from_millis(100),
            std::future::pending::<()>(),
        )
        .await;
        assert!(outcome.is_none());
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
