use crate::base::worker::{self, Cancellation};
use crate::{Source, SourceDescriptor, SourcePublisher};
use gpui::App;
use std::{io, time::Duration};
use sysinfo::{Disks, System};

use super::{
    cpu, memory,
    state::{Usage, UsageKind},
    storage,
};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const CPU_INITIAL_INTERVAL: Duration = Duration::from_millis(200);

pub struct UsageSource {
    kind: UsageKind,
}

impl UsageSource {
    /// Declares a usage source with a one-second internal sampling interval.
    pub fn new(kind: UsageKind) -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self { kind })
    }
}

impl Source for UsageSource {
    type Key = UsageKind;
    type Value = Usage;
    type Error = io::Error;
    type Guard = worker::WorkerGuard;

    fn key(&self) -> Self::Key {
        self.kind
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (sender, bridge) = publisher.thread_bridge(|value| value, |error| error, cx);
        let kind = self.kind;
        worker::spawn_worker(
            "bkss-usage",
            bridge,
            sender,
            move |publisher, cancellation| {
                futures_lite::future::block_on(run(kind, publisher, cancellation))
            },
        )
    }
}

async fn run(
    kind: UsageKind,
    publisher: crate::ThreadPublisher<Usage, io::Error>,
    cancellation: Cancellation,
) -> Result<(), io::Error> {
    let mut system = System::new();
    let mut disks = Disks::new_with_refreshed_list();
    if cancellation.is_cancelled() {
        return Ok(());
    }
    if kind == UsageKind::Cpu {
        system.refresh_cpu_usage();
        if !wait_or_cancel(CPU_INITIAL_INTERVAL, &cancellation).await {
            return Ok(());
        }
    }
    loop {
        let usage = match kind {
            UsageKind::Cpu => cpu::sample(&mut system),
            UsageKind::Memory => memory::sample(&mut system),
            UsageKind::Storage => storage::sample(&mut disks),
        };
        if publisher.publish(usage).is_err()
            || !wait_or_cancel(SAMPLE_INTERVAL, &cancellation).await
        {
            return Ok(());
        }
    }
}

async fn wait_or_cancel(interval: Duration, cancellation: &Cancellation) -> bool {
    futures_lite::future::race(
        async {
            async_io::Timer::after(interval).await;
        },
        cancellation.cancelled(),
    )
    .await;
    !cancellation.is_cancelled()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_wait_observes_timer_and_cancellation() {
        futures_lite::future::block_on(async {
            let (sender, cancellation) = worker::cancellation_channel();
            assert!(wait_or_cancel(Duration::ZERO, &cancellation).await);
            let (elapsed, ()) = futures_lite::future::zip(
                wait_or_cancel(Duration::from_secs(60), &cancellation),
                async {
                    futures_lite::future::yield_now().await;
                    drop(sender);
                },
            )
            .await;
            assert!(!elapsed);
            assert!(!wait_or_cancel(Duration::ZERO, &cancellation).await);
        });
    }
}
