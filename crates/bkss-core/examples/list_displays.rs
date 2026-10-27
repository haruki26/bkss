use std::{
    cell::RefCell,
    error::Error,
    io,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::App;
use gpui_platform::application;

const READINESS_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, PartialEq, Eq)]
enum Readiness<T> {
    Waiting,
    Ready(Vec<T>),
    TimedOut,
    Finished,
}

struct DisplayReadiness {
    started: Instant,
    timeout: Duration,
    finished: bool,
}

impl DisplayReadiness {
    fn new(timeout: Duration, started: Instant) -> Self {
        Self {
            started,
            timeout,
            finished: false,
        }
    }

    fn poll<T>(&mut self, displays: Vec<T>, now: Instant) -> Readiness<T> {
        if self.finished {
            return Readiness::Finished;
        }

        if !displays.is_empty() {
            self.finished = true;
            return Readiness::Ready(displays);
        }

        if now.duration_since(self.started) >= self.timeout {
            self.finished = true;
            return Readiness::TimedOut;
        }

        Readiness::Waiting
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let result = Rc::new(RefCell::new(None));
    let app_result = result.clone();

    application().run(move |cx: &mut App| {
        let result = app_result.clone();
        let mut readiness = DisplayReadiness::new(READINESS_TIMEOUT, Instant::now());

        cx.spawn(async move |cx| {
            loop {
                let outcome = readiness.poll(cx.update(|cx| cx.displays()), Instant::now());
                match outcome {
                    Readiness::Waiting => {
                        cx.background_executor().timer(POLL_INTERVAL).await;
                    }
                    Readiness::Ready(displays) => {
                        let descriptions: Vec<String> = displays
                            .iter()
                            .map(|display| {
                                format!("{:?}: bounds {:?}", display.id(), display.bounds())
                            })
                            .collect();
                        *result.borrow_mut() = Some(Ok(descriptions));
                        cx.update(|cx| cx.quit());
                        break;
                    }
                    Readiness::TimedOut => {
                        *result.borrow_mut() = Some(Err(format!(
                            "timed out after {READINESS_TIMEOUT:?} waiting for GPUI displays"
                        )));
                        cx.update(|cx| cx.quit());
                        break;
                    }
                    Readiness::Finished => break,
                }
            }
        })
        .detach();
    });

    match result.borrow_mut().take() {
        Some(Ok(displays)) => {
            for display in displays {
                println!("{display}");
            }
            Ok(())
        }
        Some(Err(message)) => Err(io::Error::other(message).into()),
        None => Err(
            io::Error::other("GPUI application exited before display readiness completed").into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{DisplayReadiness, Readiness};
    use std::time::{Duration, Instant};

    #[test]
    fn waits_for_empty_then_returns_ready_once() {
        let start = Instant::now();
        let mut readiness = DisplayReadiness::new(Duration::from_secs(1), start);

        assert_eq!(readiness.poll::<u8>(vec![], start), Readiness::Waiting);
        assert_eq!(
            readiness.poll(vec![1, 2], start),
            Readiness::Ready(vec![1, 2])
        );
        assert_eq!(readiness.poll(vec![3], start), Readiness::Finished);
    }

    #[test]
    fn times_out_after_deadline_when_still_empty() {
        let start = Instant::now();
        let mut readiness = DisplayReadiness::new(Duration::from_secs(1), start);

        assert_eq!(
            readiness.poll::<u8>(vec![], start + Duration::from_secs(1)),
            Readiness::TimedOut
        );
        assert_eq!(readiness.poll::<u8>(vec![], start), Readiness::Finished);
    }
}
