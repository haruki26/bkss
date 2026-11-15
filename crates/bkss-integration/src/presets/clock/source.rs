use crate::{Source, SourcePublisher};
use gpui::App;
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
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
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let precision = self.precision;
        let worker_sender = sender.clone();
        let worker = thread::Builder::new()
            .name("bkss-clock".into())
            .spawn(move || {
                let mut last = None;
                while !worker_stopped.load(Ordering::Acquire) {
                    let now = chrono::Local::now();
                    let state = ClockState {
                        date: now.format("%Y-%m-%d").to_string(),
                        time: match precision {
                            ClockPrecision::Minute => now.format("%H:%M").to_string(),
                            ClockPrecision::Second => now.format("%H:%M:%S").to_string(),
                        },
                    };
                    if last.as_ref() != Some(&state) {
                        if worker_sender.publish(state.clone()).is_err() {
                            return;
                        }
                        last = Some(state);
                    }
                    let wait = next_boundary(precision, SystemTime::now());
                    thread::park_timeout(wait);
                }
            })?;
        drop(sender);
        Ok(ClockGuard {
            stopped,
            worker: Some(worker),
            _bridge: bridge,
        })
    }
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

pub struct ClockGuard {
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    _bridge: crate::ThreadPublisherGuard,
}

impl Drop for ClockGuard {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
        }
    }
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
