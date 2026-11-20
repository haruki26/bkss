use crate::base::worker;
use crate::{Source, SourcePublisher};
use gpui::App;
use std::{
    io,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::state::{ClockPrecision, ClockState};

pub struct Clock {
    precision: ClockPrecision,
}

impl Clock {
    /// Creates the source; pass it to `SourceDescriptor::new` to declare it.
    pub fn new(precision: ClockPrecision) -> Self {
        Self { precision }
    }
}

impl Source for Clock {
    type Key = ClockPrecision;
    type Value = ClockState;
    type Error = io::Error;
    type Guard = ClockGuard;

    fn key(&self) -> Self::Key {
        self.precision
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (sender, bridge) = publisher.thread_bridge(|state| state, |error| error, cx);
        let precision = self.precision;
        worker::spawn_worker(
            "bkss-clock",
            bridge,
            sender,
            move |publisher, cancellation| {
                futures_lite::future::block_on(run(precision, publisher, cancellation))
            },
        )
        .map(|worker| ClockGuard { _worker: worker })
    }
}

pub struct ClockGuard {
    _worker: worker::WorkerGuard,
}

async fn run(
    precision: ClockPrecision,
    publisher: crate::ThreadPublisher<ClockState, io::Error>,
    cancellation: worker::Cancellation,
) -> Result<(), io::Error> {
    let mut last = None;
    while !cancellation.is_cancelled() {
        let now = chrono::Local::now();
        let state = ClockState {
            date: now.format("%Y-%m-%d").to_string(),
            time: match precision {
                ClockPrecision::Minute => now.format("%H:%M").to_string(),
                ClockPrecision::Second => now.format("%H:%M:%S").to_string(),
            },
        };
        if last.as_ref() != Some(&state) {
            if publisher.publish(state.clone()).is_err() {
                return Ok(());
            }
            last = Some(state);
        }
        let wait = next_boundary(precision, SystemTime::now());
        futures_lite::future::race(
            async {
                async_io::Timer::after(wait).await;
            },
            cancellation.cancelled(),
        )
        .await;
    }
    Ok(())
}

fn next_boundary(precision: ClockPrecision, now: SystemTime) -> Duration {
    let quantum = match precision {
        ClockPrecision::Minute => 60,
        ClockPrecision::Second => 1,
    };
    let nanos = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let quantum_nanos = quantum * 1_000_000_000;
    let remainder = nanos % quantum_nanos;
    Duration::from_nanos((quantum_nanos - remainder) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceDescriptor;

    #[test]
    fn boundary_wait_is_aligned_and_strictly_positive() {
        let _descriptor = SourceDescriptor::new(Clock::new(ClockPrecision::Minute));
        assert_eq!(
            next_boundary(
                ClockPrecision::Second,
                UNIX_EPOCH + Duration::from_millis(250)
            ),
            Duration::from_millis(750)
        );
        assert_eq!(
            next_boundary(
                ClockPrecision::Minute,
                UNIX_EPOCH + Duration::from_secs(60 * 7 + 12)
            ),
            Duration::from_secs(48)
        );
        assert_eq!(
            next_boundary(ClockPrecision::Second, UNIX_EPOCH),
            Duration::from_secs(1)
        );
    }
}
