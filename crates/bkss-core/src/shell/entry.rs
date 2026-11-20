use super::{RuntimeHandle, ShellError};
use gpui::App;

/// Shell entry points.
pub struct Shell;

impl Shell {
    /// Attaches to an existing app and runs setup before returning the handle.
    ///
    /// This runtime owns only resources and windows it creates. Setup and fatal
    /// failures stop those resources without requesting that the host app quit.
    /// It does not change the host's GPUI quit mode; GPUI may apply its existing
    /// last-window quit policy if a BKSS surface is the app's only open window.
    pub fn attach(
        cx: &mut App,
        setup: impl FnOnce(RuntimeHandle, &mut App) -> Result<(), ShellError>,
    ) -> Result<RuntimeHandle, ShellError> {
        Self::attach_with_quit_policy(cx, setup, false)
    }

    fn attach_with_quit_policy(
        cx: &mut App,
        setup: impl FnOnce(RuntimeHandle, &mut App) -> Result<(), ShellError>,
        quit_host_on_failure: bool,
    ) -> Result<RuntimeHandle, ShellError> {
        let runtime = RuntimeHandle::new(cx, quit_host_on_failure);
        if let Err(error) = setup(runtime.clone(), cx) {
            return Err(runtime.setup_failed(error, cx));
        }
        if let Some(Err(error)) = runtime.result() {
            return Err(error);
        }
        Ok(runtime)
    }

    #[cfg(target_os = "linux")]
    /// Owns the Linux GPUI application, event loop, and quit policy, then returns
    /// the finalized shell result. Unlike [`Self::attach`], failures request app quit.
    pub fn run(
        setup: impl FnOnce(RuntimeHandle, &mut App) -> Result<(), ShellError> + 'static,
    ) -> Result<(), ShellError> {
        use std::{cell::RefCell, rc::Rc};
        let result = Rc::new(RefCell::new(None));
        let app_result = Rc::clone(&result);
        let retained = Rc::new(RefCell::new(None));
        let app_retained = Rc::clone(&retained);

        gpui_platform::application().run(move |cx| {
            match Self::attach_with_quit_policy(cx, setup, true) {
                Ok(handle) => {
                    *app_result.borrow_mut() = handle.result();
                    *app_retained.borrow_mut() = Some(handle);
                }
                Err(error) => *app_result.borrow_mut() = Some(Err(error)),
            }
        });

        if let Some(handle) = retained.borrow().as_ref() {
            handle.finish_after_run()
        } else {
            result.borrow().clone()
        }
        .ok_or_else(|| {
            ShellError::setup(std::io::Error::other(
                "GPUI application exited without launching a shell",
            ))
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::test_support::*;
    use gpui::{AppContext, TestAppContext, WindowOptions};
    use std::{
        any::TypeId,
        cell::{Cell, RefCell},
        error::Error,
        rc::Rc,
    };

    #[test]
    fn source_failure_remains_primary_over_a_later_setup_error() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let (descriptor, _) = descriptor(starts.clone(), stops.clone(), true);
        let result = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                assert!(runtime.mount(&descriptor, cx).is_err());
                Err(ShellError::setup(TestError("later setup error")))
            })
        });

        let Err(ShellError::Source(failure)) = result else {
            panic!("expected source failure")
        };
        assert_eq!(failure.phase(), crate::SourceFailurePhase::Start);
        assert_eq!(failure.source_type(), TypeId::of::<TestSource>());
        assert_eq!(
            failure
                .source()
                .and_then(|e| e.downcast_ref::<TestError>())
                .map(|e| e.0),
            Some("source start")
        );
        assert_eq!(starts.get(), 1);
        assert_eq!(stops.get(), 0);
    }

    #[test]
    fn ignored_start_failure_with_successful_setup_still_fails_attach() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let (descriptor, _) = descriptor(starts.clone(), Rc::new(Cell::new(0)), true);
        let result = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                assert!(runtime.mount(&descriptor, cx).is_err());
                Ok(())
            })
        });

        assert!(matches!(result, Err(ShellError::Source(_))));
        assert_eq!(starts.get(), 1);
    }

    #[test]
    fn shutdown_during_setup_does_not_replace_later_setup_failure() {
        let cx = TestAppContext::single();
        let result = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                runtime.shutdown(cx);
                assert!(runtime.result().is_none());
                Err(ShellError::setup(TestError("after shutdown")))
            })
        });

        let Err(ShellError::Setup(error)) = result else {
            panic!("expected setup failure")
        };
        assert_eq!(
            error.downcast_ref::<TestError>().map(|error| error.0),
            Some("after shutdown")
        );
    }

    #[test]
    fn subscription_failure_is_retained_and_stops_all_mounted_resources() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let (descriptor, publisher) = descriptor(starts, stops.clone(), false);
        let binding = Rc::new(RefCell::new(None));
        let held_binding = Rc::clone(&binding);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                *held_binding.borrow_mut() = Some(runtime.mount(&descriptor, cx)?);
                Ok(())
            })
            .unwrap()
        });
        let publisher = publisher.borrow().as_ref().unwrap().clone();

        cx.update(|cx| publisher.fail(TestError("subscription"), cx));
        let Some(Err(ShellError::Source(failure))) = runtime.result() else {
            panic!("expected retained source failure")
        };
        assert_eq!(failure.phase(), crate::SourceFailurePhase::Subscription);
        assert_eq!(
            failure
                .source()
                .and_then(|e| e.downcast_ref::<TestError>())
                .map(|e| e.0),
            Some("subscription")
        );
        assert_eq!(stops.get(), 1);
        cx.quit();
        assert!(matches!(runtime.result(), Some(Err(ShellError::Source(_)))));
        drop(binding);
    }

    #[test]
    fn attached_fatal_source_failure_preserves_an_unowned_host_window() {
        let cx = TestAppContext::single();
        let host = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap()
        });
        let (descriptor, publisher) =
            descriptor(Rc::new(Cell::new(0)), Rc::new(Cell::new(0)), false);
        let binding = Rc::new(RefCell::new(None));
        let binding_slot = Rc::clone(&binding);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                *binding_slot.borrow_mut() = Some(runtime.mount(&descriptor, cx)?);
                Ok(())
            })
            .unwrap()
        });

        cx.update(|cx| {
            publisher
                .borrow()
                .as_ref()
                .unwrap()
                .fail(TestError("fatal"), cx)
        });

        assert!(matches!(runtime.result(), Some(Err(ShellError::Source(_)))));
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        assert!(host.read_with(&cx, |_, _| ()).is_ok());
        drop(binding);
    }

    #[test]
    fn fatal_failure_cleans_every_independent_key_activation() {
        let cx = TestAppContext::single();
        let first_starts = Rc::new(Cell::new(0));
        let first_stops = Rc::new(Cell::new(0));
        let second_starts = Rc::new(Cell::new(0));
        let second_stops = Rc::new(Cell::new(0));
        let (first, publisher) =
            descriptor_with_options(first_starts, first_stops.clone(), false, None, None, 1);
        let (second, _) =
            descriptor_with_options(second_starts, second_stops.clone(), false, None, None, 2);
        let bindings = Rc::new(RefCell::new(Vec::new()));
        let setup_bindings = Rc::clone(&bindings);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                setup_bindings.borrow_mut().push(runtime.mount(&first, cx)?);
                setup_bindings
                    .borrow_mut()
                    .push(runtime.mount(&second, cx)?);
                Ok(())
            })
            .unwrap()
        });

        let publisher = publisher.borrow().as_ref().unwrap().clone();
        cx.update(|cx| publisher.fail(TestError("first key failed"), cx));

        assert_eq!((first_stops.get(), second_stops.get()), (1, 1));
        assert!(matches!(runtime.result(), Some(Err(ShellError::Source(_)))));
        drop(bindings);
        assert_eq!((first_stops.get(), second_stops.get()), (1, 1));
    }

    #[test]
    fn setup_error_keeps_source_and_primary_error_context() {
        let cx = TestAppContext::single();
        let error =
            cx.update(|cx| Shell::attach(cx, |_, _| Err(ShellError::setup(TestError("setup")))));
        let Err(ShellError::Setup(error)) = error else {
            panic!("expected setup error")
        };
        assert_eq!(
            error.downcast_ref::<TestError>().map(|error| error.0),
            Some("setup")
        );
    }
}
