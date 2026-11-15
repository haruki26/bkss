use std::{error::Error, fmt, future::Future, num::NonZeroUsize, pin::Pin};

use super::worker::Cancellation;

/// Failure to admit a command to its bounded worker queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchError {
    /// The command worker has stopped.
    Inactive,
    /// The bounded command queue is full.
    Busy,
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inactive => f.write_str("command worker is inactive"),
            Self::Busy => f.write_str("command queue is busy"),
        }
    }
}

impl Error for DispatchError {}

/// Result of an admitted command.
#[derive(Debug)]
pub enum OperationError<E> {
    /// The worker stopped before producing a result.
    Inactive,
    /// The backend rejected or failed the operation.
    Backend(E),
}

impl<E: fmt::Display> fmt::Display for OperationError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inactive => f.write_str("command worker is inactive"),
            Self::Backend(error) => error.fmt(f),
        }
    }
}

impl<E: Error + 'static> Error for OperationError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Inactive => None,
            Self::Backend(error) => Some(error),
        }
    }
}

/// Awaitable result for an admitted backend operation.
pub type Operation<T, E> =
    Pin<Box<dyn Future<Output = Result<T, OperationError<E>>> + Send + 'static>>;

/// Cloneable command endpoint. Clones do not retain the source activation.
pub struct CommandHandle<R, T, E> {
    sender: async_channel::Sender<CommandRequest<R, T, E>>,
    cancellation: Cancellation,
}

impl<R, T, E> Clone for CommandHandle<R, T, E> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            cancellation: self.cancellation.clone(),
        }
    }
}

impl<R, T, E> CommandHandle<R, T, E>
where
    R: Send + 'static,
    T: Send + 'static,
    E: Error + Send + 'static,
{
    /// Attempts bounded admission immediately and returns the operation result.
    pub fn dispatch(&self, request: R) -> Result<Operation<T, E>, DispatchError> {
        if self.cancellation.is_cancelled() {
            return Err(DispatchError::Inactive);
        }
        let (reply_sender, reply_receiver) = async_channel::bounded(1);
        let request = CommandRequest {
            value: request,
            reply: reply_sender,
        };
        self.sender.try_send(request).map_err(|error| match error {
            async_channel::TrySendError::Full(_) if self.cancellation.is_cancelled() => {
                DispatchError::Inactive
            }
            async_channel::TrySendError::Full(_) => DispatchError::Busy,
            async_channel::TrySendError::Closed(_) => DispatchError::Inactive,
        })?;
        if self.cancellation.is_cancelled() {
            return Err(DispatchError::Inactive);
        }
        let cancellation = self.cancellation.clone();
        Ok(Box::pin(async move {
            let reply = async { reply_receiver.recv().await.ok() };
            let cancelled = async {
                cancellation.cancelled().await;
                None
            };
            match futures_lite::future::race(reply, cancelled).await {
                Some(result) if !cancellation.is_cancelled() => result,
                _ => Err(OperationError::Inactive),
            }
        }))
    }
}

/// One admitted request and its single-use reply path.
pub(crate) struct CommandRequest<R, T, E> {
    value: R,
    reply: async_channel::Sender<Result<T, OperationError<E>>>,
}

impl<R, T, E> CommandRequest<R, T, E> {
    pub(crate) fn into_parts(self) -> (R, impl FnOnce(Result<T, E>) + Send + 'static)
    where
        R: Send + 'static,
        T: Send + 'static,
        E: Send + 'static,
    {
        let Self { value, reply } = self;
        (value, move |result| {
            let result = result.map_err(OperationError::Backend);
            let _ = reply.try_send(result);
        })
    }
}

/// The worker-side end of a bounded command channel.
pub(crate) struct CommandReceiver<R, T, E> {
    receiver: async_channel::Receiver<CommandRequest<R, T, E>>,
}

impl<R, T, E> CommandReceiver<R, T, E> {
    pub(crate) async fn recv(&self) -> Option<CommandRequest<R, T, E>> {
        self.receiver.recv().await.ok()
    }
}

/// Creates an independent command endpoint and worker receiver.
///
/// Create it inside the worker with its cancellation token before publishing
/// the handle, so dispatch and operation futures observe guard shutdown.
pub(crate) fn channel<R, T, E>(
    capacity: NonZeroUsize,
    cancellation: Cancellation,
) -> (CommandHandle<R, T, E>, CommandReceiver<R, T, E>) {
    let (sender, receiver) = async_channel::bounded(capacity.get());
    (
        CommandHandle {
            sender,
            cancellation,
        },
        CommandReceiver { receiver },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future::block_on;

    fn cancellation() -> (Cancellation, async_channel::Sender<()>) {
        let (sender, token) = super::super::worker::cancellation_channel();
        (token, sender)
    }

    #[test]
    fn admission_is_bounded_and_stale_handles_are_inactive() {
        let (token, cancel_sender) = cancellation();
        let (commands, receiver) = channel::<u8, (), std::io::Error>(NonZeroUsize::MIN, token);
        let operation = commands.dispatch(1).unwrap();
        assert_eq!(commands.dispatch(2).err(), Some(DispatchError::Busy));
        let _request = block_on(receiver.recv()).unwrap();
        drop(cancel_sender);
        assert_eq!(commands.dispatch(3).err(), Some(DispatchError::Inactive));
        assert!(matches!(block_on(operation), Err(OperationError::Inactive)));
        drop(receiver);
        assert_eq!(commands.dispatch(4).err(), Some(DispatchError::Inactive));
    }

    #[test]
    fn backend_error_reaches_operation_and_dropped_operation_is_nonblocking() {
        let (token, _cancel_sender) = cancellation();
        let (commands, receiver) = channel::<u8, (), std::io::Error>(NonZeroUsize::MIN, token);
        let dropped = commands.dispatch(1).unwrap();
        drop(dropped);
        let request = block_on(receiver.recv()).unwrap();
        let (value, reply) = request.into_parts();
        assert_eq!(value, 1);
        reply(Err(std::io::Error::other("discarded result")));

        let operation = commands.dispatch(2).unwrap();
        let request = block_on(receiver.recv()).unwrap();
        let (value, reply) = request.into_parts();
        assert_eq!(value, 2);
        reply(Err(std::io::Error::other("backend failed")));
        let error = block_on(operation).unwrap_err();
        assert!(matches!(error, OperationError::Backend(_)));
        assert_eq!(error.source().unwrap().to_string(), "backend failed");
    }
}
