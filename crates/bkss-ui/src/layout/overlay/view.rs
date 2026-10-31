use super::*;

pub(super) struct OverlayView<V> {
    content: Entity<V>,
    instance: SurfaceInstanceHandle,
    focus: Option<FocusHandle>,
    geometry: OverlayGeometry,
    pointer_dismiss: PointerDismiss,
    glass: Glass,
    #[cfg(test)]
    observed: std::rc::Rc<std::cell::Cell<LayoutObservation>>,
}

impl<V: Render + 'static> OverlayView<V> {
    pub(super) fn new(
        definition: &OverlayDefinition,
        content: Entity<V>,
        instance: SurfaceInstanceHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = (definition.keyboard == KeyboardFocus::Interactive).then(|| cx.focus_handle());
        if window.focused(cx).is_none()
            && let Some(focus) = &focus
        {
            focus.focus(window, cx);
        }
        Self {
            content,
            instance,
            focus,
            geometry: definition.geometry,
            pointer_dismiss: definition.pointer_dismiss,
            glass: Glass::new(definition.style.clone(), cx),
            #[cfg(test)]
            observed: Default::default(),
        }
    }
}

impl<V: Render + 'static> Render for OverlayView<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let limit = self.geometry.limit(viewport, window.scale_factor());
        let mut root = div().w(viewport.width).h(viewport.height).overflow_hidden();
        if let Some(focus) = &self.focus {
            let instance = self.instance.clone();
            root = root.track_focus(focus).on_key_down(move |event, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    // A weak recipient can outlive shutdown; core already owns cleanup.
                    let _ = instance.close(cx);
                }
            });
        }
        if self.pointer_dismiss == PointerDismiss::Backdrop {
            let instance = self.instance.clone();
            root = root
                .bg(rgba(0x00000066))
                .on_any_mouse_down(move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = instance.close(cx);
                });
        }
        let opacity = self.instance.animation_sample().opacity.unwrap_or(1.0);
        let content = content_frame(&self.content, &self.glass, limit, opacity, cx);
        root.child(OverlayLayout {
            content,
            geometry: self.geometry,
            backdrop: self.pointer_dismiss == PointerDismiss::Backdrop,
            #[cfg(test)]
            observed: self.observed.clone(),
        })
    }
}

fn content_frame<V: Render + 'static>(
    content: &Entity<V>,
    glass: &Glass,
    limit: Size<Pixels>,
    opacity: f32,
    cx: &impl AppContext,
) -> AnyElement {
    // This ordinary container stops bubbling, after the user's own handlers.
    div()
        .id("bkss-overlay-content")
        .opacity(opacity)
        .flex()
        .flex_col()
        .max_w(limit.width)
        .max_h(limit.height)
        .overflow_hidden()
        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
        .child(
            glass
                .element(GlassState::default(), cx)
                .flex()
                .flex_col()
                .max_w(limit.width)
                .max_h(limit.height)
                .overflow_hidden()
                .child(content.clone()),
        )
        .into_any_element()
}

fn input_region(bounds: Bounds<Pixels>, viewport: Size<Pixels>) -> Option<Bounds<Pixels>> {
    if [
        bounds.left(),
        bounds.top(),
        bounds.right(),
        bounds.bottom(),
        viewport.width,
        viewport.height,
    ]
    .into_iter()
    .any(|value| !f32::from(value).is_finite())
    {
        return None;
    }
    // Round inward in integer logical coordinates; never expand into transparent
    // pixels. This f32 is the last exact native i32 coordinate before saturation.
    let maximum = px(2_147_483_520.);
    let left = f32::from(bounds.left().max(px(0.)).min(maximum)).ceil();
    let top = f32::from(bounds.top().max(px(0.)).min(maximum)).ceil();
    let right = f32::from(bounds.right().min(viewport.width).min(maximum)).floor();
    let bottom = f32::from(bounds.bottom().min(viewport.height).min(maximum)).floor();
    (right > left && bottom > top).then(|| {
        Bounds::new(
            point(px(left), px(top)),
            size(px(right - left), px(bottom - top)),
        )
    })
}

// Intrinsic measurement and placement need the same frame's measured size.
// A second render with cached dimensions would expose a stale native hit region.
struct OverlayLayout {
    content: AnyElement,
    geometry: OverlayGeometry,
    backdrop: bool,
    #[cfg(test)]
    observed: std::rc::Rc<std::cell::Cell<LayoutObservation>>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
struct LayoutObservation {
    content: Option<Bounds<Pixels>>,
    input: Option<Bounds<Pixels>>,
}

impl IntoElement for OverlayLayout {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for OverlayLayout {
    type RequestLayoutState = ();
    type PrepaintState = Option<Bounds<Pixels>>;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (window.request_layout(Style::default(), [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let viewport = window.viewport_size();
        if viewport.width <= px(0.) || viewport.height <= px(0.) {
            window.set_input_region(Some(&[]));
            #[cfg(test)]
            self.observed.set(LayoutObservation::default());
            return None;
        }
        let natural = self.content.layout_as_root(
            size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
            window,
            cx,
        );
        let bounds = self
            .geometry
            .bounds(natural, viewport, window.scale_factor());
        let input = if self.backdrop {
            Bounds::new(point(px(0.), px(0.)), viewport)
        } else {
            bounds
        };
        let input = input_region(input, viewport);
        window.set_input_region(Some(input.as_slice()));
        #[cfg(test)]
        self.observed.set(LayoutObservation {
            content: Some(bounds),
            input,
        });
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.content.prepaint_at(bounds.origin, window, cx);
        });
        Some(bounds)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        bounds: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(bounds) = *bounds {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                self.content.paint(window, cx)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, WindowBounds, WindowOptions};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    struct SizedContent(Size<Pixels>);
    impl Render for SizedContent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(self.0.width).h(self.0.height)
        }
    }

    struct Frame {
        content: Entity<SizedContent>,
        glass: Glass,
        geometry: OverlayGeometry,
        backdrop: bool,
        observed: Rc<Cell<LayoutObservation>>,
    }

    impl Render for Frame {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let viewport = window.viewport_size();
            div()
                .w(viewport.width)
                .h(viewport.height)
                .overflow_hidden()
                .child(OverlayLayout {
                    content: content_frame(
                        &self.content,
                        &self.glass,
                        self.geometry.limit(viewport, window.scale_factor()),
                        1.0,
                        cx,
                    ),
                    geometry: self.geometry,
                    backdrop: self.backdrop,
                    observed: self.observed.clone(),
                })
        }
    }

    fn layout(
        geometry: OverlayGeometry,
        natural: Size<Pixels>,
        viewport: Size<Pixels>,
        scale: f32,
        backdrop: bool,
    ) -> (LayoutObservation, Bounds<Pixels>) {
        let cx = TestAppContext::single();
        let observed = Rc::new(Cell::new(LayoutObservation::default()));
        let captured = observed.clone();
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(-800.), px(300.)),
                        viewport,
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    let content = cx.new(|_| SizedContent(natural));
                    cx.new(|cx| Frame {
                        content,
                        glass: Glass::new(GlassStyle::default(), cx),
                        geometry,
                        backdrop,
                        observed: captured,
                    })
                },
            )
            .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), &cx);
        visual.simulate_scale_factor_change(scale);
        visual.run_until_parked();
        let painted = visual.update(|window, _| {
            let quads = window.painted_quads();
            assert_eq!(
                quads.len(),
                1,
                "the natural Glass frame is the only painted box"
            );
            let bounds = quads[0].bounds;
            rect(
                bounds.origin.x.0 / scale,
                bounds.origin.y.0 / scale,
                bounds.size.width.0 / scale,
                bounds.size.height.0 / scale,
            )
        });
        (observed.get(), painted)
    }

    fn close_rect(actual: Bounds<Pixels>, expected: Bounds<Pixels>, scale: f32) {
        for (actual, expected) in [
            (actual.left(), expected.left()),
            (actual.top(), expected.top()),
            (actual.right(), expected.right()),
            (actual.bottom(), expected.bottom()),
        ] {
            assert!(
                f32::from(actual - expected).abs() <= 1. / scale,
                "actual={actual:?}, expected={expected:?}"
            );
        }
    }

    #[test]
    fn actual_layout_measures_natural_content_centers_and_clamps_in_physical_units() {
        for scale in [1., 1.5, 2.] {
            for natural in [size(px(120.), px(60.)), size(px(1200.), px(900.))] {
                let geometry = OverlayGeometry {
                    max_logical_size: Some([300.3, 180.3]),
                    ..Default::default()
                };
                let viewport = size(px(640.), px(360.));
                let (observed, painted) = layout(geometry, natural, viewport, scale, false);
                let actual = observed.content.unwrap();
                let expected = geometry.bounds(
                    size(natural.width.min(px(300.3)), natural.height.min(px(180.3))),
                    viewport,
                    scale,
                );
                close_rect(actual, expected, scale);
                close_rect(painted, actual, scale);
                assert_eq!(observed.input, input_region(actual, viewport));
                assert!(actual.size.width <= px(300.3) && actual.size.height <= px(180.3));
            }
        }
    }

    #[test]
    fn actual_layout_supports_edges_offsets_oversize_and_viewport_only_backdrop() {
        let viewport = size(px(301.), px(201.));
        for edges in [
            OverlayAnchors {
                top: true,
                left: true,
                ..Default::default()
            },
            OverlayAnchors {
                bottom: true,
                right: true,
                ..Default::default()
            },
            OverlayAnchors {
                top: true,
                ..Default::default()
            },
        ] {
            for offset in [[12.2, -8.5], [-1000., 1000.]] {
                for scale in [1., 1.5, 2.] {
                    let geometry = OverlayGeometry {
                        placement: OverlayPlacement::Anchor { edges, offset },
                        ..Default::default()
                    };
                    let natural = size(px(80.), px(40.));
                    let (observed, painted) = layout(geometry, natural, viewport, scale, true);
                    close_rect(painted, geometry.bounds(natural, viewport, scale), scale);
                    assert_eq!(observed.input, Some(rect(0., 0., 301., 201.)));
                }
            }
        }
        let (observed, painted) = layout(
            OverlayGeometry::default(),
            size(px(1200.), px(800.)),
            viewport,
            1.5,
            false,
        );
        close_rect(painted, rect(0., 0., 301., 201.), 1.5);
        assert_eq!(
            observed.input,
            input_region(observed.content.unwrap(), viewport)
        );
        assert_eq!(
            input_region(rect(10.2, -5.2, 20.6, 30.6), viewport),
            Some(rect(11., 0., 19., 25.))
        );
        assert_eq!(input_region(rect(f32::NAN, 0., 1., 1.), viewport), None);
        assert_eq!(input_region(rect(300.2, 0., 1., 10.), viewport), None);
        assert_eq!(
            input_region(
                rect(0., 0., f32::MAX, f32::MAX),
                size(px(f32::MAX), px(f32::MAX))
            ),
            Some(rect(0., 0., 2_147_483_520., 2_147_483_520.))
        );
    }

    struct EventContent {
        clicks: Rc<Cell<usize>>,
        keys: Rc<Cell<usize>>,
        focus: Option<FocusHandle>,
        consume_escape: bool,
        scroll: bool,
        scroll_handle: gpui::ScrollHandle,
    }

    impl Render for EventContent {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut content = div()
                .id("user-content")
                .bg(gpui::rgb(0xffffff))
                .w(px(100.))
                .h(px(80.))
                .flex_shrink_0()
                .on_any_mouse_down(
                    cx.listener(|this, _, _, _| this.clicks.set(this.clicks.get() + 1)),
                );
            if let Some(focus) = &self.focus {
                content = content.track_focus(focus).on_key_down(cx.listener(
                    |this, event: &gpui::KeyDownEvent, _, cx| {
                        this.keys.set(this.keys.get() + 1);
                        if this.consume_escape && event.keystroke.key == "escape" {
                            cx.stop_propagation();
                        }
                    },
                ));
            }
            if self.scroll {
                content = content
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .child(div().h(px(400.)).flex_shrink_0());
            }
            content
        }
    }

    struct Events {
        window: gpui::AnyWindowHandle,
        instance: SurfaceInstanceHandle,
        clicks: Rc<Cell<usize>>,
        keys: Rc<Cell<usize>>,
        child_focus: Option<FocusHandle>,
        scroll_handle: gpui::ScrollHandle,
        observed: Rc<Cell<LayoutObservation>>,
    }

    fn open_events(
        cx: &TestAppContext,
        runtime: &RuntimeHandle,
        definition: OverlayDefinition,
        child_focus: bool,
        consume_escape: bool,
        scroll: bool,
    ) -> (bkss_core::DynamicSurfaceHandle, Rc<RefCell<Vec<Events>>>) {
        let records = Rc::new(RefCell::new(Vec::new()));
        let captured = records.clone();
        let surface = cx.update(|cx| {
            let config = definition.surface_config();
            let plan = definition
                .animation
                .map(Animation::compile)
                .transpose()
                .unwrap();
            let factory =
                move |instance: SurfaceInstanceHandle, window: &mut Window, cx: &mut App| {
                    let clicks = Rc::new(Cell::new(0));
                    let keys = Rc::new(Cell::new(0));
                    let content = cx.new(|cx| {
                        let focus = child_focus.then(|| cx.focus_handle());
                        if let Some(focus) = &focus {
                            focus.focus(window, cx);
                        }
                        EventContent {
                            clicks: clicks.clone(),
                            keys: keys.clone(),
                            focus,
                            consume_escape,
                            scroll,
                            scroll_handle: gpui::ScrollHandle::new(),
                        }
                    });
                    let view = cx.new(|cx| {
                        OverlayView::new(&definition, content.clone(), instance.clone(), window, cx)
                    });
                    captured.borrow_mut().push(Events {
                        window: window.window_handle(),
                        instance: instance.clone(),
                        clicks,
                        keys,
                        child_focus: content.read(cx).focus.clone(),
                        scroll_handle: content.read(cx).scroll_handle.clone(),
                        observed: view.read(cx).observed.clone(),
                    });
                    Ok(view)
                };
            match plan {
                Some(plan) => runtime.register_animated_dynamic_layer_surface(
                    config,
                    Duration::from_secs(5),
                    plan,
                    factory,
                    cx,
                ),
                None => runtime.register_dynamic_layer_surface(
                    config,
                    Duration::from_secs(5),
                    factory,
                    cx,
                ),
            }
            .unwrap()
        });
        let display = cx.update(|cx| cx.displays()[0].id());
        cx.update(|cx| surface.open(display, cx).unwrap());
        cx.run_until_parked();
        let window = records.borrow()[0].window;
        cx.simulate_window_resize(window, size(px(400.), px(300.)));
        cx.run_until_parked();
        (surface, records)
    }

    #[test]
    fn actual_pointer_handlers_keep_content_and_consume_outside_for_only_the_receiving_generation()
    {
        use gpui::InputEvent;
        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| bkss_core::Shell::attach(cx, |_, _| Ok(())).unwrap());
        let definition = OverlayDefinition::new(KeyboardFocus::None)
            .with_pointer_dismiss(PointerDismiss::Backdrop);
        let (surface, first) = open_events(&cx, &runtime, definition.clone(), false, false, false);
        let (_, second) = open_events(&cx, &runtime, definition, false, false, false);
        let window = first.borrow()[0].window;
        let stale = first.borrow()[0].instance.clone();
        let mut visual = VisualTestContext::from_window(window, &cx);
        visual.simulate_mouse_down(
            point(px(200.), px(150.)),
            gpui::MouseButton::Left,
            gpui::Modifiers::none(),
        );
        assert_eq!(first.borrow()[0].clicks.get(), 1);
        assert_eq!(cx.update(|cx| cx.windows().len()), 2);
        let consumed = visual.update(|window, cx| {
            !window
                .dispatch_event(
                    gpui::MouseDownEvent {
                        position: point(px(10.), px(10.)),
                        button: gpui::MouseButton::Right,
                        modifiers: gpui::Modifiers::none(),
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input(),
                    cx,
                )
                .propagate
        });
        assert!(consumed, "outside pointer-down must not propagate");
        cx.run_until_parked();
        assert_eq!(
            cx.update(|cx| cx.windows()),
            vec![second.borrow()[0].window]
        );
        let display = stale.display_id();
        cx.update(|cx| surface.open(display, cx).unwrap());
        cx.run_until_parked();
        assert_eq!(first.borrow().len(), 2);
        cx.update(|cx| stale.close(cx).unwrap());
        cx.run_until_parked();
        assert_eq!(
            cx.update(|cx| cx.windows().len()),
            2,
            "an old dismissal cannot close the new generation"
        );
        cx.update(|cx| runtime.shutdown(cx));
    }

    #[test]
    fn escape_bubbles_from_content_without_stealing_child_focus_and_none_does_not_dismiss() {
        for (keyboard, consume, should_close) in [
            (KeyboardFocus::Interactive, false, true),
            (KeyboardFocus::Interactive, true, false),
            (KeyboardFocus::None, false, false),
        ] {
            let cx = TestAppContext::single();
            let runtime = cx.update(|cx| bkss_core::Shell::attach(cx, |_, _| Ok(())).unwrap());
            let (_, records) = open_events(
                &cx,
                &runtime,
                OverlayDefinition::new(keyboard),
                true,
                consume,
                false,
            );
            let window = records.borrow()[0].window;
            let focus = records.borrow()[0].child_focus.clone().unwrap();
            let mut visual = VisualTestContext::from_window(window, &cx);
            assert!(visual.update(|window, _| focus.is_focused(window)));
            visual.simulate_keystrokes("escape");
            assert_eq!(records.borrow()[0].keys.get(), 1);
            assert_eq!(cx.update(|cx| cx.windows().is_empty()), should_close);
            cx.update(|cx| runtime.shutdown(cx));
        }
        // Synthetic events above exercise GPUI routing, not native None delivery.
    }

    #[test]
    fn explicit_child_scroll_works_without_an_outer_scroll_container() {
        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| bkss_core::Shell::attach(cx, |_, _| Ok(())).unwrap());
        let (_, records) = open_events(
            &cx,
            &runtime,
            OverlayDefinition::new(KeyboardFocus::None),
            false,
            false,
            true,
        );
        let window = records.borrow()[0].window;
        let mut visual = VisualTestContext::from_window(window, &cx);
        let scroll_handle = records.borrow()[0].scroll_handle.clone();
        assert_eq!(scroll_handle.offset(), point(px(0.), px(0.)));
        // The only scrollable element belongs to the user's view; the wrapper
        // has a content mask and no scroll listener or scroll offset of its own.
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: point(px(200.), px(150.)),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-30.))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        assert!(
            scroll_handle.offset().y < px(0.),
            "the explicit child must receive the wheel event"
        );
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        cx.update(|cx| runtime.shutdown(cx));
    }

    #[test]
    fn configured_local_resize_updates_layout_and_zero_viewport_has_no_input() {
        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| bkss_core::Shell::attach(cx, |_, _| Ok(())).unwrap());
        let (_, records) = open_events(
            &cx,
            &runtime,
            OverlayDefinition::new(KeyboardFocus::None),
            false,
            false,
            false,
        );
        let window = records.borrow()[0].window;
        let observed = records.borrow()[0].observed.clone();
        assert_eq!(observed.get().content, Some(rect(150., 110., 100., 80.)));
        cx.simulate_window_resize(window, size(px(200.), px(100.)));
        cx.run_until_parked();
        assert_eq!(observed.get().content, Some(rect(50., 10., 100., 80.)));
        assert_eq!(observed.get().input, Some(rect(50., 10., 100., 80.)));
        cx.simulate_window_resize(window, size(px(0.), px(0.)));
        cx.run_until_parked();
        assert_eq!(observed.get().content, None);
        assert_eq!(observed.get().input, None);
        cx.update(|cx| runtime.shutdown(cx));
    }

    #[test]
    fn default_key_route_is_interactive_only_and_escape_closes_only_its_recipient() {
        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| bkss_core::Shell::attach(cx, |_, _| Ok(())).unwrap());
        let (_, none) = open_events(
            &cx,
            &runtime,
            OverlayDefinition::new(KeyboardFocus::None),
            false,
            false,
            false,
        );
        let mut visual_none = VisualTestContext::from_window(none.borrow()[0].window, &cx);
        assert!(visual_none.update(|window, cx| window.focused(cx).is_none()));
        let (_, first) = open_events(
            &cx,
            &runtime,
            OverlayDefinition::new(KeyboardFocus::Interactive),
            false,
            false,
            false,
        );
        let (_, second) = open_events(
            &cx,
            &runtime,
            OverlayDefinition::new(KeyboardFocus::Interactive),
            false,
            false,
            false,
        );
        let second_window = second.borrow()[0].window;
        let mut visual = VisualTestContext::from_window(second_window, &cx);
        assert!(visual.update(|window, cx| window.focused(cx).is_some()));
        visual.simulate_keystrokes("escape");
        let remaining = cx.update(|cx| cx.windows());
        assert!(remaining.contains(&first.borrow()[0].window));
        assert!(remaining.contains(&none.borrow()[0].window));
        assert!(!remaining.contains(&second_window));
        cx.update(|cx| runtime.shutdown(cx));
    }

    #[test]
    fn actual_maximum_size_is_clamped_after_physical_conversion() {
        for (scale, expected_width, expected_height) in [
            (1., 120., 80.),
            (1.5, 181. / 1.5, 121. / 1.5),
            (2., 120.5, 80.5),
        ] {
            let geometry = OverlayGeometry {
                max_logical_size: Some([120.8, 80.8]),
                ..Default::default()
            };
            let (observed, _) = layout(
                geometry,
                size(px(1000.), px(1000.)),
                size(px(500.), px(300.)),
                scale,
                false,
            );
            let bounds = observed.content.unwrap();
            assert!((f32::from(bounds.size.width) - expected_width).abs() < 0.001);
            assert!((f32::from(bounds.size.height) - expected_height).abs() < 0.001);
        }
    }

    #[test]
    fn escape_and_backdrop_keep_the_animated_recipient_until_exit_finishes() {
        use crate::{EasingCurve, Timing};
        for escape in [false, true] {
            let cx = TestAppContext::single();
            let runtime = cx.update(|cx| bkss_core::Shell::attach(cx, |_, _| Ok(())).unwrap());
            let definition = OverlayDefinition::new(KeyboardFocus::Interactive)
                .with_pointer_dismiss(PointerDismiss::Backdrop)
                .with_animation(
                    Animation::exit(Timing::Easing {
                        duration_ms: 1000,
                        curve: EasingCurve::Linear,
                    })
                    .opacity(1., 0.),
                )
                .unwrap();
            let (_, records) = open_events(&cx, &runtime, definition, false, false, false);
            let window = records.borrow()[0].window;
            let instance = records.borrow()[0].instance.clone();
            let observed = records.borrow()[0].observed.clone();
            let initial = observed.get();
            let mut visual = VisualTestContext::from_window(window, &cx);
            if escape {
                visual.simulate_keystrokes("escape");
            } else {
                visual.simulate_mouse_down(
                    point(px(10.), px(10.)),
                    gpui::MouseButton::Left,
                    gpui::Modifiers::none(),
                );
            }
            cx.run_until_parked();
            assert_eq!(cx.update(|cx| cx.windows().len()), 1);
            cx.executor().advance_clock(Duration::from_millis(500));
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
            });
            cx.run_until_parked();
            assert!((instance.animation_sample().opacity.unwrap() - 0.5).abs() < 0.001);
            let quads = visual.update(|window, _| window.painted_quads());
            assert_eq!(quads.len(), 3);
            assert!(
                (quads[0].background.as_solid().unwrap().a - 0.4).abs() < 0.001,
                "backdrop must not fade"
            );
            assert!(
                (quads[1].background.as_solid().unwrap().a - 0.125).abs() < 0.001,
                "Glass tint shares the content wrapper"
            );
            assert!(
                (quads[2].background.as_solid().unwrap().a - 0.5).abs() < 0.001,
                "user content must fade with Glass"
            );
            assert_eq!(observed.get().input, initial.input);
            assert_eq!(observed.get().content, initial.content);
            cx.executor().advance_clock(Duration::from_millis(500));
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
            });
            cx.run_until_parked();
            assert!(cx.update(|cx| cx.windows().is_empty()));
            cx.update(|cx| runtime.shutdown(cx));
        }
    }
}
