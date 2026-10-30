use bkss_core::{
    DisplaySelection, LayerSurfaceConfig, LayerSurfaceDeclaration, RuntimeHandle, Shell,
    ShellError, Source, SourceDescriptor, SourcePublisher, SurfaceDescriptor, SurfaceError,
};
use gpui::{
    App, AppContext, DisplayId, Size, TestAppContext, WindowOptions,
    layer_shell::LayerShellOptions, px,
};
use std::{
    cell::{Cell, RefCell},
    error::Error,
    rc::Rc,
};

struct FatalSource {
    stops: Rc<Cell<usize>>,
    publisher: FatalPublisher,
}

type FatalPublisher = Rc<RefCell<Option<SourcePublisher<FatalSource>>>>;

struct FatalGuard(Rc<Cell<usize>>);

impl Drop for FatalGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

impl Source for FatalSource {
    type Key = u8;
    type Value = ();
    type Error = std::io::Error;
    type Guard = FatalGuard;

    fn key(&self) -> Self::Key {
        1
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        _: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        *self.publisher.borrow_mut() = Some(publisher);
        Ok(FatalGuard(Rc::clone(&self.stops)))
    }
}

fn failing_source(stops: Rc<Cell<usize>>) -> (SourceDescriptor<FatalSource>, FatalPublisher) {
    let publisher = Rc::new(RefCell::new(None));
    (
        SourceDescriptor::new(FatalSource {
            stops,
            publisher: Rc::clone(&publisher),
        }),
        publisher,
    )
}

#[test]
fn surface_registration_allows_multiple_surfaces_and_rejects_after_shutdown() {
    let cx = TestAppContext::single();
    let called = Rc::new(Cell::new(0));
    let called_first = Rc::clone(&called);
    let runtime = cx.update(|cx| {
        Shell::attach(cx, |runtime, cx| {
            let descriptor = SurfaceDescriptor::new(
                DisplayId::new(99),
                Size::new(px(20.), px(20.)),
                move |_, cx| {
                    called_first.set(called_first.get() + 1);
                    cx.new(|_| gpui::EmptyView)
                },
            )?;
            runtime.mount_surface(descriptor, cx)?;
            let second = SurfaceDescriptor::new(
                DisplayId::new(99),
                Size::new(px(32.), px(18.)),
                |_, cx| cx.new(|_| gpui::EmptyView),
            )?;
            runtime.mount_surface(second, cx)
        })
        .unwrap()
    });
    let duplicate_called = Rc::clone(&called);
    let duplicate = SurfaceDescriptor::new(
        DisplayId::new(99),
        Size::new(px(20.), px(20.)),
        move |_, cx| {
            duplicate_called.set(duplicate_called.get() + 1);
            cx.new(|_| gpui::EmptyView)
        },
    )
    .unwrap();
    cx.update(|cx| runtime.mount_surface(duplicate, cx).unwrap());

    cx.update(|cx| runtime.shutdown(cx));
    let after_shutdown =
        SurfaceDescriptor::new(DisplayId::new(1), Size::new(px(20.), px(20.)), |_, cx| {
            cx.new(|_| gpui::EmptyView)
        })
        .unwrap();
    assert!(matches!(
        cx.update(|cx| runtime.mount_surface(after_shutdown, cx)),
        Err(ShellError::Setup(_))
    ));
    cx.run_until_parked();
    assert_eq!(called.get(), 0);
}

#[test]
fn multiple_layer_declarations_create_owned_surfaces_and_shutdown_preserves_host() {
    let cx = TestAppContext::single();
    let host = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui::EmptyView)
        })
        .unwrap()
    });
    let runtime = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        for _ in 0..2 {
            let declaration = LayerSurfaceDeclaration::new(
                DisplaySelection::AllConnected,
                |_, _| LayerSurfaceConfig {
                    size: Size::new(px(40.), px(20.)),
                    options: LayerShellOptions::default(),
                },
                |_, _, cx| cx.new(|_| gpui::EmptyView),
            );
            runtime.mount_layer_surface(declaration, cx).unwrap();
        }
        runtime
    });

    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 3);
    cx.update(|cx| runtime.shutdown(cx));
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
}

#[test]
fn layer_factory_shutdown_cancels_sibling_and_cleans_late_window() {
    let cx = TestAppContext::single();
    let called = Rc::new(Cell::new(0));
    let called_second = Rc::clone(&called);
    let runtime_slot = Rc::new(RefCell::new(None::<RuntimeHandle>));
    let runtime_for_factory = Rc::clone(&runtime_slot);
    let runtime = cx
        .update(|cx| {
            let runtime = Shell::attach(cx, |runtime, cx| {
                *runtime_slot.borrow_mut() = Some(runtime.clone());
                let first = LayerSurfaceDeclaration::new(
                    DisplaySelection::AllConnected,
                    |_, _| LayerSurfaceConfig {
                        size: Size::new(px(40.), px(20.)),
                        options: LayerShellOptions::default(),
                    },
                    move |_, _, cx| {
                        runtime_for_factory.borrow().as_ref().unwrap().shutdown(cx);
                        cx.new(|_| gpui::EmptyView)
                    },
                );
                runtime.mount_layer_surface(first, cx)?;
                let second = LayerSurfaceDeclaration::new(
                    DisplaySelection::AllConnected,
                    |_, _| LayerSurfaceConfig {
                        size: Size::new(px(40.), px(20.)),
                        options: LayerShellOptions::default(),
                    },
                    move |_, _, cx| {
                        called_second.set(called_second.get() + 1);
                        cx.new(|_| gpui::EmptyView)
                    },
                );
                runtime.mount_layer_surface(second, cx)
            })?;
            Ok::<_, ShellError>(runtime)
        })
        .unwrap();

    cx.run_until_parked();
    assert_eq!(called.get(), 0);
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    assert!(runtime.result().is_none());
}

#[test]
fn invalid_layer_configuration_is_fatal_with_cause_and_no_window() {
    let cx = TestAppContext::single();
    let runtime = cx.update(|cx| {
        Shell::attach(cx, |runtime, cx| {
            let declaration = LayerSurfaceDeclaration::new(
                DisplaySelection::AllConnected,
                |_, _| LayerSurfaceConfig {
                    size: Size::new(px(0.), px(20.)),
                    options: LayerShellOptions::default(),
                },
                |_, _, cx| cx.new(|_| gpui::EmptyView),
            );
            runtime.mount_layer_surface(declaration, cx)
        })
        .unwrap()
    });

    cx.run_until_parked();
    let Some(Err(ShellError::Surface(error))) = runtime.result() else {
        panic!("expected invalid layer configuration failure")
    };
    assert!(error.source().is_some());
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
}

#[test]
fn shutdown_drops_a_pending_layer_factory() {
    let cx = TestAppContext::single();
    let capture = Rc::new(());
    let weak_capture = Rc::downgrade(&capture);
    let runtime = cx.update(|cx| {
        Shell::attach(cx, |runtime, cx| {
            let factory_capture = Rc::clone(&capture);
            let declaration = LayerSurfaceDeclaration::new(
                DisplaySelection::Explicit(vec![DisplayId::new(99)]),
                |_, _| LayerSurfaceConfig {
                    size: Size::new(px(40.), px(20.)),
                    options: LayerShellOptions::default(),
                },
                move |_, _, cx| {
                    let _ = &factory_capture;
                    cx.new(|_| gpui::EmptyView)
                },
            );
            runtime.mount_layer_surface(declaration, cx)
        })
        .unwrap()
    });

    cx.run_until_parked();
    drop(capture);
    assert!(weak_capture.upgrade().is_some());
    cx.update(|cx| runtime.shutdown(cx));
    assert!(weak_capture.upgrade().is_none());
}

#[test]
fn attached_runtime_closes_only_its_surface_and_leaves_host_window_alive() {
    let cx = TestAppContext::single();
    let host = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui::EmptyView)
        })
        .unwrap()
    });
    let created = Rc::new(Cell::new(0));
    let created_by_factory = Rc::clone(&created);
    let runtime = cx.update(|cx| {
        Shell::attach(cx, |runtime, cx| {
            let descriptor = SurfaceDescriptor::new(
                DisplayId::new(1),
                Size::new(px(48.), px(20.)),
                move |_, cx| {
                    created_by_factory.set(created_by_factory.get() + 1);
                    cx.new(|_| gpui::EmptyView)
                },
            )?;
            runtime.mount_surface(descriptor, cx)?;
            let second =
                SurfaceDescriptor::new(DisplayId::new(1), Size::new(px(32.), px(18.)), |_, cx| {
                    cx.new(|_| gpui::EmptyView)
                })?;
            runtime.mount_surface(second, cx)
        })
        .unwrap()
    });

    cx.run_until_parked();
    assert_eq!(created.get(), 1);
    assert_eq!(cx.update(|cx| cx.windows().len()), 3);
    cx.update(|cx| runtime.shutdown(cx));
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
}

#[test]
fn shutdown_during_factory_removes_the_surface_created_after_reentrant_shutdown() {
    let cx = TestAppContext::single();
    let host = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui::EmptyView)
        })
        .unwrap()
    });
    let runtime_slot = Rc::new(RefCell::new(None::<RuntimeHandle>));
    let factory_runtime = Rc::clone(&runtime_slot);
    let runtime = cx.update(|cx| {
        Shell::attach(cx, |runtime, cx| {
            *runtime_slot.borrow_mut() = Some(runtime.clone());
            let descriptor = SurfaceDescriptor::new(
                DisplayId::new(1),
                Size::new(px(48.), px(20.)),
                move |_, cx| {
                    factory_runtime.borrow().as_ref().unwrap().shutdown(cx);
                    cx.new(|_| gpui::EmptyView)
                },
            )?;
            runtime.mount_surface(descriptor, cx)
        })
        .unwrap()
    });

    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
    assert!(runtime.result().is_none());
}

#[test]
fn readiness_timeout_is_a_fatal_typed_error_for_the_selected_display() {
    let cx = TestAppContext::single();
    let runtime = cx.update(|cx| {
        Shell::attach(cx, |runtime, cx| {
            let descriptor = SurfaceDescriptor::new(
                DisplayId::new(99),
                Size::new(px(48.), px(20.)),
                |_, cx| cx.new(|_| gpui::EmptyView),
            )?
            .readiness_timeout(std::time::Duration::ZERO);
            runtime.mount_surface(descriptor, cx)
        })
        .unwrap()
    });

    cx.run_until_parked();
    let Some(Err(ShellError::Surface(error))) = runtime.result() else {
        panic!("expected typed display readiness failure")
    };
    assert_eq!(error.display_id(), DisplayId::new(99));
    assert!(matches!(
        error.as_ref(),
        SurfaceError::DisplayReadinessTimeout { timeout, .. }
            if *timeout == std::time::Duration::ZERO
    ));
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
}

#[test]
fn fatal_source_failure_after_surface_creation_closes_only_owned_surface_once() {
    let cx = TestAppContext::single();
    let host = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui::EmptyView)
        })
        .unwrap()
    });
    let stops = Rc::new(Cell::new(0));
    let (resource, publisher) = failing_source(stops.clone());
    let binding = Rc::new(RefCell::new(None));
    let binding_slot = Rc::clone(&binding);
    let runtime = cx.update(|cx| {
        Shell::attach(cx, |runtime, cx| {
            *binding_slot.borrow_mut() = Some(runtime.mount(&resource, cx)?);
            let surface =
                SurfaceDescriptor::new(DisplayId::new(1), Size::new(px(48.), px(20.)), |_, cx| {
                    cx.new(|_| gpui::EmptyView)
                })?;
            runtime.mount_surface(surface, cx)
        })
        .unwrap()
    });
    let publisher = publisher.borrow().as_ref().unwrap().clone();

    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 2);
    cx.update(|cx| publisher.fail(std::io::Error::other("source failed"), cx));
    cx.update(|cx| {
        runtime.shutdown(cx);
        runtime.shutdown(cx);
    });

    assert!(matches!(runtime.result(), Some(Err(ShellError::Source(_)))));
    assert_eq!(stops.get(), 1);
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
    drop(binding);
    assert_eq!(stops.get(), 1);
}

#[test]
fn dropping_last_handle_schedules_cleanup_of_owned_surface_only() {
    let cx = TestAppContext::single();
    let host = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui::EmptyView)
        })
        .unwrap()
    });
    let runtime = cx.update(|cx| {
        Shell::attach(cx, |runtime, cx| {
            let descriptor =
                SurfaceDescriptor::new(DisplayId::new(1), Size::new(px(48.), px(20.)), |_, cx| {
                    cx.new(|_| gpui::EmptyView)
                })?;
            runtime.mount_surface(descriptor, cx)
        })
        .unwrap()
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 2);

    drop(runtime);
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
}

#[test]
fn host_quit_cleans_runtime_surface_and_finalizes_success() {
    let cx = TestAppContext::single();
    let runtime = cx.update(|cx| {
        Shell::attach(cx, |runtime, cx| {
            let descriptor =
                SurfaceDescriptor::new(DisplayId::new(1), Size::new(px(48.), px(20.)), |_, cx| {
                    cx.new(|_| gpui::EmptyView)
                })?;
            runtime.mount_surface(descriptor, cx)
        })
        .unwrap()
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);

    cx.quit();

    assert!(matches!(runtime.result(), Some(Ok(()))));
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
}
