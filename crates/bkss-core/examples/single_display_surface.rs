use std::{
    any::TypeId,
    cell::{Cell, RefCell},
    error::Error,
    io,
    process::ExitCode,
    rc::Rc,
    time::Duration,
};

use bkss_core::{
    RuntimeHandle, Shell, ShellError, Source, SourceDescriptor, SourceFailurePhase,
    SourcePublisher, SurfaceDescriptor,
};
use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, DisplayId, Render, Size, Window,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, div,
    layer_shell::{Anchor, KeyboardInteractivity, LayerShellOptions},
    prelude::*,
    px, rgb,
};
use gpui_platform::application;

const READINESS_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_CHECK_DELAY: Duration = Duration::from_millis(500);
const HOST_OBSERVATION_DELAY: Duration = Duration::from_secs(1);
const FAILURE_MESSAGE: &str = "single-display example subscription failure";
const USAGE: &str = "usage: single_display_surface <run-normal|run-fatal|run-repeat|attach-normal|attach-fatal> --display ID [--seconds 15]\nUse the numeric ID inside DisplayId(ID) printed by the list_displays example; seconds must be 1..=300.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    RunNormal,
    RunFatal,
    RunRepeat,
    AttachNormal,
    AttachFatal,
}

impl Mode {
    fn is_run(self) -> bool {
        matches!(self, Self::RunNormal | Self::RunFatal | Self::RunRepeat)
    }

    fn is_fatal(self) -> bool {
        matches!(self, Self::RunFatal | Self::AttachFatal)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Args {
    mode: Mode,
    display: DisplayId,
    seconds: u64,
}

fn parse_args(args: &[String]) -> Result<Args, &'static str> {
    let mode = match args.first().map(String::as_str) {
        Some("run-normal") => Mode::RunNormal,
        Some("run-fatal") => Mode::RunFatal,
        Some("run-repeat") => Mode::RunRepeat,
        Some("attach-normal") => Mode::AttachNormal,
        Some("attach-fatal") => Mode::AttachFatal,
        _ => return Err("select a lifecycle mode"),
    };
    let mut display = None;
    let mut seconds = None;
    let mut flags = args[1..].iter();
    while let Some(flag) = flags.next() {
        let value = flags.next().ok_or("flag requires a value")?;
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("flag values must be unsigned decimal integers");
        }
        match flag.as_str() {
            "--display" if display.is_none() => {
                display = Some(DisplayId::new(
                    value.parse().map_err(|_| "invalid display ID")?,
                ));
            }
            "--seconds" if seconds.is_none() => {
                let value = value.parse::<u64>().map_err(|_| "invalid seconds")?;
                if !(1..=300).contains(&value) {
                    return Err("seconds must be 1..=300");
                }
                seconds = Some(value);
            }
            _ => return Err("unknown or repeated flag"),
        }
    }
    Ok(Args {
        mode,
        display: display.ok_or("--display ID is required")?,
        seconds: seconds.unwrap_or(15),
    })
}

struct Rectangle {
    host: bool,
}

impl Render for Rectangle {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(if self.host { 0xd7eee8 } else { 0x19394c }))
            .text_color(rgb(if self.host { 0x19394c } else { 0xffffff }))
            .text_size(px(18.))
            .child(if self.host {
                "BKSS host witness"
            } else {
                "BKSS single-display surface"
            })
    }
}

#[derive(Default)]
struct Observation {
    creations: Cell<u32>,
    surface: Cell<Option<AnyWindowHandle>>,
    action_ran: Cell<bool>,
    host_verified: Cell<bool>,
    watchdog_fired: Cell<bool>,
}

// The host retains its own window and attached runtime until its event loop ends.
#[derive(Default)]
struct Host {
    window: Option<WindowHandle<Rectangle>>,
    runtime: Option<RuntimeHandle>,
}

struct FailureSource(Rc<RefCell<Option<SourcePublisher<Self>>>>);

impl Source for FailureSource {
    type Key = ();
    type Value = ();
    type Error = io::Error;
    type Guard = ();

    fn key(&self) -> Self::Key {}

    fn start(&self, publisher: SourcePublisher<Self>, _cx: &mut App) -> Result<(), io::Error> {
        *self.0.borrow_mut() = Some(publisher);
        Ok(())
    }
}

fn expected_result(result: &Result<(), ShellError>, fatal: bool) -> bool {
    match (result, fatal) {
        (Ok(()), false) => true,
        (Err(ShellError::Source(failure)), true) => {
            failure.phase() == SourceFailurePhase::Subscription
                && failure.source_type() == TypeId::of::<FailureSource>()
                && failure.source().map(ToString::to_string).as_deref() == Some(FAILURE_MESSAGE)
        }
        _ => false,
    }
}

fn open_host_witness(args: Args, cx: &mut App) -> Result<WindowHandle<Rectangle>, ShellError> {
    cx.open_window(
        WindowOptions {
            display_id: Some(args.display),
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: gpui::point(px(0.), px(0.)),
                size: Size::new(px(260.), px(64.)),
            })),
            focus: false,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: "bkss-example-host-witness".into(),
                anchor: Anchor::TOP | Anchor::LEFT,
                keyboard_interactivity: KeyboardInteractivity::None,
                exclusive_zone: Some(px(0.)),
                ..Default::default()
            }),
            ..Default::default()
        },
        |window, cx| {
            window.set_input_region(Some(&[]));
            cx.new(|_| Rectangle { host: true })
        },
    )
    .map_err(|error| ShellError::setup(io::Error::other(error)))
}

fn setup(
    args: Args,
    runtime: RuntimeHandle,
    cx: &mut App,
    observation: Rc<Observation>,
    host: Option<Rc<RefCell<Host>>>,
) -> Result<(), ShellError> {
    let (binding, publisher) = if args.mode.is_fatal() {
        let slot = Rc::new(RefCell::new(None));
        let descriptor = SourceDescriptor::new(FailureSource(Rc::clone(&slot)));
        let binding = runtime.mount(&descriptor, cx)?;
        let publisher = slot
            .borrow_mut()
            .take()
            .ok_or_else(|| ShellError::setup(io::Error::other("failure source did not start")))?;
        (Some(binding), Some(publisher))
    } else {
        (None, None)
    };

    let watchdog_runtime = runtime.clone();
    let watchdog_observation = Rc::clone(&observation);
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(READINESS_TIMEOUT + Duration::from_secs(args.seconds + 3))
            .await;
        watchdog_observation.watchdog_fired.set(true);
        eprintln!(
            "{:?}: watchdog elapsed; result = {:?}",
            args.mode,
            watchdog_runtime.result()
        );
        cx.update(|cx| {
            watchdog_runtime.shutdown(cx);
            if !args.mode.is_run() {
                // This is the host's bounded exit, not the attached runtime's quit policy.
                cx.quit();
            }
        });
    })
    .detach();

    let action_runtime = runtime.clone();
    let descriptor = SurfaceDescriptor::new(
        args.display,
        Size::new(px(360.), px(96.)),
        move |window, cx| {
            observation.creations.set(observation.creations.get() + 1);
            observation.surface.set(Some(window.window_handle()));
            println!(
                "{:?}: content factory #{} for {:?}; visibility/input still require native observation",
                args.mode, observation.creations.get(), args.display
            );
            cx.spawn(async move |cx| {
                if let Some(host) = &host {
                    let opened = cx.update(|cx| open_host_witness(args, cx));
                    match opened {
                        Ok(window) => host.borrow_mut().window = Some(window),
                        Err(error) => {
                            eprintln!("host witness creation failed: {error}");
                            cx.update(|cx| {
                                action_runtime.shutdown(cx);
                                cx.quit();
                            });
                            return;
                        }
                    }
                }
                // Start observation time only after the runtime content factory ran.
                cx.background_executor().timer(Duration::from_secs(args.seconds)).await;
                observation.action_ran.set(true);
                cx.update(|cx| {
                    if let Some(publisher) = publisher {
                        println!("{:?}: publishing delayed subscription failure", args.mode);
                        publisher.fail(io::Error::other(FAILURE_MESSAGE), cx);
                    } else {
                        println!("{:?}: shutdown #1", args.mode);
                        action_runtime.shutdown(cx);
                        if args.mode == Mode::RunRepeat {
                            println!("{:?}: shutdown #2", args.mode);
                            action_runtime.shutdown(cx);
                        }
                    }
                });
                // A publisher is not a consumer: retain the binding through the failure.
                drop(binding);
                if let Some(host) = host {
                    cx.background_executor().timer(HOST_CHECK_DELAY).await;
                    cx.update(|cx| {
                        let host_alive = host.borrow().window.as_ref().is_some_and(|window| {
                            window.read_with(cx, |_, _| ()).is_ok()
                        });
                        let surface_removed = observation.surface.get().is_some_and(|surface| {
                            surface.update(cx, |_, _, _| ()).is_err()
                        });
                        let result = action_runtime.result();
                        let result_matches = if args.mode.is_fatal() {
                            result.as_ref().is_some_and(|result| expected_result(result, true))
                        } else {
                            result.is_none()
                        };
                        observation.host_verified.set(host_alive && surface_removed && result_matches);
                        println!(
                            "{:?}: host alive={host_alive}; exact runtime surface removed={surface_removed}; runtime result before host quit={result:?}",
                            args.mode
                        );
                    });
                    cx.background_executor().timer(HOST_OBSERVATION_DELAY).await;
                    println!("{:?}: host requests its own quit", args.mode);
                    cx.update(|cx| cx.quit());
                }
            })
            .detach();
            cx.new(|_| Rectangle { host: false })
        },
    )?
    .readiness_timeout(READINESS_TIMEOUT);
    runtime.mount_surface(descriptor, cx)
}

fn main() -> ExitCode {
    let args = match parse_args(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "requested target={:?}; mode={:?}; observation={}s",
        args.display, args.mode, args.seconds
    );
    let observation = Rc::new(Observation::default());
    let setup_observation = Rc::clone(&observation);
    let result = if args.mode.is_run() {
        Some(Shell::run(move |runtime, cx| {
            setup(args, runtime, cx, setup_observation, None)
        }))
    } else {
        let host = Rc::new(RefCell::new(Host::default()));
        let app_host = Rc::clone(&host);
        let attach_error = Rc::new(RefCell::new(None));
        let app_error = Rc::clone(&attach_error);
        application().run(move |cx| {
            let setup_host = Rc::clone(&app_host);
            match Shell::attach(cx, move |runtime, cx| {
                setup(args, runtime, cx, setup_observation, Some(setup_host))
            }) {
                Ok(runtime) => app_host.borrow_mut().runtime = Some(runtime),
                Err(error) => {
                    *app_error.borrow_mut() = Some(error);
                    cx.quit();
                }
            }
        });
        let error = attach_error.borrow_mut().take();
        if let Some(error) = error {
            Some(Err(error))
        } else {
            host.borrow()
                .runtime
                .as_ref()
                .and_then(RuntimeHandle::result)
        }
    };
    println!(
        "{:?}: final caller-side result={result:?}; creations={}",
        args.mode,
        observation.creations.get()
    );
    let success = observation.creations.get() == 1
        && observation.action_ran.get()
        && !observation.watchdog_fired.get()
        && (args.mode.is_run() || observation.host_verified.get())
        && result
            .as_ref()
            .is_some_and(|result| expected_result(result, args.mode.is_fatal()));
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::{Args, DisplayId, Mode, parse_args};

    fn parse(args: &[&str]) -> Result<Args, &'static str> {
        parse_args(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn accepts_all_modes_and_explicit_display_with_default_or_bounded_duration() {
        for (name, mode) in [
            ("run-normal", Mode::RunNormal),
            ("run-fatal", Mode::RunFatal),
            ("run-repeat", Mode::RunRepeat),
            ("attach-normal", Mode::AttachNormal),
            ("attach-fatal", Mode::AttachFatal),
        ] {
            assert_eq!(
                parse(&[name, "--display", "0"]),
                Ok(Args {
                    mode,
                    display: DisplayId::new(0),
                    seconds: 15
                })
            );
            for (seconds, duration) in [("1", 1), ("300", 300)] {
                assert_eq!(
                    parse(&[
                        name,
                        "--seconds",
                        seconds,
                        "--display",
                        "18446744073709551615"
                    ]),
                    Ok(Args {
                        mode,
                        display: DisplayId::new(u64::MAX),
                        seconds: duration,
                    })
                );
            }
        }
    }

    #[test]
    fn rejects_missing_unknown_repeated_malformed_and_out_of_range_arguments() {
        for args in [
            vec![],
            vec!["unknown"],
            vec!["run-normal"],
            vec!["run-normal", "--display"],
            vec!["run-normal", "--display", "-1"],
            vec!["run-normal", "--display", "+1"],
            vec!["run-normal", "--display", "DisplayId(1)"],
            vec!["run-normal", "--display", "18446744073709551616"],
            vec!["run-normal", "--display", ""],
            vec!["run-normal", "--display", "1", "--seconds", "0"],
            vec!["run-normal", "--display", "1", "--seconds", "301"],
            vec!["run-normal", "--display", "1", "--seconds", "1.5"],
            vec![
                "run-normal",
                "--display",
                "1",
                "--seconds",
                "18446744073709551616",
            ],
            vec!["run-normal", "--display", "1", "--display", "2"],
            vec![
                "run-normal",
                "--display",
                "1",
                "--seconds",
                "1",
                "--seconds",
                "2",
            ],
            vec!["run-normal", "--display", "1", "--unknown", "1"],
            vec!["run-normal", "--display", "1", "extra"],
        ] {
            assert!(parse(&args).is_err(), "accepted {args:?}");
        }
    }
}
