use super::{
    backend::{self, RawDevice, Snapshot},
    error::AudioError,
    state::AudioState,
};
use crate::{
    Source, SourceDescriptor, SourcePublisher,
    base::{
        Operation, WorkerGuard,
        command::{self, CommandHandle},
        worker::{self, Cancellation},
    },
};
use gpui::App;
use libpulse_binding::{
    callbacks::ListResult,
    context::{
        self, Context,
        introspect::{Introspector, SinkInfo, SourceInfo},
        subscribe::InterestMaskSet,
    },
    mainloop::standard::Mainloop,
    operation::State as OperationState,
    volume::{ChannelVolumes, Volume},
};
use std::{
    cell::{Cell, RefCell},
    num::NonZeroUsize,
    rc::Rc,
    time::Duration,
};

#[derive(Clone, Copy)]
enum DeviceKind {
    Sink,
    Source,
}

enum Request {
    Volume(DeviceKind, f32),
    Mute(DeviceKind, bool),
}

#[derive(Clone)]
pub struct AudioControl {
    commands: CommandHandle<Request, (), AudioError>,
}

impl AudioControl {
    pub fn set_output_volume(
        &self,
        volume: f32,
    ) -> Result<Operation<(), AudioError>, AudioControlError> {
        let request = volume_request(DeviceKind::Sink, volume)?;
        self.commands
            .dispatch(request)
            .map_err(AudioControlError::Dispatch)
    }

    pub fn set_input_volume(
        &self,
        volume: f32,
    ) -> Result<Operation<(), AudioError>, AudioControlError> {
        let request = volume_request(DeviceKind::Source, volume)?;
        self.commands
            .dispatch(request)
            .map_err(AudioControlError::Dispatch)
    }

    pub fn set_output_muted(
        &self,
        muted: bool,
    ) -> Result<Operation<(), AudioError>, AudioControlError> {
        self.commands
            .dispatch(Request::Mute(DeviceKind::Sink, muted))
            .map_err(AudioControlError::Dispatch)
    }

    pub fn set_input_muted(
        &self,
        muted: bool,
    ) -> Result<Operation<(), AudioError>, AudioControlError> {
        self.commands
            .dispatch(Request::Mute(DeviceKind::Source, muted))
            .map_err(AudioControlError::Dispatch)
    }
}

#[derive(Debug)]
pub enum AudioControlError {
    InvalidVolume,
    Dispatch(crate::DispatchError),
}

impl std::fmt::Display for AudioControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidVolume => f.write_str("audio volume must be finite and between 0 and 1"),
            Self::Dispatch(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for AudioControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidVolume => None,
            Self::Dispatch(error) => Some(error),
        }
    }
}

fn volume_request(kind: DeviceKind, volume: f32) -> Result<Request, AudioControlError> {
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return Err(AudioControlError::InvalidVolume);
    }
    Ok(Request::Volume(kind, volume))
}

pub struct Audio;

impl Audio {
    pub fn new() -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self)
    }
}

impl Source for Audio {
    type Key = ();
    type Value = AudioState;
    type Error = AudioError;
    type Guard = WorkerGuard;

    fn key(&self) {}

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (sender, bridge) = publisher.thread_bridge(|state| state, |error| error, cx);
        worker::spawn_worker("bkss-audio", bridge, sender, move |sender, cancellation| {
            run_audio(sender, cancellation)
        })
        .map_err(AudioError::Worker)
    }
}

fn run_audio(
    publisher: crate::ThreadPublisher<AudioState, AudioError>,
    cancellation: Cancellation,
) -> Result<(), AudioError> {
    let (commands, receiver) = command::channel(
        NonZeroUsize::new(16).unwrap_or(NonZeroUsize::MIN),
        cancellation.clone(),
    );
    let control = AudioControl { commands };
    let mut mainloop = Mainloop::new()
        .ok_or_else(|| AudioError::Connection("could not create PulseAudio mainloop".into()))?;
    let mut context = Context::new(&mainloop, "BKSS audio integration")
        .ok_or_else(|| AudioError::Connection("could not create PulseAudio context".into()))?;
    context
        .connect(None, context::FlagSet::NOAUTOSPAWN, None)
        .map_err(|cause| AudioError::Pulse {
            operation: "start PulseAudio connection",
            cause,
        })?;
    while context.get_state() != context::State::Ready {
        if cancellation.is_cancelled() {
            return Ok(());
        }
        if matches!(
            context.get_state(),
            context::State::Failed | context::State::Terminated
        ) {
            return Err(AudioError::Pulse {
                operation: "connect to PulseAudio",
                cause: context.errno(),
            });
        }
        iterate(&mut mainloop)?;
        wait_tick(&receiver, &cancellation);
    }

    let dirty = Rc::new(Cell::new(true));
    let dirty_callback = Rc::clone(&dirty);
    context.set_subscribe_callback(Some(Box::new(move |_, _, _| dirty_callback.set(true))));
    let mask = InterestMaskSet::SERVER | InterestMaskSet::SINK | InterestMaskSet::SOURCE;
    let subscription_result = Rc::new(Cell::new(None));
    let subscription_callback = Rc::clone(&subscription_result);
    let subscription = context.subscribe(mask, move |success| {
        subscription_callback.set(Some(success));
    });
    while subscription.get_state() == OperationState::Running {
        if cancellation.is_cancelled() {
            return Ok(());
        }
        iterate(&mut mainloop)?;
        wait_tick(&receiver, &cancellation);
    }
    if subscription.get_state() == OperationState::Done && subscription_result.get() == Some(true) {
        dirty.set(true);
    } else {
        return Err(AudioError::Pulse {
            operation: "subscribe to PulseAudio changes",
            cause: context.errno(),
        });
    }

    let mut current = Snapshot::default();
    let mut published: Option<AudioState> = None;
    loop {
        if cancellation.is_cancelled() {
            return Ok(());
        }
        if dirty.replace(false) {
            current = query_snapshot(&mut mainloop, &context, &cancellation)?;
            let state = AudioState {
                default_sink: current.default_sink.clone(),
                default_source: current.default_source.clone(),
                available_sinks: current.sinks.clone(),
                available_sources: current.sources.clone(),
                control: control.clone(),
            };
            let changed = published.as_ref().is_none_or(|previous| {
                previous.default_sink != state.default_sink
                    || previous.default_source != state.default_source
                    || previous.available_sinks != state.available_sinks
                    || previous.available_sources != state.available_sources
            });
            if changed {
                if publisher.publish(state.clone()).is_err() {
                    return Ok(());
                }
                published = Some(state);
            }
        }
        match wait_tick(&receiver, &cancellation) {
            Some(request) => {
                if cancellation.is_cancelled() {
                    return Ok(());
                }
                let (request, reply) = request.into_parts();
                let result =
                    apply_request(&mut mainloop, &context, &current, request, &cancellation);
                if cancellation.is_cancelled() {
                    return Ok(());
                }
                reply(result);
            }
            None if cancellation.is_cancelled() => return Ok(()),
            None => {}
        }
        iterate(&mut mainloop)?;
        if matches!(
            context.get_state(),
            context::State::Failed | context::State::Terminated
        ) {
            return Err(AudioError::Pulse {
                operation: "query PulseAudio endpoints",
                cause: context.errno(),
            });
        }
    }
}

fn iterate(mainloop: &mut Mainloop) -> Result<(), AudioError> {
    match mainloop.iterate(false) {
        libpulse_binding::mainloop::standard::IterateResult::Success(_) => Ok(()),
        libpulse_binding::mainloop::standard::IterateResult::Quit(_) => Err(
            AudioError::Connection("PulseAudio mainloop quit unexpectedly".into()),
        ),
        libpulse_binding::mainloop::standard::IterateResult::Err(cause) => Err(AudioError::Pulse {
            operation: "iterate PulseAudio mainloop",
            cause,
        }),
    }
}

fn wait_tick(
    commands: &command::CommandReceiver<Request, (), AudioError>,
    cancellation: &Cancellation,
) -> Option<command::CommandRequest<Request, (), AudioError>> {
    // Pulse callbacks are driven separately; this timeout bounds command/cancel latency only.
    let cancelled = async {
        cancellation.cancelled().await;
        None
    };
    let timeout = async {
        let _ = async_io::Timer::after(Duration::from_millis(30)).await;
        None
    };
    futures_lite::future::block_on(futures_lite::future::race(
        futures_lite::future::race(commands.recv(), cancelled),
        timeout,
    ))
}

fn query_snapshot(
    mainloop: &mut Mainloop,
    context: &Context,
    cancellation: &Cancellation,
) -> Result<Snapshot, AudioError> {
    struct Parts {
        server_done: bool,
        sinks_done: bool,
        sources_done: bool,
        failed: bool,
        default_sink: Option<String>,
        default_source: Option<String>,
        sinks: Vec<RawDevice>,
        sources: Vec<RawDevice>,
    }
    let parts = Rc::new(RefCell::new(Parts {
        server_done: false,
        sinks_done: false,
        sources_done: false,
        failed: false,
        default_sink: None,
        default_source: None,
        sinks: Vec::new(),
        sources: Vec::new(),
    }));
    let server_parts = Rc::clone(&parts);
    let sinks_parts = Rc::clone(&parts);
    let sources_parts = Rc::clone(&parts);
    let introspector = context.introspect();
    let _server_op = introspector.get_server_info(move |info| {
        let mut parts = server_parts.borrow_mut();
        parts.default_sink = info.default_sink_name.as_deref().map(str::to_owned);
        parts.default_source = info.default_source_name.as_deref().map(str::to_owned);
        parts.server_done = true;
    });
    let _sinks_op = introspector.get_sink_info_list(move |result| match result {
        ListResult::Item(info) => sinks_parts.borrow_mut().sinks.push(sink_tuple(info)),
        ListResult::End => sinks_parts.borrow_mut().sinks_done = true,
        ListResult::Error => {
            let mut parts = sinks_parts.borrow_mut();
            parts.sinks_done = true;
            parts.failed = true;
        }
    });
    let _sources_op = introspector.get_source_info_list(move |result| match result {
        ListResult::Item(info) => sources_parts.borrow_mut().sources.push(source_tuple(info)),
        ListResult::End => sources_parts.borrow_mut().sources_done = true,
        ListResult::Error => {
            let mut parts = sources_parts.borrow_mut();
            parts.sources_done = true;
            parts.failed = true;
        }
    });
    loop {
        if cancellation.is_cancelled() {
            return Err(AudioError::Connection(
                "audio source cancelled during query".into(),
            ));
        }
        if matches!(
            context.get_state(),
            context::State::Failed | context::State::Terminated
        ) {
            return Err(AudioError::Pulse {
                operation: "connect to PulseAudio",
                cause: context.errno(),
            });
        }
        let done = {
            let parts = parts.borrow();
            parts.server_done && parts.sinks_done && parts.sources_done
        };
        let snapshot_parts = parts.borrow();
        let failed = (!snapshot_parts.server_done
            && _server_op.get_state() != OperationState::Running)
            || (!snapshot_parts.sinks_done && _sinks_op.get_state() != OperationState::Running)
            || (!snapshot_parts.sources_done && _sources_op.get_state() != OperationState::Running);
        drop(snapshot_parts);
        if failed {
            return Err(AudioError::Pulse {
                operation: "query PulseAudio endpoints",
                cause: context.errno(),
            });
        }
        if done {
            let parts = parts.borrow();
            if parts.failed {
                return Err(AudioError::Pulse {
                    operation: "query PulseAudio endpoints",
                    cause: context.errno(),
                });
            }
            return Ok(backend::snapshot(
                parts.default_sink.clone(),
                parts.default_source.clone(),
                parts.sinks.clone(),
                parts.sources.clone(),
            ));
        }
        iterate(mainloop)?;
        wait_query_tick(cancellation);
    }
}

fn sink_tuple(info: &SinkInfo<'_>) -> RawDevice {
    RawDevice {
        name: info.name.as_deref().unwrap_or_default().to_owned(),
        description: info.description.as_deref().unwrap_or_default().to_owned(),
        muted: info.mute,
        volume: info.volume.get().iter().map(|v| v.0).collect(),
    }
}

fn source_tuple(info: &SourceInfo<'_>) -> RawDevice {
    RawDevice {
        name: info.name.as_deref().unwrap_or_default().to_owned(),
        description: info.description.as_deref().unwrap_or_default().to_owned(),
        muted: info.mute,
        volume: info.volume.get().iter().map(|v| v.0).collect(),
    }
}

fn wait_query_tick(cancellation: &Cancellation) {
    futures_lite::future::block_on(futures_lite::future::race(
        async {
            cancellation.cancelled().await;
        },
        async {
            let _ = async_io::Timer::after(Duration::from_millis(10)).await;
        },
    ));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandProgress {
    Pending,
    Completed(bool),
    Failed,
}

fn command_progress(
    callback_done: bool,
    callback_success: bool,
    operation_state: OperationState,
    context_state: context::State,
) -> CommandProgress {
    if callback_done {
        CommandProgress::Completed(callback_success)
    } else if matches!(
        context_state,
        context::State::Failed | context::State::Terminated
    ) || operation_state != OperationState::Running
    {
        CommandProgress::Failed
    } else {
        CommandProgress::Pending
    }
}

fn apply_request(
    mainloop: &mut Mainloop,
    context: &Context,
    snapshot: &Snapshot,
    request: Request,
    cancellation: &Cancellation,
) -> Result<(), AudioError> {
    let (kind, volume, mute) = match request {
        Request::Volume(kind, value) => (kind, Some(value), None),
        Request::Mute(kind, value) => (kind, None, Some(value)),
    };
    let done = Rc::new(Cell::new(false));
    let success = Rc::new(Cell::new(false));
    let done_callback = Rc::clone(&done);
    let success_callback = Rc::clone(&success);
    let operation = match kind {
        DeviceKind::Sink => {
            let sink = snapshot
                .default_sink
                .as_ref()
                .ok_or_else(|| AudioError::Backend {
                    operation: "change default audio device",
                    cause: "no default output is available".into(),
                })?;
            let intro: Introspector = context.introspect();
            if let Some(volume) = volume {
                let mut channels = ChannelVolumes::default();
                let channels_count =
                    sink.volume
                        .len()
                        .clamp(1, ChannelVolumes::CHANNELS_MAX as usize) as u8;
                channels.set(
                    channels_count,
                    Volume((volume * Volume::NORMAL.0 as f32) as u32),
                );
                let mut intro = intro;
                intro.set_sink_volume_by_name(
                    &sink.name,
                    &channels,
                    Some(Box::new(move |ok| {
                        success_callback.set(ok);
                        done_callback.set(true);
                    })),
                )
            } else {
                let mut intro = intro;
                intro.set_sink_mute_by_name(
                    &sink.name,
                    mute.unwrap_or(false),
                    Some(Box::new(move |ok| {
                        success_callback.set(ok);
                        done_callback.set(true);
                    })),
                )
            }
        }
        DeviceKind::Source => {
            let source = snapshot
                .default_source
                .as_ref()
                .ok_or_else(|| AudioError::Backend {
                    operation: "change default audio device",
                    cause: "no default input is available".into(),
                })?;
            let intro: Introspector = context.introspect();
            if let Some(volume) = volume {
                let mut channels = ChannelVolumes::default();
                let channels_count = source
                    .volume
                    .len()
                    .clamp(1, ChannelVolumes::CHANNELS_MAX as usize)
                    as u8;
                channels.set(
                    channels_count,
                    Volume((volume * Volume::NORMAL.0 as f32) as u32),
                );
                let mut intro = intro;
                intro.set_source_volume_by_name(
                    &source.name,
                    &channels,
                    Some(Box::new(move |ok| {
                        success_callback.set(ok);
                        done_callback.set(true);
                    })),
                )
            } else {
                let mut intro = intro;
                intro.set_source_mute_by_name(
                    &source.name,
                    mute.unwrap_or(false),
                    Some(Box::new(move |ok| {
                        success_callback.set(ok);
                        done_callback.set(true);
                    })),
                )
            }
        }
    };
    loop {
        if cancellation.is_cancelled() {
            return Err(AudioError::Connection(
                "audio source cancelled during command".into(),
            ));
        }
        match command_progress(
            done.get(),
            success.get(),
            operation.get_state(),
            context.get_state(),
        ) {
            CommandProgress::Completed(true) => return Ok(()),
            CommandProgress::Completed(false) => {
                return Err(AudioError::Pulse {
                    operation: "change PulseAudio device",
                    cause: context.errno(),
                });
            }
            CommandProgress::Failed => {
                return Err(AudioError::Pulse {
                    operation: "complete PulseAudio device change",
                    cause: context.errno(),
                });
            }
            CommandProgress::Pending => {
                iterate(mainloop)?;
                wait_query_tick(cancellation);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future::block_on;

    #[test]
    fn volume_validation_precedes_command_admission() {
        assert!(matches!(
            volume_request(DeviceKind::Sink, f32::NAN),
            Err(AudioControlError::InvalidVolume)
        ));
        assert!(matches!(
            volume_request(DeviceKind::Source, -0.01),
            Err(AudioControlError::InvalidVolume)
        ));
        assert!(volume_request(DeviceKind::Sink, 0.0).is_ok());
        assert!(volume_request(DeviceKind::Source, 1.0).is_ok());
    }

    #[test]
    fn invalid_volume_is_rejected_without_enqueueing_and_old_control_is_inactive() {
        let (cancel_sender, cancellation) = worker::cancellation_channel();
        let (commands, receiver) =
            command::channel::<Request, (), AudioError>(NonZeroUsize::MIN, cancellation);
        let control = AudioControl { commands };
        assert!(matches!(
            control.set_output_volume(f32::INFINITY),
            Err(AudioControlError::InvalidVolume)
        ));
        let operation = control
            .set_input_volume(0.75)
            .expect("valid request is admitted");
        let request = block_on(receiver.recv()).expect("valid request reaches worker");
        let (request, reply) = request.into_parts();
        assert!(matches!(request, Request::Volume(DeviceKind::Source, value) if value == 0.75));
        reply(Ok(()));
        block_on(operation).expect("fake worker replies successfully");

        drop(cancel_sender);
        assert!(matches!(
            control.set_output_muted(true),
            Err(AudioControlError::Dispatch(crate::DispatchError::Inactive))
        ));
        drop(receiver);
    }

    #[test]
    fn pulse_command_waits_only_while_context_and_operation_are_running() {
        assert_eq!(
            command_progress(false, false, OperationState::Running, context::State::Ready,),
            CommandProgress::Pending
        );
        assert_eq!(
            command_progress(false, false, OperationState::Done, context::State::Ready,),
            CommandProgress::Failed
        );
        assert_eq!(
            command_progress(
                false,
                false,
                OperationState::Cancelled,
                context::State::Ready,
            ),
            CommandProgress::Failed
        );
        for state in [context::State::Failed, context::State::Terminated] {
            assert_eq!(
                command_progress(false, false, OperationState::Running, state),
                CommandProgress::Failed
            );
        }
        assert_eq!(
            command_progress(true, true, OperationState::Done, context::State::Ready,),
            CommandProgress::Completed(true)
        );
        assert_eq!(
            command_progress(true, false, OperationState::Done, context::State::Ready,),
            CommandProgress::Completed(false)
        );
    }
}
