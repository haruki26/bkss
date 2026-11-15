use crate::{ThreadPublisher, ThreadPublisherGuard};
use std::{
    error::Error,
    io,
    thread::{self, JoinHandle},
};

/// Cloneable cancellation signal for an integration worker.
///
/// Dropping the worker guard closes every clone's receiver. Async setup and
/// backend calls should race their future against [`Cancellation::cancelled`].
#[derive(Clone)]
pub(crate) struct Cancellation {
    receiver: async_channel::Receiver<()>,
}

pub(crate) fn cancellation_channel() -> (async_channel::Sender<()>, Cancellation) {
    let (sender, receiver) = async_channel::bounded(1);
    (sender, Cancellation { receiver })
}

impl Cancellation {
    /// Returns whether shutdown has been requested.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.receiver.is_closed()
    }

    /// Completes when the worker guard requests shutdown.
    pub(crate) async fn cancelled(&self) {
        let _ = self.receiver.recv().await;
    }
}

/// Owns UI delivery and requests nonblocking worker cancellation on drop.
pub struct WorkerGuard {
    bridge: Option<ThreadPublisherGuard>,
    cancellation: Option<async_channel::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        drop(self.bridge.take());
        drop(self.cancellation.take());
        drop(self.worker.take());
    }
}

/// Starts a worker while retaining the UI bridge and cancellation lifetime.
///
/// The worker is detached on guard drop; cancellation does not join it or wait
/// for backend I/O. Async operations must be raced against the provided token
/// to prevent a blocked call from retaining backend resources indefinitely.
pub(crate) fn spawn_worker<P, E>(
    name: &'static str,
    bridge: ThreadPublisherGuard,
    publisher: ThreadPublisher<P, E>,
    run: impl FnOnce(ThreadPublisher<P, E>, Cancellation) -> Result<(), E> + Send + 'static,
) -> io::Result<WorkerGuard>
where
    P: Send + 'static,
    E: Error + Send + 'static,
{
    let (cancel_sender, cancellation) = cancellation_channel();
    let worker = thread::Builder::new().name(name.into()).spawn(move || {
        if let Err(error) = run(publisher.clone(), cancellation.clone())
            && !cancellation.is_cancelled()
        {
            let _ = publisher.fail(error);
        }
    })?;
    Ok(WorkerGuard {
        bridge: Some(bridge),
        cancellation: Some(cancel_sender),
        worker: Some(worker),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future::block_on;

    #[test]
    fn cancellation_is_broadcast_to_clones() {
        let (sender, cancellation) = cancellation_channel();
        let clone = cancellation.clone();
        assert!(!cancellation.is_cancelled());
        drop(sender);
        assert!(cancellation.is_cancelled());
        assert!(clone.is_cancelled());
        block_on(async {
            futures_lite::future::zip(cancellation.cancelled(), clone.cancelled()).await;
        });
    }
}
