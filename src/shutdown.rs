//! Shutdown signalling shared by the server and daemon.
//!
//! A `watch` channel is enough: shutdown is a single monotonic event, and
//! every task can observe it without holding a lock.

use tokio::sync::watch;

pub type Shutdown = watch::Receiver<bool>;

pub fn channel() -> (watch::Sender<bool>, Shutdown) {
    watch::channel(false)
}

/// Install SIGINT/SIGTERM handling and return a receiver that flips to
/// `true` on the first signal.
pub fn install_signal_handler() -> Shutdown {
    let (tx, rx) = channel();

    tokio::spawn(async move {
        wait_for_signal().await;
        let _ = tx.send(true);
    });

    rx
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(err) => {
                tracing::error!("cannot install SIGTERM handler: {err}");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
