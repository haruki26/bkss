use bkss_core::{
    DynamicSurfaceHandle, LayerSurfaceConfig, MountedBinding, Shell, ShellError, Source,
    SourceDescriptor, SourcePublisher, SourceState, SurfaceError,
};
use gpui::{
    App, AppContext, Context, DisplayId, Render, Size, TestAppContext, Window, WindowOptions, div,
    layer_shell::LayerShellOptions, px,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

struct TrackedSource {
    starts: Rc<Cell<usize>>,
    stops: Rc<Cell<usize>>,
}

struct Stop(Rc<Cell<usize>>);

struct FailingSource {
    stops: Rc<Cell<usize>>,
    publisher: Rc<RefCell<Option<SourcePublisher<Self>>>>,
}

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

impl Source for TrackedSource {
    type Key = &'static str;
    type Value = ();
    type Error = std::io::Error;
    type Guard = Stop;

    fn key(&self) -> Self::Key {
        "dynamic surface resource"
    }

    fn start(&self, _: SourcePublisher<Self>, _: &mut App) -> Result<Self::Guard, Self::Error> {
        self.starts.set(self.starts.get() + 1);
        Ok(Stop(Rc::clone(&self.stops)))
    }
}

impl Source for FailingSource {
    type Key = &'static str;
    type Value = ();
    type Error = std::io::Error;
    type Guard = Stop;

    fn key(&self) -> Self::Key {
        "dynamic failure resource"
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        _: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        *self.publisher.borrow_mut() = Some(publisher);
        Ok(Stop(Rc::clone(&self.stops)))
    }
}

struct BoundView {
    _binding: MountedBinding<()>,
    drops: Rc<Cell<usize>>,
}

struct DroppedView(Rc<Cell<usize>>);

impl Render for DroppedView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
    }
}

impl Drop for DroppedView {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

impl Render for BoundView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
    }
}

impl Drop for BoundView {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

fn config() -> LayerSurfaceConfig {
    LayerSurfaceConfig {
        size: Size::new(px(40.), px(20.)),
        options: LayerShellOptions::default(),
    }
}

#[test]
fn dynamic_surface_owns_binding_per_open_and_reopens_with_fresh_instance() {
    let cx = TestAppContext::single();
    let starts = Rc::new(Cell::new(0));
    let stops = Rc::new(Cell::new(0));
    let view_drops = Rc::new(Cell::new(0));
    let instances = Rc::new(RefCell::new(Vec::new()));
    let descriptor = SourceDescriptor::new(TrackedSource {
        starts: Rc::clone(&starts),
        stops: Rc::clone(&stops),
    });

    let (runtime, surface, display) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let weak_runtime = runtime.downgrade();
        let source = descriptor.clone();
        let drops = Rc::clone(&view_drops);
        let factory_instances = Rc::clone(&instances);
        let surface = runtime
            .register_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |instance, _, cx| {
                    assert_eq!(instance.display_id(), cx.displays()[0].id());
                    factory_instances.borrow_mut().push(instance.clone());
                    let runtime = weak_runtime.upgrade().ok_or_else(|| {
                        ShellError::setup(std::io::Error::other(
                            "runtime disappeared during factory",
                        ))
                    })?;
                    let binding = runtime.mount(&source, cx)?;
                    assert_eq!(binding.reactive().get(cx), SourceState::Loading);
                    Ok(cx.new(|_| BoundView {
                        _binding: binding,
                        drops: Rc::clone(&drops),
                    }))
                },
                cx,
            )
            .unwrap();
        (runtime, surface, cx.displays()[0].id())
    });

    cx.update(|cx| {
        surface.open(display, cx).unwrap();
        surface.open(display, cx).unwrap();
    });
    cx.run_until_parked();
    assert_eq!(
        starts.get(),
        1,
        "idempotent open starts one resource activation"
    );
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);

    cx.update(|cx| surface.close(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(stops.get(), 1);
    assert_eq!(view_drops.get(), 1);
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);

    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(starts.get(), 2, "reopen obtains a fresh mounted activation");
    cx.update(|cx| instances.borrow()[0].close(cx).unwrap());
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        1,
        "stale instance cannot close its successor"
    );
    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(stops.get(), 2);
    assert_eq!(view_drops.get(), 2);
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
}

#[test]
fn factory_failure_keeps_original_error_and_cleans_all_dynamic_registrations() {
    let cx = TestAppContext::single();
    let host = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui::EmptyView)
        })
        .unwrap()
    });
    let failed = Rc::new(Cell::new(0));
    let pending = Rc::new(Cell::new(0));
    let failed_surface = Rc::new(RefCell::new(None::<DynamicSurfaceHandle>));
    let failed_factory = Rc::clone(&failed);
    let pending_factory = Rc::clone(&pending);
    let surface_slot = Rc::clone(&failed_surface);
    let runtime = cx.update(|cx| {
        Shell::attach(cx, |runtime, cx| {
            *surface_slot.borrow_mut() =
                Some(runtime.register_dynamic_layer_surface::<gpui::EmptyView>(
                    config(),
                    Duration::from_secs(2),
                    move |_, _, _| {
                        failed_factory.set(failed_factory.get() + 1);
                        Err(ShellError::setup(std::io::Error::other("factory cause")))
                    },
                    cx,
                )?);
            let sibling = runtime.register_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |_, _, cx| {
                    pending_factory.set(pending_factory.get() + 1);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?;
            sibling.open(DisplayId::new(99), cx).unwrap();
            Ok(())
        })
        .unwrap()
    });
    let display = cx.update(|cx| cx.displays()[0].id());
    cx.update(|cx| {
        failed_surface
            .borrow()
            .as_ref()
            .unwrap()
            .open(display, cx)
            .unwrap()
    });
    cx.run_until_parked();
    assert_eq!(failed.get(), 1);
    assert_eq!(pending.get(), 0);
    assert!(matches!(
        runtime.result(),
        Some(Err(ShellError::Setup(error)))
            if error.downcast_ref::<std::io::Error>()
                .is_some_and(|cause| cause.to_string() == "factory cause")
    ));
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
}

#[test]
fn opening_missing_display_past_zero_timeout_is_fatal_and_never_calls_factory() {
    let cx = TestAppContext::single();
    let host = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui::EmptyView)
        })
        .unwrap()
    });
    let calls = Rc::new(Cell::new(0));
    let factory_calls = Rc::clone(&calls);
    let surface_slot = Rc::new(RefCell::new(None::<DynamicSurfaceHandle>));
    let surface_for_attach = Rc::clone(&surface_slot);
    let (runtime, surface) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_dynamic_layer_surface(
                config(),
                Duration::ZERO,
                move |_, _, cx| {
                    factory_calls.set(factory_calls.get() + 1);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?;
            *surface_for_attach.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        let surface = surface_slot.borrow_mut().take().unwrap();
        (runtime, surface)
    });

    cx.update(|cx| surface.open(DisplayId::new(99), cx).unwrap());
    cx.run_until_parked();
    assert_eq!(calls.get(), 0);
    assert!(matches!(
        runtime.result(),
        Some(Err(ShellError::Surface(error)))
            if matches!(error.as_ref(), SurfaceError::DisplayReadinessTimeout { display_id, timeout }
                if *display_id == DisplayId::new(99) && *timeout == Duration::ZERO)
    ));
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
}

#[test]
fn source_failure_closes_static_and_dynamic_windows_cancels_pending_and_stops_once() {
    use bkss_core::{DisplaySelection, LayerSurfaceDeclaration};
    use gpui::layer_shell::LayerShellOptions;

    let cx = TestAppContext::single();
    let host = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui::EmptyView)
        })
        .unwrap()
    });
    let stops = Rc::new(Cell::new(0));
    let publisher = Rc::new(RefCell::new(None));
    let descriptor = SourceDescriptor::new(FailingSource {
        stops: Rc::clone(&stops),
        publisher: Rc::clone(&publisher),
    });
    let pending_calls = Rc::new(Cell::new(0));
    let pending_factory = Rc::clone(&pending_calls);
    let binding_slot = Rc::new(RefCell::new(None));
    let runtime = cx.update(|cx| {
        Shell::attach(cx, |runtime, cx| {
            let binding_slot = Rc::clone(&binding_slot);
            let resource = descriptor.clone();
            let weak_runtime = runtime.downgrade();
            let static_surface = LayerSurfaceDeclaration::new(
                DisplaySelection::Explicit(vec![cx.displays()[0].id()]),
                |_, _| LayerSurfaceConfig {
                    size: Size::new(px(40.), px(20.)),
                    options: LayerShellOptions::default(),
                },
                move |_, _, cx| {
                    let runtime = weak_runtime.upgrade().unwrap();
                    *binding_slot.borrow_mut() = Some(runtime.mount(&resource, cx).unwrap());
                    cx.new(|_| gpui::EmptyView)
                },
            );
            runtime.mount_layer_surface(static_surface, cx)?;

            let weak_runtime = runtime.downgrade();
            let dynamic_resource = descriptor.clone();
            let dynamic_surface = runtime.register_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |_, _, cx| {
                    let runtime = weak_runtime.upgrade().unwrap();
                    let binding = runtime.mount(&dynamic_resource, cx)?;
                    Ok(cx.new(|_| BoundView {
                        _binding: binding,
                        drops: Rc::new(Cell::new(0)),
                    }))
                },
                cx,
            )?;
            dynamic_surface.open(cx.displays()[0].id(), cx).unwrap();

            let pending = runtime.register_dynamic_layer_surface(
                config(),
                Duration::from_secs(30),
                move |_, _, cx| {
                    pending_factory.set(pending_factory.get() + 1);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?;
            pending.open(DisplayId::new(99), cx).unwrap();
            Ok(())
        })
        .unwrap()
    });

    cx.run_until_parked();
    let publisher = publisher.borrow().as_ref().unwrap().clone();
    cx.update(|cx| publisher.fail(std::io::Error::other("subscription failed"), cx));
    cx.run_until_parked();
    assert!(matches!(runtime.result(), Some(Err(ShellError::Source(_)))));
    assert_eq!(stops.get(), 1);
    assert_eq!(pending_calls.get(), 0);
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(stops.get(), 1);
    drop(binding_slot);
    assert_eq!(stops.get(), 1);
}

#[test]
fn closing_pending_request_prevents_later_timeout_failure() {
    let cx = TestAppContext::single();
    let calls = Rc::new(Cell::new(0));
    let factory_calls = Rc::clone(&calls);
    let (runtime, surface) = cx.update(|cx| {
        let surface = Rc::new(RefCell::new(None::<DynamicSurfaceHandle>));
        let surface_slot = Rc::clone(&surface);
        let runtime = Shell::attach(cx, |runtime, cx| {
            let invalid = runtime.register_dynamic_layer_surface(
                LayerSurfaceConfig {
                    size: Size::new(px(0.), px(20.)),
                    options: LayerShellOptions::default(),
                },
                Duration::from_secs(1),
                |_, _, cx| Ok(cx.new(|_| gpui::EmptyView)),
                cx,
            );
            assert!(matches!(invalid, Err(ShellError::Setup(_))));

            let stretched = LayerSurfaceConfig {
                size: Size::new(px(0.), px(0.)),
                options: gpui::layer_shell::LayerShellOptions {
                    anchor: gpui::layer_shell::Anchor::LEFT
                        | gpui::layer_shell::Anchor::RIGHT
                        | gpui::layer_shell::Anchor::TOP
                        | gpui::layer_shell::Anchor::BOTTOM,
                    ..Default::default()
                },
            };
            assert!(
                runtime
                    .register_dynamic_layer_surface(
                        stretched,
                        Duration::from_secs(1),
                        |_, _, cx| Ok(cx.new(|_| gpui::EmptyView)),
                        cx,
                    )
                    .is_ok()
            );

            *surface_slot.borrow_mut() = Some(runtime.register_dynamic_layer_surface(
                config(),
                Duration::from_secs(30),
                move |_, _, cx| {
                    factory_calls.set(factory_calls.get() + 1);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?);
            Ok(())
        })
        .unwrap();
        let surface = surface.borrow_mut().take().unwrap();
        (runtime, surface)
    });
    let missing = DisplayId::new(99);
    cx.update(|cx| {
        surface.open(missing, cx).unwrap();
        surface.close(missing, cx).unwrap();
    });
    cx.executor().advance_clock(Duration::from_secs(31));
    cx.run_until_parked();
    assert_eq!(calls.get(), 0);
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    assert!(runtime.result().is_none());
}

#[test]
fn native_window_close_allows_reopen_and_old_instance_cannot_close_new_window() {
    let cx = TestAppContext::single();
    let calls = Rc::new(Cell::new(0));
    let factory_calls = Rc::clone(&calls);
    let instances = Rc::new(RefCell::new(Vec::new()));
    let factory_instances = Rc::clone(&instances);
    let view_drops = Rc::new(Cell::new(0));
    let factory_drops = Rc::clone(&view_drops);
    let surface_slot = Rc::new(RefCell::new(None::<DynamicSurfaceHandle>));
    let surface_for_attach = Rc::clone(&surface_slot);
    let (runtime, surface, display) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |instance, _, cx| {
                    factory_calls.set(factory_calls.get() + 1);
                    factory_instances.borrow_mut().push(instance);
                    Ok(cx.new(|_| DroppedView(Rc::clone(&factory_drops))))
                },
                cx,
            )?;
            *surface_for_attach.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        let surface = surface_slot.borrow_mut().take().unwrap();
        (runtime, surface, cx.displays()[0].id())
    });

    cx.update(|cx| {
        surface.open(display, cx).unwrap();
        surface.open(display, cx).unwrap();
    });
    cx.run_until_parked();
    assert_eq!(calls.get(), 1, "repeated open is idempotent");
    cx.update(|cx| {
        let window = *cx.windows().last().unwrap();
        cx.update_window(window, |_, window, _| window.remove_window())
            .unwrap();
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);

    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(calls.get(), 2);
    assert_eq!(
        view_drops.get(),
        1,
        "the cancelled generation's returned view is dropped"
    );
    cx.update(|cx| instances.borrow()[0].close(cx).unwrap());
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        1,
        "the natively closed instance cannot close its successor"
    );
    cx.update(|cx| surface.close(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    assert_eq!(view_drops.get(), 2, "explicit close drops the mounted view");

    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(calls.get(), 3, "close permits a fresh generation");
    cx.update(|cx| instances.borrow()[1].close(cx).unwrap());
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        1,
        "stale instance cannot close its successor"
    );
    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(view_drops.get(), 3);
}

#[test]
fn dropping_runtime_cleans_dynamic_surface_even_while_external_handle_remains() {
    let cx = TestAppContext::single();
    let host = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui::EmptyView)
        })
        .unwrap()
    });
    let starts = Rc::new(Cell::new(0));
    let stops = Rc::new(Cell::new(0));
    let drops = Rc::new(Cell::new(0));
    let descriptor = SourceDescriptor::new(TrackedSource {
        starts: Rc::clone(&starts),
        stops: Rc::clone(&stops),
    });
    let (runtime, surface, display, weak) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let weak = runtime.downgrade();
        let factory_runtime = weak.clone();
        let source = descriptor.clone();
        let view_drops = Rc::clone(&drops);
        let surface = runtime
            .register_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |_, _, cx| {
                    let runtime = factory_runtime.upgrade().ok_or_else(|| {
                        ShellError::setup(std::io::Error::other("runtime closed"))
                    })?;
                    let binding = runtime.mount(&source, cx)?;
                    Ok(cx.new(|_| BoundView {
                        _binding: binding,
                        drops: Rc::clone(&view_drops),
                    }))
                },
                cx,
            )
            .unwrap();
        (runtime, surface, cx.displays()[0].id(), weak)
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(starts.get(), 1);
    drop(runtime);
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
    assert_eq!(stops.get(), 1);
    assert_eq!(drops.get(), 1);
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
    drop(surface);
    assert_eq!(stops.get(), 1);
}

#[test]
fn factory_close_reopen_does_not_let_old_generation_replace_or_close_new_one() {
    let cx = TestAppContext::single();
    let calls = Rc::new(Cell::new(0));
    let factory_calls = Rc::clone(&calls);
    let instances = Rc::new(RefCell::new(Vec::new()));
    let factory_instances = Rc::clone(&instances);
    let view_drops = Rc::new(Cell::new(0));
    let factory_drops = Rc::clone(&view_drops);
    let surface_slot = Rc::new(RefCell::new(None::<DynamicSurfaceHandle>));
    let surface_for_factory = Rc::clone(&surface_slot);
    let (runtime, surface, display) = cx.update(|cx| {
        let display = cx.displays()[0].id();
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |instance, _, cx| {
                    let call = factory_calls.get() + 1;
                    factory_calls.set(call);
                    factory_instances.borrow_mut().push(instance);
                    if call == 1 {
                        let surface = surface_for_factory.borrow().as_ref().unwrap().clone();
                        surface.close(display, cx).unwrap();
                        surface.open(display, cx).unwrap();
                    }
                    Ok(cx.new(|_| DroppedView(Rc::clone(&factory_drops))))
                },
                cx,
            )?;
            *surface_slot.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        let surface = surface_slot.borrow().as_ref().unwrap().clone();
        (runtime, surface, display)
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(calls.get(), 2);
    assert_eq!(
        view_drops.get(),
        1,
        "cancelled generation's returned view is dropped"
    );
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    cx.update(|cx| instances.borrow()[0].close(cx).unwrap());
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(view_drops.get(), 2);
}

#[test]
fn reentrant_shutdown_from_factory_drops_late_content_and_cancels_sibling() {
    let cx = TestAppContext::single();
    let host = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui::EmptyView)
        })
        .unwrap()
    });
    let starts = Rc::new(Cell::new(0));
    let stops = Rc::new(Cell::new(0));
    let drops = Rc::new(Cell::new(0));
    let sibling_calls = Rc::new(Cell::new(0));
    let sibling_factory = Rc::clone(&sibling_calls);
    let surface_slot = Rc::new(RefCell::new(None::<DynamicSurfaceHandle>));
    let surface_for_attach = Rc::clone(&surface_slot);
    let descriptor = SourceDescriptor::new(TrackedSource {
        starts: Rc::clone(&starts),
        stops: Rc::clone(&stops),
    });
    let (runtime, surface) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |runtime, cx| {
            let weak_runtime = runtime.downgrade();
            let source = descriptor.clone();
            let view_drops = Rc::clone(&drops);
            let primary = runtime.register_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |_, _, cx| {
                    let runtime = weak_runtime.upgrade().unwrap();
                    let binding = runtime.mount(&source, cx)?;
                    runtime.shutdown(cx);
                    Ok(cx.new(|_| BoundView {
                        _binding: binding,
                        drops: Rc::clone(&view_drops),
                    }))
                },
                cx,
            )?;
            let sibling = runtime.register_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |_, _, cx| {
                    sibling_factory.set(sibling_factory.get() + 1);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?;
            primary.open(cx.displays()[0].id(), cx).unwrap();
            sibling.open(cx.displays()[0].id(), cx).unwrap();
            *surface_for_attach.borrow_mut() = Some(primary);
            Ok(())
        })
        .unwrap();
        let surface = surface_slot.borrow_mut().take().unwrap();
        (runtime, surface)
    });
    cx.run_until_parked();
    assert!(runtime.result().is_none());
    assert_eq!(starts.get(), 1);
    assert_eq!(stops.get(), 1);
    assert_eq!(drops.get(), 1);
    assert_eq!(sibling_calls.get(), 0);
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(stops.get(), 1);
    drop(surface);
}
