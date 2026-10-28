mod binding;
mod error;
mod runtime;

pub use binding::MountedBinding;
pub use error::ShellError;
pub use runtime::RuntimeHandle;

use gpui::App;

/// Shell entry points.
pub struct Shell;

impl Shell {
    /// Attaches to an existing app and runs setup before returning the handle.
    ///
    /// Fatal source failures and setup failures request quit of the entire host
    /// app. Setup failure immediately stops this runtime's resources.
    pub fn attach(
        cx: &mut App,
        setup: impl FnOnce(RuntimeHandle, &mut App) -> Result<(), ShellError>,
    ) -> Result<RuntimeHandle, ShellError> {
        let runtime = RuntimeHandle::new(cx);
        if let Err(error) = setup(runtime.clone(), cx) {
            return Err(runtime.setup_failed(error, cx));
        }
        if let Some(Err(error)) = runtime.result() {
            return Err(error);
        }
        Ok(runtime)
    }

    #[cfg(target_os = "linux")]
    /// Runs the Linux GPUI event loop and returns its finalized shell result.
    pub fn run(
        setup: impl FnOnce(RuntimeHandle, &mut App) -> Result<(), ShellError> + 'static,
    ) -> Result<(), ShellError> {
        use std::{cell::RefCell, rc::Rc};
        let result = Rc::new(RefCell::new(None));
        let app_result = Rc::clone(&result);
        let retained = Rc::new(RefCell::new(None));
        let app_retained = Rc::clone(&retained);

        gpui_platform::application().run(move |cx| match Self::attach(cx, setup) {
            Ok(handle) => {
                *app_result.borrow_mut() = handle.result();
                *app_retained.borrow_mut() = Some(handle);
            }
            Err(error) => *app_result.borrow_mut() = Some(Err(error)),
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
pub(super) mod test_support {
    use crate::{Source, SourceDescriptor, SourcePublisher};
    use gpui::App;
    use std::{
        cell::{Cell, RefCell},
        error::Error,
        fmt,
        rc::Rc,
    };

    #[derive(Debug)]
    pub(super) struct TestError(pub(super) &'static str);

    impl fmt::Display for TestError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    impl Error for TestError {}

    pub(super) struct TestSource {
        pub(super) key: u8,
        pub(super) starts: Rc<Cell<usize>>,
        pub(super) stops: Rc<Cell<usize>>,
        pub(super) publisher: Rc<RefCell<Option<SourcePublisher<Self>>>>,
        pub(super) fails: bool,
        pub(super) on_start: Option<TestStart>,
        pub(super) on_drop: Option<Rc<dyn Fn()>>,
    }

    pub(super) struct TestGuard(pub(super) Rc<Cell<usize>>, Option<Rc<dyn Fn()>>);

    pub(super) type TestDescriptor = SourceDescriptor<TestSource>;
    pub(super) type TestPublisher = Rc<RefCell<Option<SourcePublisher<TestSource>>>>;
    pub(super) type TestStart = Rc<dyn Fn(SourcePublisher<TestSource>, &mut App)>;

    impl Drop for TestGuard {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
            if let Some(on_drop) = &self.1 {
                on_drop();
            }
        }
    }

    impl Source for TestSource {
        type Key = u8;
        type Value = u8;
        type Error = TestError;
        type Guard = TestGuard;

        fn key(&self) -> u8 {
            self.key
        }

        fn start(
            &self,
            publisher: SourcePublisher<Self>,
            cx: &mut App,
        ) -> Result<Self::Guard, Self::Error> {
            self.starts.set(self.starts.get() + 1);
            *self.publisher.borrow_mut() = Some(publisher.clone());
            if let Some(on_start) = &self.on_start {
                on_start(publisher, cx);
            }
            if self.fails {
                Err(TestError("source start"))
            } else {
                Ok(TestGuard(Rc::clone(&self.stops), self.on_drop.clone()))
            }
        }
    }

    pub(super) fn descriptor(
        starts: Rc<Cell<usize>>,
        stops: Rc<Cell<usize>>,
        fails: bool,
    ) -> (TestDescriptor, TestPublisher) {
        descriptor_with_options(starts, stops, fails, None, None, 1)
    }

    pub(super) fn descriptor_with_options(
        starts: Rc<Cell<usize>>,
        stops: Rc<Cell<usize>>,
        fails: bool,
        on_start: Option<TestStart>,
        on_drop: Option<Rc<dyn Fn()>>,
        key: u8,
    ) -> (TestDescriptor, TestPublisher) {
        let publisher = Rc::new(RefCell::new(None));
        (
            SourceDescriptor::new(TestSource {
                key,
                starts,
                stops,
                publisher: Rc::clone(&publisher),
                fails,
                on_start,
                on_drop,
            }),
            publisher,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use gpui::TestAppContext;
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
