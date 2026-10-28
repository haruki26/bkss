use std::{
    cell::{Cell, RefCell},
    error::Error,
    fmt,
    process::ExitCode,
    rc::Rc,
    time::Duration,
};

use bkss_core::{
    MountedBinding, RuntimeHandle, Shell, ShellError, Source, SourceDescriptor, SourceFailurePhase,
    SourcePublisher,
};
use gpui::App;
use gpui_platform::application;

const RUNTIME_ACTION_DELAY: Duration = Duration::from_millis(100);
const HOST_CHECK_DELAY: Duration = Duration::from_millis(250);
const HOST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    RunNormal,
    RunFatal,
    AttachNormal,
    AttachFatal,
}

impl Mode {
    fn is_run(self) -> bool {
        matches!(self, Self::RunNormal | Self::RunFatal)
    }

    fn is_fatal(self) -> bool {
        matches!(self, Self::RunFatal | Self::AttachFatal)
    }

    fn name(self) -> &'static str {
        match self {
            Self::RunNormal => "run-normal",
            Self::RunFatal => "run-fatal",
            Self::AttachNormal => "attach-normal",
            Self::AttachFatal => "attach-fatal",
        }
    }
}

#[derive(Debug)]
struct DemoFailure(&'static str);

impl fmt::Display for DemoFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for DemoFailure {}

struct DemoSource {
    publisher: Rc<RefCell<Option<SourcePublisher<Self>>>>,
    stops: Rc<Cell<usize>>,
}

struct DemoGuard(Rc<Cell<usize>>);

impl Drop for DemoGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

impl Source for DemoSource {
    type Key = &'static str;
    type Value = ();
    type Error = DemoFailure;
    type Guard = DemoGuard;

    fn key(&self) -> Self::Key {
        "shell-lifecycle-example"
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        _cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        *self.publisher.borrow_mut() = Some(publisher);
        Ok(DemoGuard(Rc::clone(&self.stops)))
    }
}

type BindingSlot = Rc<RefCell<Option<MountedBinding<()>>>>;
type PublisherSlot = Rc<RefCell<Option<SourcePublisher<DemoSource>>>>;
type HostReport = Rc<RefCell<Option<(bool, String)>>>;
type StopCounter = Rc<Cell<usize>>;

fn parse_mode(args: &[String]) -> Option<Mode> {
    if args.len() != 1 {
        return None;
    }
    match args[0].as_str() {
        "run-normal" => Some(Mode::RunNormal),
        "run-fatal" => Some(Mode::RunFatal),
        "attach-normal" => Some(Mode::AttachNormal),
        "attach-fatal" => Some(Mode::AttachFatal),
        _ => None,
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let Some(mode) = parse_mode(&args) else {
        eprintln!(
            "usage: cargo run -p bkss-core --example shell_lifecycle -- \
             <run-normal|run-fatal|attach-normal|attach-fatal>"
        );
        return ExitCode::FAILURE;
    };

    let success = if mode.is_run() {
        run_mode(mode)
    } else {
        attach_mode(mode)
    };

    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn retain_source(
    runtime: &RuntimeHandle,
    cx: &mut App,
    binding_slot: &BindingSlot,
    publisher_slot: &PublisherSlot,
    stops: &StopCounter,
) -> Result<(), ShellError> {
    let descriptor = SourceDescriptor::new(DemoSource {
        publisher: Rc::clone(publisher_slot),
        stops: Rc::clone(stops),
    });
    *binding_slot.borrow_mut() = Some(runtime.mount(&descriptor, cx)?);
    Ok(())
}

fn schedule_runtime_action(
    mode: Mode,
    runtime: RuntimeHandle,
    cx: &mut App,
    publisher_slot: &PublisherSlot,
) -> Result<(), ShellError> {
    let publisher =
        if mode.is_fatal() {
            Some(
                publisher_slot.borrow().as_ref().cloned().ok_or_else(|| {
                    ShellError::setup(std::io::Error::other("source did not start"))
                })?,
            )
        } else {
            None
        };

    cx.spawn(async move |cx| {
        cx.background_executor().timer(RUNTIME_ACTION_DELAY).await;
        if let Some(publisher) = publisher {
            cx.update(|cx| publisher.fail(DemoFailure("intentional subscription failure"), cx));
        } else {
            cx.update(|cx| runtime.shutdown(cx));
        }
    })
    .detach();
    Ok(())
}

fn run_mode(mode: Mode) -> bool {
    let binding_slot = Rc::new(RefCell::new(None));
    let publisher_slot = Rc::new(RefCell::new(None));
    let stops = Rc::new(Cell::new(0));
    let setup_binding = Rc::clone(&binding_slot);
    let setup_publisher = Rc::clone(&publisher_slot);
    let setup_stops = Rc::clone(&stops);

    let result = Shell::run(move |runtime, cx| {
        retain_source(&runtime, cx, &setup_binding, &setup_publisher, &setup_stops)?;
        schedule_runtime_action(mode, runtime, cx, &setup_publisher)
    });

    let result_matches = if mode.is_fatal() {
        matches!(&result, Err(error) if is_expected_fatal(error))
    } else {
        result.is_ok()
    };
    let stops_before_binding_drop = stops.get();
    drop(binding_slot.borrow_mut().take());
    let stops_after_binding_drop = stops.get();
    let success = result_matches && stops_before_binding_drop == 1 && stops_after_binding_drop == 1;
    println!(
        "{}: runtime returned {result:?}; source guard drops before/after binding drop = {stops_before_binding_drop}/{stops_after_binding_drop}",
        mode.name()
    );
    success
}

fn attach_mode(mode: Mode) -> bool {
    let binding_slot = Rc::new(RefCell::new(None));
    let publisher_slot = Rc::new(RefCell::new(None));
    let stops = Rc::new(Cell::new(0));
    let report: HostReport = Rc::new(RefCell::new(None));
    let runtime_slot = Rc::new(RefCell::new(None));
    let app_binding = Rc::clone(&binding_slot);
    let app_publisher = Rc::clone(&publisher_slot);
    let app_stops = Rc::clone(&stops);
    let app_report = Rc::clone(&report);
    let app_runtime = Rc::clone(&runtime_slot);

    application().run(move |cx: &mut App| {
        let setup_binding = Rc::clone(&app_binding);
        let setup_publisher = Rc::clone(&app_publisher);
        let setup_stops = Rc::clone(&app_stops);
        let attached = Shell::attach(cx, move |runtime, cx| {
            retain_source(
                &runtime,
                cx,
                &setup_binding,
                &setup_publisher,
                &setup_stops,
            )?;
            schedule_runtime_action(mode, runtime, cx, &setup_publisher)
        });

        let runtime = match attached {
            Ok(runtime) => runtime,
            Err(error) => {
                *app_report.borrow_mut() = Some((false, format!("attach failed: {error}")));
                cx.quit();
                return;
            }
        };
        *app_runtime.borrow_mut() = Some(runtime.clone());

        let checked_runtime = runtime.clone();
        let checked_report = Rc::clone(&app_report);
        let checked_stops = Rc::clone(&app_stops);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(HOST_CHECK_DELAY).await;
            let observed = checked_runtime.result();
            let stop_count = checked_stops.get();
            let matches = match (&observed, mode.is_fatal()) {
                (None, false) => stop_count == 1,
                (Some(Err(error)), true) => is_expected_fatal(error) && stop_count == 1,
                _ => false,
            };
            let message = format!(
                "pre-quit host callback ran; runtime result snapshot = {observed:?}; source guard drops = {stop_count}"
            );
            *checked_report.borrow_mut() = Some((matches, message));
            cx.update(|cx| cx.quit());
        })
        .detach();

        let timeout_report = Rc::clone(&app_report);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(HOST_TIMEOUT).await;
            let timed_out = timeout_report.borrow().is_none();
            if timed_out {
                *timeout_report.borrow_mut() = Some((false, "host timeout elapsed".into()));
                cx.update(|cx| cx.quit());
            }
        })
        .detach();
    });

    let final_result = runtime_slot
        .borrow()
        .as_ref()
        .and_then(RuntimeHandle::result);
    let final_result_matches = if mode.is_fatal() {
        matches!(&final_result, Some(Err(error)) if is_expected_fatal(error))
    } else {
        matches!(&final_result, Some(Ok(())))
    };
    let stops_before_binding_drop = stops.get();
    drop(binding_slot.borrow_mut().take());
    let stops_after_binding_drop = stops.get();
    drop(runtime_slot.borrow_mut().take());

    let outcome = report.borrow_mut().take();
    match outcome {
        Some((success, message)) => {
            println!(
                "{}: {message}; final result after host quit = {final_result:?}; source guard drops before/after binding drop = {stops_before_binding_drop}/{stops_after_binding_drop}",
                mode.name()
            );
            success
                && final_result_matches
                && stops_before_binding_drop == 1
                && stops_after_binding_drop == 1
        }
        None => {
            eprintln!(
                "{}: GPUI exited without the host verification callback",
                mode.name()
            );
            false
        }
    }
}

fn is_expected_fatal(error: &ShellError) -> bool {
    matches!(
        error,
        ShellError::Source(failure)
            if failure.phase() == SourceFailurePhase::Subscription
                && failure
                    .source()
                    .and_then(|cause| cause.downcast_ref::<DemoFailure>())
                    .is_some_and(|cause| cause.0 == "intentional subscription failure")
    )
}

#[cfg(test)]
mod tests {
    use super::{Mode, parse_mode};

    #[test]
    fn parses_each_lifecycle_mode() {
        for (arg, mode) in [
            ("run-normal", Mode::RunNormal),
            ("run-fatal", Mode::RunFatal),
            ("attach-normal", Mode::AttachNormal),
            ("attach-fatal", Mode::AttachFatal),
        ] {
            assert_eq!(parse_mode(&[arg.to_owned()]), Some(mode));
        }
    }

    #[test]
    fn rejects_missing_extra_and_unknown_modes() {
        assert_eq!(parse_mode(&[]), None);
        assert_eq!(parse_mode(&["run-normal".into(), "extra".into()]), None);
        assert_eq!(parse_mode(&["unknown".into()]), None);
    }
}
