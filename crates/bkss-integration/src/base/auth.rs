use async_channel::{Receiver, Sender, TrySendError};
use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

/// A single-completion answer capability for a pending authentication request.
pub struct Responder<A> {
    shared: Arc<Shared<A>>,
}

impl<A> Clone for Responder<A> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<A> Responder<A> {
    /// Completes the request once, unless it was cancelled or has expired.
    pub fn respond(&self, answer: A) -> Result<(), RespondError> {
        let mut state = self.shared.lock();
        if matches!(state.completion, Completion::Pending(_))
            && Instant::now() >= self.shared.expires_at
        {
            state.expire();
            return Err(RespondError::Expired);
        }
        match &mut state.completion {
            Completion::Pending(sender) => match sender.try_send(answer) {
                Ok(()) => {
                    state.completion = Completion::Completed;
                    Ok(())
                }
                Err(TrySendError::Closed(_)) => {
                    state.completion = Completion::Cancelled;
                    Err(RespondError::Cancelled)
                }
                Err(TrySendError::Full(_)) => {
                    state.completion = Completion::Completed;
                    Err(RespondError::AlreadyCompleted)
                }
            },
            Completion::Completed => Err(RespondError::AlreadyCompleted),
            Completion::Cancelled => Err(RespondError::Cancelled),
            Completion::Expired => Err(RespondError::Expired),
        }
    }

    /// Invalidates a pending request, including while its receiver is awaiting.
    pub(crate) fn invalidate(&self) {
        self.shared.lock().invalidate();
    }
}

/// Failure to complete a pending authentication request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RespondError {
    /// Another clone already completed the request.
    AlreadyCompleted,
    /// The request was cancelled by its owner.
    Cancelled,
    /// The finite response deadline elapsed.
    Expired,
    /// The requested duration could not be represented by the system clock.
    InvalidExpiry,
}

impl fmt::Display for RespondError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AlreadyCompleted => "authentication request already completed",
            Self::Cancelled => "authentication request was cancelled",
            Self::Expired => "authentication request expired",
            Self::InvalidExpiry => "authentication expiry is out of range",
        })
    }
}

impl Error for RespondError {}

/// Move-only value intended for secrets; its debug representation is redacted.
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &T {
        &self.0
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

struct Shared<A> {
    expires_at: Instant,
    state: Mutex<State<A>>,
}

impl<A> Shared<A> {
    fn lock(&self) -> MutexGuard<'_, State<A>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct State<A> {
    completion: Completion<A>,
}

impl<A> State<A> {
    fn expire(&mut self) {
        if matches!(self.completion, Completion::Pending(_)) {
            self.completion = Completion::Expired;
        }
    }

    fn invalidate(&mut self) {
        if matches!(self.completion, Completion::Pending(_)) {
            self.completion = Completion::Cancelled;
        }
    }
}

enum Completion<A> {
    Pending(Sender<A>),
    Completed,
    Cancelled,
    Expired,
}

/// Internal receiver capability paired with the public responder.
pub(crate) struct Pending<A> {
    shared: Arc<Shared<A>>,
    receiver: Receiver<A>,
}

impl<A> Pending<A> {
    pub(crate) async fn receive(self) -> Result<A, RespondError> {
        let deadline = self.shared.expires_at;
        let receive = async { self.receiver.recv().await.ok() };
        let timeout = async {
            async_io::Timer::at(deadline).await;
            None
        };
        match futures_lite::future::race(receive, timeout).await {
            Some(answer) => Ok(answer),
            None => self.expire_or_receive(),
        }
    }

    fn terminal_error(&self) -> Result<A, RespondError> {
        Err(match self.shared.lock().completion {
            Completion::Completed => RespondError::AlreadyCompleted,
            Completion::Cancelled => RespondError::Cancelled,
            Completion::Expired => RespondError::Expired,
            Completion::Pending(_) => RespondError::Cancelled,
        })
    }

    fn expire_or_receive(&self) -> Result<A, RespondError> {
        let mut state = self.shared.lock();
        match state.completion {
            Completion::Completed => drop(state),
            Completion::Cancelled => return Err(RespondError::Cancelled),
            Completion::Expired => return Err(RespondError::Expired),
            Completion::Pending(_) => {
                state.expire();
                return Err(RespondError::Expired);
            }
        }
        match self.receiver.try_recv() {
            Ok(answer) => Ok(answer),
            Err(_) => self.terminal_error(),
        }
    }
}

impl<A> Drop for Pending<A> {
    fn drop(&mut self) {
        self.shared.lock().invalidate();
    }
}

/// Creates an answer capability with a finite response deadline.
pub(crate) fn request<A>(timeout: Duration) -> Result<(Responder<A>, Pending<A>), RespondError> {
    let expires_at = Instant::now()
        .checked_add(timeout)
        .ok_or(RespondError::InvalidExpiry)?;
    let (sender, receiver) = async_channel::bounded(1);
    let shared = Arc::new(Shared {
        expires_at,
        state: Mutex::new(State {
            completion: Completion::Pending(sender),
        }),
    });
    Ok((
        Responder {
            shared: Arc::clone(&shared),
        },
        Pending { shared, receiver },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future::block_on;

    #[test]
    fn one_cloned_responder_wins_and_followup_is_rejected() {
        let (responder, pending) = request(Duration::from_secs(1)).unwrap();
        let clone = responder.clone();
        let after_receive = responder.clone();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let first_barrier = Arc::clone(&barrier);
        let first = std::thread::spawn(move || {
            first_barrier.wait();
            responder.respond(7)
        });
        let second_barrier = Arc::clone(&barrier);
        let second = std::thread::spawn(move || {
            second_barrier.wait();
            clone.respond(8)
        });
        barrier.wait();
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err(RespondError::AlreadyCompleted))
                .count(),
            1
        );
        let answer = block_on(pending.receive()).unwrap();
        assert!(answer == 7 || answer == 8);
        assert_eq!(
            after_receive.respond(9),
            Err(RespondError::AlreadyCompleted)
        );
    }

    #[test]
    fn pending_drop_cancels_and_deadline_expires_without_a_response() {
        let (responder, pending) = request(Duration::from_secs(1)).unwrap();
        drop(pending);
        assert_eq!(responder.respond(7), Err(RespondError::Cancelled));

        let (responder, pending) = request(Duration::ZERO).unwrap();
        assert_eq!(block_on(pending.receive()), Err(RespondError::Expired));
        assert_eq!(responder.respond(7), Err(RespondError::Expired));
    }

    #[test]
    fn responder_cancels_a_pending_receive() {
        let (responder, pending) = request::<i32>(Duration::from_secs(1)).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let worker_barrier = Arc::clone(&barrier);
        let receive = std::thread::spawn(move || {
            worker_barrier.wait();
            block_on(pending.receive())
        });
        barrier.wait();
        responder.invalidate();
        assert_eq!(receive.join().unwrap(), Err(RespondError::Cancelled));
        assert_eq!(responder.respond(3), Err(RespondError::Cancelled));
    }

    #[test]
    fn secret_debug_is_redacted() {
        assert_eq!(
            format!("{:?}", Secret::new("private")),
            "Secret([REDACTED])"
        );
    }
}
