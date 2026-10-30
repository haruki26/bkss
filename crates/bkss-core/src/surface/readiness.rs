use std::time::{Duration, Instant};

pub(crate) struct DisplayReadiness {
    started: Instant,
    timeout: Duration,
    finished: bool,
}

pub(crate) enum Readiness {
    Waiting,
    Ready,
    TimedOut,
    Finished,
}

impl DisplayReadiness {
    pub(crate) fn new(timeout: Duration, started: Instant) -> Self {
        Self {
            started,
            timeout,
            finished: false,
        }
    }

    pub(crate) fn poll(&mut self, found: bool, now: Instant) -> Readiness {
        if self.finished {
            return Readiness::Finished;
        }
        if now.duration_since(self.started) >= self.timeout {
            self.finished = true;
            return Readiness::TimedOut;
        }
        if found {
            self.finished = true;
            return Readiness::Ready;
        }
        Readiness::Waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn readiness_deadline_precedes_presence_and_terminal_states_do_not_repeat() {
        let start = Instant::now();
        let timeout = Duration::from_millis(10);

        let mut before_deadline = DisplayReadiness::new(timeout, start);
        assert!(matches!(
            before_deadline.poll(false, start),
            Readiness::Waiting
        ));
        assert!(matches!(
            before_deadline.poll(true, start + timeout - Duration::from_nanos(1)),
            Readiness::Ready
        ));
        assert!(matches!(
            before_deadline.poll(true, start + timeout),
            Readiness::Finished
        ));

        let mut exact_deadline = DisplayReadiness::new(timeout, start);
        assert!(matches!(
            exact_deadline.poll(true, start + timeout),
            Readiness::TimedOut
        ));
        assert!(matches!(
            exact_deadline.poll(true, start),
            Readiness::Finished
        ));

        let mut delayed_presence = DisplayReadiness::new(timeout, start);
        assert!(matches!(
            delayed_presence.poll(false, start),
            Readiness::Waiting
        ));
        assert!(matches!(
            delayed_presence.poll(true, start + timeout + Duration::from_millis(40)),
            Readiness::TimedOut
        ));
        assert!(matches!(
            delayed_presence.poll(false, start),
            Readiness::Finished
        ));

        let mut zero_timeout = DisplayReadiness::new(Duration::ZERO, start);
        assert!(matches!(
            zero_timeout.poll(true, start),
            Readiness::TimedOut
        ));
        assert!(matches!(
            zero_timeout.poll(true, start),
            Readiness::Finished
        ));
    }
}
