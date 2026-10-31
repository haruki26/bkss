use super::runtime::{
    RuntimeInner, SurfaceRegistrationError, record_setup_failure, record_surface_failure,
    request_host_quit, stop_runtime,
};
use crate::{
    ShellError,
    surface::{
        DISPLAY_POLL_INTERVAL, DisplayReadiness, LayerSurfaceConfig, LayerSurfaceDeclaration,
        LayerSurfaceError, Readiness, SURFACE_NAMESPACE, StartupTargetError, StartupTargets,
        SurfaceDescriptor, SurfaceError,
    },
};
use gpui::{AnyWindowHandle, App, Bounds, Render, Task, layer_shell::LayerShellOptions, px};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub(super) struct SurfaceWork {
    task: RefCell<Option<Task<()>>>,
    active: Cell<bool>,
    cancel: RefCell<Option<Box<dyn FnOnce()>>>,
}

struct LayerFactory<V: Render + 'static> {
    content: crate::surface::LayerContentFactory<V>,
}

pub(super) fn mount_surface<V: Render + 'static>(
    inner: &Rc<RuntimeInner>,
    descriptor: SurfaceDescriptor<V>,
    cx: &mut App,
) -> Result<(), ShellError> {
    if inner.stopped.get() {
        return Err(ShellError::setup(SurfaceRegistrationError::Closed));
    }
    let weak = Rc::downgrade(inner);
    let display_id = descriptor.display_id;
    let size = descriptor.size;
    let timeout = descriptor.readiness_timeout;
    let content = Rc::new(RefCell::new(Some(descriptor.content)));
    let work = Rc::new(SurfaceWork {
        task: RefCell::new(None),
        active: Cell::new(false),
        cancel: RefCell::new(None),
    });
    let cancel_content = Rc::clone(&content);
    *work.cancel.borrow_mut() = Some(Box::new(move || {
        let content = cancel_content.borrow_mut().take();
        drop(content);
    }));
    let task_content = Rc::clone(&content);
    let weak_work = Rc::downgrade(&work);
    let started = cx.background_executor().now();
    let task = cx.spawn(async move |cx| {
        let mut readiness = DisplayReadiness::new(timeout, started);
        loop {
            let Some(inner) = weak.upgrade() else { break };
            let Some(work) = weak_work.upgrade() else {
                break;
            };
            work.active.set(true);
            if inner.stopped.get() {
                work.active.set(false);
                break;
            }
            let found = cx.update(|cx| {
                cx.displays()
                    .iter()
                    .any(|display| display.id() == display_id)
            });
            match readiness.poll(found, cx.background_executor().now()) {
                Readiness::Waiting => {
                    work.active.set(false);
                    drop(inner);
                    drop(work);
                    cx.background_executor().timer(DISPLAY_POLL_INTERVAL).await;
                }
                Readiness::TimedOut => {
                    work.active.set(false);
                    let cancel = work.cancel.borrow_mut().take();
                    if let Some(cancel) = cancel {
                        cancel();
                    }
                    cx.update(|app| {
                        record_surface_failure(
                            &inner,
                            SurfaceError::DisplayReadinessTimeout {
                                display_id,
                                timeout,
                            },
                            app,
                        )
                    });
                    break;
                }
                Readiness::Ready => {
                    if inner.stopped.get() {
                        work.active.set(false);
                        break;
                    }
                    let content = task_content.borrow_mut().take();
                    let Some(content) = content else {
                        work.active.set(false);
                        break;
                    };
                    let opened = cx
                        .open_window(
                            super::surface_window::layer_window_options(
                                display_id,
                                size,
                                LayerShellOptions {
                                    namespace: SURFACE_NAMESPACE.to_owned(),
                                    exclusive_zone: Some(px(0.)),
                                    keyboard_interactivity:
                                        gpui::layer_shell::KeyboardInteractivity::None,
                                    ..Default::default()
                                },
                            ),
                            |window, cx| {
                                window.set_input_region(Some(&[]));
                                content(window, cx)
                            },
                        )
                        .map(Into::into)
                        .map_err(|cause| cause.into_boxed_dyn_error());
                    let cancel = work.cancel.borrow_mut().take();
                    if let Some(cancel) = cancel {
                        cancel();
                    }
                    cx.update(|app| finish_surface_open(&inner, display_id, opened, app));
                    work.active.set(false);
                    break;
                }
                Readiness::Finished => {
                    work.active.set(false);
                    break;
                }
            }
        }
    });
    *work.task.borrow_mut() = Some(task);
    inner.surface_works.borrow_mut().push(work);
    Ok(())
}

pub(super) fn mount_layer_surface<V: Render + 'static>(
    inner: &Rc<RuntimeInner>,
    declaration: LayerSurfaceDeclaration<V>,
    cx: &mut App,
) -> Result<(), ShellError> {
    if inner.stopped.get() {
        return Err(ShellError::setup(SurfaceRegistrationError::Closed));
    }

    let started = cx.background_executor().now();
    let snapshot = cx
        .displays()
        .iter()
        .map(|display| (display.id(), display.bounds()))
        .collect::<Vec<_>>();
    let mut targets = StartupTargets::new(
        declaration.displays,
        snapshot,
        started,
        declaration.readiness_timeout,
    )
    .map_err(ShellError::setup)?;
    let factory = Rc::new(RefCell::new(Some(LayerFactory {
        content: declaration.content,
    })));
    let configure: Rc<dyn Fn(gpui::DisplayId, Bounds<gpui::Pixels>) -> LayerSurfaceConfig> =
        Rc::from(declaration.configure);
    let work = Rc::new(SurfaceWork {
        task: RefCell::new(None),
        active: Cell::new(false),
        cancel: RefCell::new(None),
    });
    let cancel_factory = Rc::clone(&factory);
    *work.cancel.borrow_mut() = Some(Box::new(move || {
        let factory = cancel_factory.borrow_mut().take();
        drop(factory);
    }));

    let weak = Rc::downgrade(inner);
    let weak_work = Rc::downgrade(&work);
    let task_factory = Rc::clone(&factory);
    let task_configure = Rc::clone(&configure);
    let task = cx.spawn(async move |cx| {
        loop {
            let Some(inner) = weak.upgrade() else { break };
            let Some(work) = weak_work.upgrade() else {
                break;
            };
            if inner.stopped.get() {
                break;
            }
            work.active.set(true);
            let snapshot = cx.update(|app| {
                app.displays()
                    .iter()
                    .map(|display| (display.id(), display.bounds()))
                    .collect::<Vec<_>>()
            });
            let ready = match targets.poll(&snapshot, cx.background_executor().now()) {
                Ok(ready) => ready,
                Err(StartupTargetError::EnumerationTimeout { timeout }) => {
                    work.active.set(false);
                    let error =
                        ShellError::setup(LayerSurfaceError::DisplayEnumerationTimeout { timeout });
                    cx.update(|app| {
                        record_setup_failure(&inner, error);
                        stop_runtime(&inner, app);
                        request_host_quit(&inner, app);
                    });
                    break;
                }
                Err(StartupTargetError::DisplayTimeout {
                    display_id,
                    timeout,
                }) => {
                    work.active.set(false);
                    cx.update(|app| {
                        record_surface_failure(
                            &inner,
                            SurfaceError::DisplayReadinessTimeout {
                                display_id,
                                timeout,
                            },
                            app,
                        )
                    });
                    break;
                }
            };

            let mut failed = false;
            for (display_id, bounds) in ready {
                if inner.stopped.get() {
                    break;
                }
                let config = task_configure(display_id, bounds);
                if inner.stopped.get() {
                    break;
                }
                if !valid_layer_config(&config) {
                    work.active.set(false);
                    let cause: Rc<dyn std::error::Error + Send + Sync> =
                        Rc::new(LayerSurfaceError::InvalidConfiguration { size: config.size });
                    cx.update(|app| {
                        record_surface_failure(
                            &inner,
                            SurfaceError::Creation { display_id, cause },
                            app,
                        )
                    });
                    failed = true;
                    break;
                }
                if inner.stopped.get() {
                    break;
                }
                let Some(mut factory) = task_factory.borrow_mut().take() else {
                    failed = true;
                    break;
                };
                let mut options = config.options;
                options.namespace = SURFACE_NAMESPACE.to_owned();
                options.keyboard_interactivity = gpui::layer_shell::KeyboardInteractivity::None;
                let opened = cx
                    .open_window(
                        super::surface_window::layer_window_options(
                            display_id,
                            config.size,
                            options,
                        ),
                        |window, app| {
                            window.set_input_region(Some(&[]));
                            (factory.content)(display_id, window, app)
                        },
                    )
                    .map(Into::into)
                    .map_err(|cause| cause.into_boxed_dyn_error());
                if inner.stopped.get() {
                    drop(factory);
                    cx.update(|app| finish_surface_open(&inner, display_id, opened, app));
                    failed = true;
                    break;
                }
                cx.update(|app| finish_surface_open(&inner, display_id, opened, app));
                if !inner.stopped.get() {
                    *task_factory.borrow_mut() = Some(factory);
                } else {
                    drop(factory);
                }
                if inner.stopped.get() {
                    failed = true;
                    break;
                }
            }
            work.active.set(false);
            if failed || inner.stopped.get() || targets.finished() {
                let cancel = work.cancel.borrow_mut().take();
                if let Some(cancel) = cancel {
                    cancel();
                }
                break;
            }
            drop(inner);
            drop(work);
            cx.background_executor().timer(DISPLAY_POLL_INTERVAL).await;
        }
    });
    *work.task.borrow_mut() = Some(task);
    inner.surface_works.borrow_mut().push(work);
    Ok(())
}

fn valid_layer_config(config: &LayerSurfaceConfig) -> bool {
    [config.size.width, config.size.height]
        .into_iter()
        .all(|dimension| f32::from(dimension).is_finite() && f32::from(dimension) >= 1.0)
        && config
            .options
            .exclusive_zone
            .is_none_or(|zone| f32::from(zone).is_finite() && f32::from(zone) >= 0.0)
}

pub(super) fn finish_surface_open(
    inner: &RuntimeInner,
    display_id: gpui::DisplayId,
    opened: Result<AnyWindowHandle, Box<dyn std::error::Error + Send + Sync>>,
    cx: &mut App,
) {
    match opened {
        Ok(handle) if !inner.stopped.get() => inner.surfaces.borrow_mut().push(handle),
        Ok(handle) => super::surface_window::remove_window(cx, handle),
        Err(cause) => record_surface_failure(
            inner,
            SurfaceError::Creation {
                display_id,
                cause: Rc::from(cause),
            },
            cx,
        ),
    }
}

pub(super) fn stop_surface_works(inner: &RuntimeInner) {
    let works = inner.surface_works.borrow().clone();
    for work in works {
        if !work.active.get() {
            work.task.borrow_mut().take();
            cancel_surface_work(&work);
        }
    }
}

fn cancel_surface_work(work: &SurfaceWork) {
    let cancel = work.cancel.borrow_mut().take();
    if let Some(cancel) = cancel {
        cancel();
    }
}

pub(super) fn drop_surface_works(works: &mut Vec<Rc<SurfaceWork>>) {
    for work in std::mem::take(works) {
        work.task.borrow_mut().take();
        cancel_surface_work(&work);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{Shell, test_support::*};
    use crate::surface::SurfaceDescriptor;
    use gpui::{AppContext, DisplayId, Size, TestAppContext, WindowOptions, px};
    use std::{
        cell::{Cell, RefCell},
        error::Error,
        rc::Rc,
    };

    #[test]
    fn shutdown_cancels_waiting_surface_before_its_factory_runs() {
        let cx = TestAppContext::single();
        let called = Rc::new(Cell::new(false));
        let called_by_factory = Rc::clone(&called);
        let factory_lifetime = Rc::new(());
        let factory_weak = Rc::downgrade(&factory_lifetime);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                let descriptor = SurfaceDescriptor::new(
                    DisplayId::new(99),
                    Size::new(px(48.), px(20.)),
                    move |_, cx| {
                        let _ = &factory_lifetime;
                        called_by_factory.set(true);
                        cx.new(|_| gpui::EmptyView)
                    },
                )?
                .readiness_timeout(std::time::Duration::from_secs(30));
                runtime.mount_surface(descriptor, cx)
            })
            .unwrap()
        });
        cx.run_until_parked();
        assert!(
            runtime
                .0
                .surface_works
                .borrow()
                .iter()
                .all(|work| !work.active.get())
        );
        cx.update(|cx| runtime.shutdown(cx));
        assert!(factory_weak.upgrade().is_none());
        cx.run_until_parked();
        assert!(!called.get());
        assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    }

    #[test]
    fn creation_result_error_is_fatal_once_and_preserves_cause_target_and_host_window() {
        let cx = TestAppContext::single();
        let host = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap()
        });
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let (resource, _) = descriptor(starts.clone(), stops.clone(), false);
        let binding = Rc::new(RefCell::new(None));
        let binding_slot = Rc::clone(&binding);
        let target = DisplayId::new(99);
        let factory_lifetime = Rc::new(());
        let factory_weak = Rc::downgrade(&factory_lifetime);
        let called = Rc::new(Cell::new(false));
        let called_by_factory = Rc::clone(&called);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                *binding_slot.borrow_mut() = Some(runtime.mount(&resource, cx)?);
                let surface =
                    SurfaceDescriptor::new(target, Size::new(px(48.), px(20.)), move |_, cx| {
                        let _ = &factory_lifetime;
                        called_by_factory.set(true);
                        cx.new(|_| gpui::EmptyView)
                    })?;
                runtime.mount_surface(surface, cx)
            })
            .unwrap()
        });
        cx.run_until_parked();
        assert!(
            runtime
                .0
                .surface_works
                .borrow()
                .iter()
                .all(|work| !work.active.get())
        );
        let cause: Box<dyn std::error::Error + Send + Sync> =
            Box::new(std::io::Error::other("layer creation failed"));
        cx.update(|cx| finish_surface_open(&runtime.0, target, Err(cause), cx));

        let Some(Err(ShellError::Surface(error))) = runtime.result() else {
            panic!("expected surface creation failure")
        };
        assert_eq!(error.display_id(), target);
        assert_eq!(
            error
                .source()
                .and_then(|cause| cause.downcast_ref::<std::io::Error>())
                .map(std::io::Error::to_string)
                .as_deref(),
            Some("layer creation failed")
        );
        assert_eq!((starts.get(), stops.get()), (1, 1));
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        assert!(host.read_with(&cx, |_, _| ()).is_ok());
        assert!(factory_weak.upgrade().is_none());
        assert!(!called.get());

        let retry =
            SurfaceDescriptor::new(DisplayId::new(1), Size::new(px(48.), px(20.)), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap();
        assert!(matches!(
            cx.update(|cx| runtime.mount_surface(retry, cx)),
            Err(ShellError::Setup(_))
        ));
        cx.run_until_parked();
        assert_eq!(starts.get(), 1);
        assert_eq!(stops.get(), 1);
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        drop(binding);
    }

    #[test]
    fn an_existing_source_failure_remains_primary_over_a_late_creation_error() {
        let cx = TestAppContext::single();
        let host = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap()
        });
        let stops = Rc::new(Cell::new(0));
        let (resource, publisher) = descriptor(Rc::new(Cell::new(0)), stops.clone(), false);
        let binding = Rc::new(RefCell::new(None));
        let binding_slot = Rc::clone(&binding);
        let target = DisplayId::new(99);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                *binding_slot.borrow_mut() = Some(runtime.mount(&resource, cx)?);
                let surface =
                    SurfaceDescriptor::new(target, Size::new(px(48.), px(20.)), |_, cx| {
                        cx.new(|_| gpui::EmptyView)
                    })?;
                runtime.mount_surface(surface, cx)
            })
            .unwrap()
        });
        let publisher = publisher.borrow().as_ref().unwrap().clone();

        cx.update(|cx| publisher.fail(TestError("source failed first"), cx));
        let cause: Box<dyn std::error::Error + Send + Sync> =
            Box::new(std::io::Error::other("late create error"));
        cx.update(|cx| finish_surface_open(&runtime.0, target, Err(cause), cx));

        let Some(Err(ShellError::Source(error))) = runtime.result() else {
            panic!("expected the first source failure to remain primary")
        };
        assert_eq!(
            error
                .source()
                .and_then(|cause| cause.downcast_ref::<TestError>())
                .map(|error| error.0),
            Some("source failed first")
        );
        assert_eq!(stops.get(), 1);
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        assert!(host.read_with(&cx, |_, _| ()).is_ok());
        drop(binding);
    }
}
