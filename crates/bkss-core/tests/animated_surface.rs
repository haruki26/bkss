use bkss_core::{
    AnimationPlan, Channel, DynamicSurfaceHandle, EasingCurve, LayerSurfaceConfig, MountedBinding,
    PlanTiming, PropertyAnimation, Shell, ShellError, Source, SourceDescriptor, SourcePublisher,
    SurfaceInstanceHandle,
};
use gpui::{
    App, AppContext, Context, Render, Size, TestAppContext, Window, div,
    layer_shell::LayerShellOptions, px,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

struct LeasedSource {
    starts: Rc<Cell<usize>>,
    stops: Rc<Cell<usize>>,
}

struct LeaseDrop(Rc<Cell<usize>>);

impl Drop for LeaseDrop {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

impl Source for LeasedSource {
    type Key = &'static str;
    type Value = ();
    type Error = std::io::Error;
    type Guard = LeaseDrop;

    fn key(&self) -> Self::Key {
        "animated surface lease"
    }

    fn start(&self, _: SourcePublisher<Self>, _: &mut App) -> Result<Self::Guard, Self::Error> {
        self.starts.set(self.starts.get() + 1);
        Ok(LeaseDrop(Rc::clone(&self.stops)))
    }
}

struct LeasedView {
    _binding: MountedBinding<()>,
    drops: Rc<Cell<usize>>,
}

impl Render for LeasedView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
    }
}

impl Drop for LeasedView {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
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

fn config() -> LayerSurfaceConfig {
    LayerSurfaceConfig {
        size: Size::new(px(40.), px(20.)),
        options: LayerShellOptions::default(),
    }
}

fn plan() -> AnimationPlan {
    let timing = Some(PlanTiming::Easing {
        duration: Duration::from_millis(100),
        curve: EasingCurve::Linear,
    });
    AnimationPlan {
        opacity: Some(Channel {
            enter: Some(PropertyAnimation {
                endpoints: [0.0, 1.0],
                timing,
            }),
            exit: Some(PropertyAnimation {
                endpoints: [1.0, 0.0],
                timing,
            }),
        }),
        ..AnimationPlan::default()
    }
}

fn deferred_plan() -> AnimationPlan {
    let timing = Some(PlanTiming::Easing {
        duration: Duration::from_millis(100),
        curve: EasingCurve::Linear,
    });
    AnimationPlan {
        opacity: Some(Channel {
            enter: Some(PropertyAnimation {
                endpoints: [0.0, 1.0],
                timing,
            }),
            exit: Some(PropertyAnimation {
                endpoints: [1.0, 0.0],
                timing,
            }),
        }),
        translation_x: Some(Channel {
            enter: Some(PropertyAnimation {
                endpoints: [-40.0, 0.0],
                timing,
            }),
            exit: Some(PropertyAnimation {
                endpoints: [0.0, -40.0],
                timing,
            }),
        }),
        ..AnimationPlan::default()
    }
}

fn simulate_frame(cx: &mut TestAppContext) -> usize {
    let window = cx.update(|cx| *cx.windows().last().unwrap());
    cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx))
        .unwrap()
}

#[test]
fn exit_retains_lease_and_reopen_retargets_the_same_generation() {
    let mut cx = TestAppContext::single();
    let starts = Rc::new(Cell::new(0));
    let stops = Rc::new(Cell::new(0));
    let drops = Rc::new(Cell::new(0));
    let calls = Rc::new(Cell::new(0));
    let instance_slot = Rc::new(RefCell::new(None::<SurfaceInstanceHandle>));
    let surface_slot = Rc::new(RefCell::new(None::<DynamicSurfaceHandle>));
    let surface_for_setup = Rc::clone(&surface_slot);
    let (runtime, surface, display) = cx.update(|cx| {
        let display = cx.displays()[0].id();
        let runtime = Shell::attach(cx, |runtime, cx| {
            let source = SourceDescriptor::new(LeasedSource {
                starts: Rc::clone(&starts),
                stops: Rc::clone(&stops),
            });
            let weak_runtime = runtime.downgrade();
            let factory_instance = Rc::clone(&instance_slot);
            let factory_drops = Rc::clone(&drops);
            let factory_calls = Rc::clone(&calls);
            let surface = runtime.register_animated_dynamic_layer_surface(
                config(),
                Duration::from_secs(5),
                plan(),
                move |instance, _, cx| {
                    factory_calls.set(factory_calls.get() + 1);
                    assert_eq!(instance.animation_sample().opacity, Some(0.0));
                    *factory_instance.borrow_mut() = Some(instance);
                    let runtime = weak_runtime.upgrade().ok_or_else(|| {
                        ShellError::setup(std::io::Error::other("runtime closed"))
                    })?;
                    let binding = runtime.mount(&source, cx)?;
                    Ok(cx.new(|_| LeasedView {
                        _binding: binding,
                        drops: Rc::clone(&factory_drops),
                    }))
                },
                cx,
            )?;
            *surface_for_setup.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        let surface = surface_slot.borrow_mut().take().unwrap();
        (runtime, surface, display)
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(calls.get(), 1);
    assert_eq!(starts.get(), 1);
    assert_eq!(stops.get(), 0);
    let instance = instance_slot.borrow().as_ref().unwrap().clone();
    assert_eq!(instance.animation_sample().opacity, Some(0.0));

    cx.executor().advance_clock(Duration::from_millis(40));
    assert_eq!(
        instance.animation_sample().opacity,
        Some(0.0),
        "reading a sample does not advance the runner"
    );
    assert!(simulate_frame(&mut cx) > 0);
    let before_close = instance.animation_sample().opacity.unwrap();
    assert!((before_close - 0.4).abs() < 0.001);

    cx.update(|cx| surface.close(display, cx).unwrap());
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert_eq!(drops.get(), 0);
    assert_eq!(stops.get(), 0);
    let exit_sample = instance.animation_sample();
    assert!(exit_sample.running);
    assert!((exit_sample.opacity.unwrap() - before_close).abs() < 0.001);

    cx.update(|cx| surface.open(display, cx).unwrap());
    let reopened = instance.animation_sample();
    assert!(reopened.running);
    assert!((reopened.opacity.unwrap() - before_close).abs() < 0.001);
    cx.executor().advance_clock(Duration::from_millis(100));
    assert!(simulate_frame(&mut cx) > 0);
    assert_eq!(instance.animation_sample().opacity, Some(1.0));
    assert_eq!(calls.get(), 1, "reopen retains the mounted generation");
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);

    cx.update(|cx| surface.close(display, cx).unwrap());
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    cx.executor().advance_clock(Duration::from_millis(100));
    assert!(simulate_frame(&mut cx) > 0);
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    assert_eq!(drops.get(), 1);
    assert_eq!(stops.get(), 1);
    assert!(!instance.animation_sample().running);
    cx.update(|cx| runtime.shutdown(cx));
}

#[test]
fn invalid_plan_has_no_registration_side_effects_and_pending_close_cancels_readiness() {
    let cx = TestAppContext::single();
    let runtime = cx.update(|cx| Shell::attach(cx, |_, _| Ok(()))).unwrap();
    let mut invalid = plan();
    invalid
        .opacity
        .as_mut()
        .unwrap()
        .enter
        .as_mut()
        .unwrap()
        .endpoints[1] = 2.0;
    let calls = Rc::new(Cell::new(0));
    let factory_calls = Rc::clone(&calls);
    let pending_factory_calls = Rc::clone(&calls);
    let error = cx.update(|cx| {
        runtime.register_animated_dynamic_layer_surface(
            config(),
            Duration::from_secs(2),
            invalid,
            move |_, _, cx| {
                factory_calls.set(factory_calls.get() + 1);
                Ok(cx.new(|_| gpui::EmptyView))
            },
            cx,
        )
    });
    assert!(matches!(error, Err(ShellError::Setup(_))));
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    assert_eq!(calls.get(), 0);

    let surface = cx.update(|cx| {
        runtime
            .register_animated_dynamic_layer_surface(
                config(),
                Duration::from_secs(30),
                plan(),
                move |_, _, cx| {
                    pending_factory_calls.set(pending_factory_calls.get() + 1);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )
            .unwrap()
    });
    let missing = gpui::DisplayId::new(999);
    cx.update(|cx| {
        surface.open(missing, cx).unwrap();
        surface.close(missing, cx).unwrap();
    });
    cx.executor().advance_clock(Duration::from_secs(31));
    cx.run_until_parked();
    assert_eq!(calls.get(), 0);
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    assert!(runtime.result().is_none());
    cx.update(|cx| runtime.shutdown(cx));
}

#[test]
fn shutdown_cancels_exit_animation_and_removes_surface_immediately() {
    let mut cx = TestAppContext::single();
    let instance_slot = Rc::new(RefCell::new(None::<SurfaceInstanceHandle>));
    let surface_slot = Rc::new(RefCell::new(None));
    let factory_instance = Rc::clone(&instance_slot);
    let setup_surface = Rc::clone(&surface_slot);
    let (runtime, surface, display) = cx.update(|cx| {
        let display = cx.displays()[0].id();
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_animated_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                plan(),
                move |instance, _, cx| {
                    *factory_instance.borrow_mut() = Some(instance);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?;
            *setup_surface.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        (runtime, surface_slot.borrow_mut().take().unwrap(), display)
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    let instance = instance_slot.borrow().as_ref().unwrap().clone();
    cx.executor().advance_clock(Duration::from_millis(20));
    assert_eq!(simulate_frame(&mut cx), 1);
    cx.update(|cx| surface.close(display, cx).unwrap());
    assert!(instance.animation_sample().running);
    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    cx.executor().advance_clock(Duration::from_secs(1));
    assert!(!instance.animation_sample().running);
}

#[test]
fn reentrant_factory_close_reopen_cancels_old_animated_generation() {
    let cx = TestAppContext::single();
    let surface_slot = Rc::new(RefCell::new(None::<DynamicSurfaceHandle>));
    let instances = Rc::new(RefCell::new(Vec::new()));
    let calls = Rc::new(Cell::new(0));
    let drops = Rc::new(Cell::new(0));
    let (runtime, surface, display) = cx.update(|cx| {
        let display = cx.displays()[0].id();
        let setup_surface = Rc::clone(&surface_slot);
        let factory_surface = Rc::clone(&surface_slot);
        let factory_instances = Rc::clone(&instances);
        let factory_calls = Rc::clone(&calls);
        let factory_drops = Rc::clone(&drops);
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_animated_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                plan(),
                move |instance, _, cx| {
                    factory_instances.borrow_mut().push(instance);
                    let call = factory_calls.get() + 1;
                    factory_calls.set(call);
                    if call == 1 {
                        let surface = factory_surface.borrow().as_ref().unwrap().clone();
                        surface.close(display, cx).unwrap();
                        surface.open(display, cx).unwrap();
                    }
                    Ok(cx.new(|_| DroppedView(Rc::clone(&factory_drops))))
                },
                cx,
            )?;
            *setup_surface.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        (
            runtime,
            surface_slot.borrow().as_ref().unwrap().clone(),
            display,
        )
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(calls.get(), 2);
    assert_eq!(instances.borrow().len(), 2);
    assert_eq!(instances.borrow()[0].animation_sample().opacity, None);
    assert_eq!(instances.borrow()[1].animation_sample().opacity, Some(0.0));
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert_eq!(drops.get(), 1);
    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(drops.get(), 2);
}

#[test]
fn native_window_close_discards_animation_and_stale_instance_cannot_close_reopen() {
    let mut cx = TestAppContext::single();
    let instances = Rc::new(RefCell::new(Vec::new()));
    let surface_slot = Rc::new(RefCell::new(None::<DynamicSurfaceHandle>));
    let setup_slot = Rc::clone(&surface_slot);
    let (runtime, surface, display) = cx.update(|cx| {
        let display = cx.displays()[0].id();
        let factory_instances = Rc::clone(&instances);
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_animated_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                plan(),
                move |instance, _, cx| {
                    factory_instances.borrow_mut().push(instance);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?;
            *setup_slot.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        (runtime, surface_slot.borrow_mut().take().unwrap(), display)
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(instances.borrow().len(), 1);
    let first = instances.borrow()[0].clone();
    let old_window = cx.update(|cx| *cx.windows().last().unwrap());
    cx.update_window(old_window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    assert_eq!(first.animation_sample().opacity, None);

    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(instances.borrow().len(), 2);
    assert_eq!(instances.borrow()[1].animation_sample().opacity, Some(0.0));
    cx.update(|cx| first.close(cx).unwrap());
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
}

#[test]
fn dropping_last_runtime_handle_cancels_queued_animation_frame() {
    let cx = TestAppContext::single();
    let instance_slot = Rc::new(RefCell::new(None::<SurfaceInstanceHandle>));
    let surface_slot = Rc::new(RefCell::new(None::<DynamicSurfaceHandle>));
    let setup_surface = Rc::clone(&surface_slot);
    let factory_instance = Rc::clone(&instance_slot);
    let (runtime, surface, display) = cx.update(|cx| {
        let display = cx.displays()[0].id();
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_animated_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                plan(),
                move |instance, _, cx| {
                    *factory_instance.borrow_mut() = Some(instance);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?;
            *setup_surface.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        (runtime, surface_slot.borrow_mut().take().unwrap(), display)
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    let instance = instance_slot.borrow().as_ref().unwrap().clone();
    assert!(instance.animation_sample().running);
    drop(runtime);
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    assert_eq!(instance.animation_sample().opacity, None);
    cx.update(|cx| surface.close(display, cx).unwrap());
}

#[test]
fn animated_factory_failure_remains_primary_and_cleans_runtime() {
    let cx = TestAppContext::single();
    let instance_slot = Rc::new(RefCell::new(None::<SurfaceInstanceHandle>));
    let captured = Rc::clone(&instance_slot);
    let surface_slot = Rc::new(RefCell::new(None::<DynamicSurfaceHandle>));
    let setup_surface = Rc::clone(&surface_slot);
    let (runtime, surface, display) = cx.update(|cx| {
        let display = cx.displays()[0].id();
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_animated_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                plan(),
                move |instance, _, _| {
                    *captured.borrow_mut() = Some(instance);
                    Err::<gpui::Entity<gpui::EmptyView>, _>(ShellError::setup(
                        std::io::Error::other("animated factory failed"),
                    ))
                },
                cx,
            )?;
            *setup_surface.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        (runtime, surface_slot.borrow_mut().take().unwrap(), display)
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    assert!(matches!(runtime.result(), Some(Err(ShellError::Setup(_)))));
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    assert_eq!(
        instance_slot
            .borrow()
            .as_ref()
            .unwrap()
            .animation_sample()
            .opacity,
        None
    );
}

#[test]
fn deferred_animation_starts_at_first_initialization_and_is_idempotent() {
    let mut cx = TestAppContext::single();
    let instance_slot = Rc::new(RefCell::new(None::<SurfaceInstanceHandle>));
    let factory_instance = Rc::clone(&instance_slot);
    let surface_slot = Rc::new(RefCell::new(None));
    let setup_surface = Rc::clone(&surface_slot);
    let (runtime, surface, display) = cx.update(|cx| {
        let display = cx.displays()[0].id();
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_deferred_animated_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |instance, _, cx| {
                    *factory_instance.borrow_mut() = Some(instance);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?;
            *setup_surface.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        (runtime, surface_slot.borrow_mut().take().unwrap(), display)
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    let instance = instance_slot.borrow().as_ref().unwrap().clone();

    cx.executor().advance_clock(Duration::from_secs(1));
    let initial = cx
        .update(|cx| instance.initialize_animation(deferred_plan(), cx))
        .unwrap();
    assert_eq!(initial.opacity, Some(0.0));
    assert_eq!(initial.translation_x, Some(-40.0));
    assert!(initial.running);
    assert_eq!(
        cx.update(|cx| instance.initialize_animation(AnimationPlan::default(), cx))
            .unwrap(),
        initial
    );

    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(50));
    assert!(simulate_frame(&mut cx) > 0);
    let sample = instance.animation_sample();
    assert!((sample.opacity.unwrap() - 0.5).abs() < 0.001);
    assert!((sample.translation_x.unwrap() + 20.0).abs() < 0.001);
    cx.update(|cx| surface.close(display, cx).unwrap());
    assert!(instance.animation_sample().running);
    cx.update(|cx| surface.open(display, cx).unwrap());
    assert!(instance.animation_sample().running);
    assert_eq!(
        instance.animation_sample().translation_x,
        sample.translation_x,
        "reopen during exit retains the initialized generation"
    );
    cx.update(|cx| runtime.shutdown(cx));
}

#[test]
fn deferred_animation_initialized_while_factory_is_pending_survives_mount() {
    let mut cx = TestAppContext::single();
    let initial_slot = Rc::new(RefCell::new(None));
    let factory_initial = Rc::clone(&initial_slot);
    let instance_slot = Rc::new(RefCell::new(None::<SurfaceInstanceHandle>));
    let factory_instance = Rc::clone(&instance_slot);
    let surface_slot = Rc::new(RefCell::new(None));
    let setup_surface = Rc::clone(&surface_slot);
    let (runtime, surface, display) = cx.update(|cx| {
        let display = cx.displays()[0].id();
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_deferred_animated_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |instance, _, cx| {
                    *factory_instance.borrow_mut() = Some(instance.clone());
                    *factory_initial.borrow_mut() =
                        Some(instance.initialize_animation(deferred_plan(), cx)?);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?;
            *setup_surface.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        (runtime, surface_slot.borrow_mut().take().unwrap(), display)
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(initial_slot.borrow().as_ref().unwrap().opacity, Some(0.0));
    assert_eq!(
        initial_slot.borrow().as_ref().unwrap().translation_x,
        Some(-40.0)
    );
    let instance = instance_slot.borrow().as_ref().unwrap().clone();
    cx.executor().advance_clock(Duration::from_millis(50));
    assert!(simulate_frame(&mut cx) > 0);
    let sample = instance.animation_sample();
    assert!((sample.opacity.unwrap() - 0.5).abs() < 0.001);
    assert!((sample.translation_x.unwrap() + 20.0).abs() < 0.001);
    cx.update(|cx| runtime.shutdown(cx));
}

#[test]
fn close_before_deferred_initialization_rejects_stale_handle_and_fresh_generation_arms() {
    let cx = TestAppContext::single();
    let instances = Rc::new(RefCell::new(Vec::new()));
    let factory_instances = Rc::clone(&instances);
    let surface_slot = Rc::new(RefCell::new(None));
    let setup_surface = Rc::clone(&surface_slot);
    let (runtime, surface, display) = cx.update(|cx| {
        let display = cx.displays()[0].id();
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_deferred_animated_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |instance, _, cx| {
                    factory_instances.borrow_mut().push(instance);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?;
            *setup_surface.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        (runtime, surface_slot.borrow_mut().take().unwrap(), display)
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    let old = instances.borrow()[0].clone();
    cx.update(|cx| surface.close(display, cx).unwrap());
    cx.run_until_parked();
    let invalid = AnimationPlan {
        opacity: Some(Channel {
            enter: Some(PropertyAnimation {
                endpoints: [0.0, f32::NAN],
                timing: None,
            }),
            exit: None,
        }),
        ..AnimationPlan::default()
    };
    assert!(
        cx.update(|cx| old.initialize_animation(invalid, cx))
            .is_err()
    );
    assert!(runtime.result().is_none(), "stale initialization is inert");

    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    let current = instances.borrow()[1].clone();
    let initial = cx
        .update(|cx| current.initialize_animation(deferred_plan(), cx))
        .unwrap();
    assert_eq!(initial.opacity, Some(0.0));
    assert_eq!(initial.translation_x, Some(-40.0));
    cx.update(|cx| runtime.shutdown(cx));
}

#[test]
fn invalid_deferred_plan_preserves_runner_error_and_stops_runtime() {
    let cx = TestAppContext::single();
    let instance_slot = Rc::new(RefCell::new(None::<SurfaceInstanceHandle>));
    let factory_instance = Rc::clone(&instance_slot);
    let surface_slot = Rc::new(RefCell::new(None));
    let setup_surface = Rc::clone(&surface_slot);
    let (runtime, surface, display) = cx.update(|cx| {
        let display = cx.displays()[0].id();
        let runtime = Shell::attach(cx, |runtime, cx| {
            let surface = runtime.register_deferred_animated_dynamic_layer_surface(
                config(),
                Duration::from_secs(2),
                move |instance, _, cx| {
                    *factory_instance.borrow_mut() = Some(instance);
                    Ok(cx.new(|_| gpui::EmptyView))
                },
                cx,
            )?;
            *setup_surface.borrow_mut() = Some(surface);
            Ok(())
        })
        .unwrap();
        (runtime, surface_slot.borrow_mut().take().unwrap(), display)
    });
    cx.update(|cx| surface.open(display, cx).unwrap());
    cx.run_until_parked();
    let instance = instance_slot.borrow().as_ref().unwrap().clone();
    let invalid = AnimationPlan {
        translation_x: Some(Channel {
            enter: Some(PropertyAnimation {
                endpoints: [0.0, f32::NAN],
                timing: None,
            }),
            exit: None,
        }),
        ..AnimationPlan::default()
    };
    let error = cx
        .update(|cx| instance.initialize_animation(invalid, cx))
        .unwrap_err();
    assert!(matches!(
        error,
        ShellError::Setup(error)
            if error.downcast_ref::<bkss_core::RunnerError>()
                == Some(&bkss_core::RunnerError::InvalidPlan)
    ));
    assert!(matches!(
        runtime.result(),
        Some(Err(ShellError::Setup(error)))
            if error.downcast_ref::<bkss_core::RunnerError>()
                == Some(&bkss_core::RunnerError::InvalidPlan)
    ));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
}
