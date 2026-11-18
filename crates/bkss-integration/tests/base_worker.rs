use bkss_core::{ResourceScope, SourcePublisher};
use bkss_integration::{
    Source, ThreadPublisher, ThreadPublisherGuard,
    base::{Cancellation, WorkerGuard, spawn_worker},
};
use gpui::{App, TestAppContext};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

struct WorkerSource {
    cancelled: Arc<AtomicBool>,
    publication_closed: Arc<AtomicBool>,
}

impl Source for WorkerSource {
    type Key = ();
    type Value = ();
    type Error = io::Error;
    type Guard = WorkerGuard;

    fn key(&self) {}

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (publisher, bridge): (ThreadPublisher<(), io::Error>, ThreadPublisherGuard) =
            publisher.thread_bridge(|()| (), |error| error, cx);
        let cancelled = Arc::clone(&self.cancelled);
        let publication_closed = Arc::clone(&self.publication_closed);
        spawn_worker(
            "base-worker-contract-test",
            bridge,
            publisher,
            move |publisher, token: Cancellation| {
                futures_lite::future::block_on(token.cancelled());
                cancelled.store(token.is_cancelled(), Ordering::Release);
                publication_closed.store(
                    matches!(
                        publisher.publish(()),
                        Err(bkss_core::ThreadPublishError::Closed(()))
                    ),
                    Ordering::Release,
                );
                Ok(())
            },
        )
    }
}

#[test]
fn public_worker_guard_cancels_worker_and_closes_ui_publication() {
    let cx = TestAppContext::single();
    let scope = cx.update(ResourceScope::new);
    let cancelled = Arc::new(AtomicBool::new(false));
    let publication_closed = Arc::new(AtomicBool::new(false));
    let source = bkss_core::SourceDescriptor::new(WorkerSource {
        cancelled: Arc::clone(&cancelled),
        publication_closed: Arc::clone(&publication_closed),
    });
    let lease = cx.update(|cx| scope.acquire(&source, cx).expect("activate worker source"));

    drop(lease);

    let deadline = Instant::now() + Duration::from_secs(2);
    while !cancelled.load(Ordering::Acquire) || !publication_closed.load(Ordering::Acquire) {
        assert!(
            Instant::now() < deadline,
            "worker did not observe guard drop"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert!(cancelled.load(Ordering::Acquire));
    assert!(publication_closed.load(Ordering::Acquire));
}
