use super::{
    declaration::ShellSurface,
    geometry::{Geometry, input_region},
};
use bkss_core::SurfaceInstanceHandle;
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Context, Element, ElementId, Entity,
    FocusHandle, GlobalElementId, InspectorElementId, Interactivity, IntoElement, LayoutId, Pixels,
    Refineable, Render, Style, Styled, Window, div, layer_shell::KeyboardInteractivity, point,
    prelude::*, px, rgba, size,
};
use std::{cell::Cell, rc::Rc};

pub(super) struct SurfaceView<V> {
    definition: ShellSurface,
    content: Entity<V>,
    instance: SurfaceInstanceHandle,
    focus: Option<FocusHandle>,
    animated: bool,
    initialized: Rc<Cell<bool>>,
    failed: Rc<Cell<bool>>,
    #[cfg(test)]
    observed: Rc<Cell<Observation>>,
}

impl<V: Render + 'static> SurfaceView<V> {
    pub fn new(
        definition: ShellSurface,
        content: Entity<V>,
        instance: SurfaceInstanceHandle,
        animated: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus =
            (definition.keyboard() != KeyboardInteractivity::None).then(|| cx.focus_handle());
        // This is only GPUI's fallback key route, not a native activation request.
        if window.focused(cx).is_none()
            && let Some(focus) = &focus
        {
            focus.focus(window, cx);
        }
        #[cfg(test)]
        let observed = definition.observed.clone();
        Self {
            definition,
            content,
            instance,
            focus,
            animated,
            initialized: Rc::new(Cell::new(false)),
            failed: Rc::new(Cell::new(false)),
            #[cfg(test)]
            observed,
        }
    }
}

impl<V: Render + 'static> Render for SurfaceView<V> {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let limit = self
            .definition
            .geometry
            .limit(viewport, window.scale_factor());
        let mut content = match self.definition.geometry {
            Geometry::Bar(bar) if bar.horizontal() => {
                let root = div()
                    .flex()
                    .items_center()
                    .h(bar.thickness.get())
                    .px(px(12.));
                if bar.full_length {
                    root.w(limit.width)
                } else {
                    root
                }
            }
            Geometry::Bar(bar) => {
                let root = div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .w(bar.thickness.get())
                    .py(px(12.));
                if bar.full_length {
                    root.h(limit.height)
                } else {
                    root
                }
            }
            Geometry::Content { .. } => div().flex().flex_col(),
        };
        content.style().refine(&self.definition.style);
        let content = div()
            .flex()
            .flex_col()
            .max_w(limit.width)
            .max_h(limit.height)
            .overflow_hidden()
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .child(content.flex_shrink_0().child(self.content.clone()))
            .into_any_element();
        let mut root = div().w(viewport.width).h(viewport.height).overflow_hidden();
        if let Some(focus) = &self.focus {
            let instance = self.instance.clone();
            root = root.track_focus(focus).on_key_down(move |event, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    let _ = instance.close(cx);
                }
            });
        }
        if self.definition.backdrop {
            let instance = self.instance.clone();
            root = root
                .bg(rgba(0x00000066))
                .on_any_mouse_down(move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = instance.close(cx);
                });
        }
        root.child(SurfaceLayout {
            content,
            definition: self.definition.clone(),
            instance: self.instance.clone(),
            animated: self.animated,
            initialized: self.initialized.clone(),
            failed: self.failed.clone(),
            opacity: Interactivity::default(),
            #[cfg(test)]
            observed: self.observed.clone(),
        })
    }
}

struct SurfaceLayout {
    content: AnyElement,
    definition: ShellSurface,
    instance: SurfaceInstanceHandle,
    animated: bool,
    initialized: Rc<Cell<bool>>,
    failed: Rc<Cell<bool>>,
    opacity: Interactivity,
    #[cfg(test)]
    observed: Rc<Cell<Observation>>,
}

struct PaintedContent {
    bounds: Bounds<Pixels>,
    mask: Bounds<Pixels>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Observation {
    content: Option<Bounds<Pixels>>,
    input: Option<Bounds<Pixels>>,
    sample: Option<bkss_core::AnimationSample>,
}

impl IntoElement for SurfaceLayout {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for SurfaceLayout {
    type RequestLayoutState = ();
    type PrepaintState = Option<PaintedContent>;

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
        let scale = window.scale_factor();
        #[cfg(test)]
        self.observed.set(Observation::default());
        if self.failed.get()
            || viewport.width <= px(0.)
            || viewport.height <= px(0.)
            || !f32::from(viewport.width).is_finite()
            || !f32::from(viewport.height).is_finite()
            || !scale.is_finite()
            || scale <= 0.
        {
            window.set_input_region(Some(&[]));
            return None;
        }
        let natural = self.content.layout_as_root(
            size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
            window,
            cx,
        );
        if [natural.width, natural.height]
            .into_iter()
            .any(|v| !f32::from(v).is_finite())
        {
            window.set_input_region(Some(&[]));
            return None;
        }
        let sample = if self.animated && !self.initialized.get() {
            let plan = self.definition.transitions.resolve(natural);
            let result = plan
                .map_err(bkss_core::ShellError::setup)
                .and_then(|plan| self.instance.initialize_animation(plan, cx));
            match result {
                Ok(sample) => {
                    self.initialized.set(true);
                    sample
                }
                Err(_) => {
                    // Core owns fatal failure and cleanup; never reveal fallback content.
                    self.failed.set(true);
                    window.set_input_region(Some(&[]));
                    return None;
                }
            }
        } else {
            self.instance.animation_sample()
        };
        let mut bounds = self.definition.geometry.bounds(natural, viewport, scale);
        bounds.origin = window.pixel_snap_point(
            bounds.origin
                + point(
                    px(sample.translation_x.unwrap_or(0.)),
                    px(sample.translation_y.unwrap_or(0.)),
                ),
        );
        if [bounds.left(), bounds.top(), bounds.right(), bounds.bottom()]
            .into_iter()
            .any(|value| !f32::from(value).is_finite())
        {
            window.set_input_region(Some(&[]));
            return None;
        }
        let viewport_bounds = Bounds::new(point(px(0.), px(0.)), viewport);
        let mask = bounds.intersect(&viewport_bounds);
        let input = input_region(
            if self.definition.backdrop {
                viewport_bounds
            } else {
                mask
            },
            viewport,
        );
        window.set_input_region(Some(input.as_slice()));
        // GPUI's public Interactivity paint continuation applies subtree opacity.
        // Capturing .opacity() during Render would miss this first measured sample.
        self.opacity.base_style.opacity = Some(sample.opacity.unwrap_or(1.));
        #[cfg(test)]
        self.observed.set(Observation {
            content: Some(bounds),
            input,
            sample: Some(sample),
        });
        window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
            self.content.prepaint_at(bounds.origin, window, cx);
        });
        Some(PaintedContent { bounds, mask })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(state) = state else {
            return;
        };
        window.with_content_mask(Some(ContentMask { bounds: state.mask }), |window| {
            self.opacity.paint(
                None,
                None,
                state.bounds,
                None,
                window,
                cx,
                |_, window, cx| {
                    self.content.paint(window, cx);
                },
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transition;
    use crate::shell::{Bar, KeyboardInteractivity, Overlay, SurfaceHandle, Thickness};
    use bkss_core::{RuntimeHandle, Shell};
    use gpui::{AppContext, Size, TestAppContext, VisualTestContext};
    use std::{cell::RefCell, time::Duration};

    struct Content {
        clicks: Rc<Cell<usize>>,
        keys: Rc<Cell<usize>>,
        focus: Option<FocusHandle>,
        consume_escape: bool,
        renders: Rc<Cell<usize>>,
    }

    impl Render for Content {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            let clicks = self.clicks.clone();
            let mut root = div()
                .w(px(100.))
                .h(px(80.))
                .bg(gpui::rgb(0xffffff))
                .on_any_mouse_down(move |_, _, _| clicks.set(clicks.get() + 1));
            if let Some(focus) = &self.focus {
                let keys = self.keys.clone();
                let consume = self.consume_escape;
                root = root.track_focus(focus).on_key_down(move |event, _, cx| {
                    keys.set(keys.get() + 1);
                    if consume && event.keystroke.key == "escape" {
                        cx.stop_propagation();
                    }
                });
            }
            root
        }
    }

    struct Record {
        window: gpui::AnyWindowHandle,
        instance: SurfaceInstanceHandle,
        observed: Rc<Cell<Observation>>,
        clicks: Rc<Cell<usize>>,
        keys: Rc<Cell<usize>>,
        renders: Rc<Cell<usize>>,
    }

    fn mount(
        cx: &TestAppContext,
        runtime: &RuntimeHandle,
        definition: ShellSurface,
        focus_child: bool,
        consume_escape: bool,
    ) -> (SurfaceHandle, Rc<RefCell<Vec<Record>>>) {
        let records = Rc::new(RefCell::new(Vec::new()));
        let captured = records.clone();
        let observed = definition.observed.clone();
        let handle = cx.update(|cx| {
            definition
                .mount(
                    runtime,
                    move |instance, _, window, cx| {
                        let clicks = Rc::new(Cell::new(0));
                        let keys = Rc::new(Cell::new(0));
                        let renders = Rc::new(Cell::new(0));
                        let content = cx.new(|cx| {
                            let focus = focus_child.then(|| cx.focus_handle());
                            if let Some(focus) = &focus {
                                focus.focus(window, cx);
                            }
                            Content {
                                clicks: clicks.clone(),
                                keys: keys.clone(),
                                focus,
                                consume_escape,
                                renders: renders.clone(),
                            }
                        });
                        captured.borrow_mut().push(Record {
                            window: window.window_handle(),
                            instance,
                            observed: observed.clone(),
                            clicks,
                            keys,
                            renders,
                        });
                        Ok(content)
                    },
                    cx,
                )
                .unwrap()
        });
        let output = cx.update(|cx| cx.displays()[0].id());
        cx.update(|cx| handle.show(output, cx).unwrap());
        cx.run_until_parked();
        (handle, records)
    }

    fn overlay_definition(overlay: Overlay) -> ShellSurface {
        overlay.0
    }

    fn resize(
        cx: &TestAppContext,
        record: &Record,
        viewport: Size<Pixels>,
        scale: f32,
    ) -> VisualTestContext {
        cx.simulate_window_resize(record.window, viewport);
        let visual = VisualTestContext::from_window(record.window, cx);
        visual.simulate_scale_factor_change(scale);
        cx.run_until_parked();
        visual
    }

    #[test]
    fn first_measured_frame_starts_clock_and_applies_opacity_without_a_visible_flash() {
        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| Shell::attach(cx, |_, _| Ok(())).unwrap());
        let (_, records) = mount(
            &cx,
            &runtime,
            overlay_definition(
                Overlay::new().transition(
                    Transition::fade()
                        .and(Transition::slide_up())
                        .duration(Duration::from_secs(1)),
                ),
            ),
            false,
            false,
        );
        let records = records.borrow();
        let record = &records[0];
        cx.simulate_window_resize(record.window, size(px(0.), px(0.)));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(10));
        let mut visual = resize(&cx, record, size(px(400.), px(300.)), 1.);
        let observed = record.observed.get();
        let sample = observed.sample.unwrap();
        assert_eq!(sample.opacity, Some(0.));
        assert_eq!(sample.translation_y, Some(80.));
        assert_eq!(observed.content.unwrap().origin, point(px(150.), px(190.)));
        visual.update(|window, _| {
            assert!(
                window
                    .painted_quads()
                    .iter()
                    .all(|quad| { quad.background.as_solid().is_none_or(|color| color.a == 0.) })
            );
        });
        cx.executor().advance_clock(Duration::from_millis(500));
        visual.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        let sample = record.observed.get().sample.unwrap();
        assert!(sample.opacity.unwrap() > 0. && sample.opacity.unwrap() < 1.);
        assert!(record.renders.get() > 1);
        cx.update(|cx| runtime.shutdown(cx));
    }

    #[test]
    fn all_slide_directions_move_real_handlers_and_share_clipped_native_bounds_at_each_scale() {
        for scale in [1., 1.5, 2.] {
            for transition in [
                Transition::slide_up(),
                Transition::slide_down(),
                Transition::slide_left(),
                Transition::slide_right(),
            ] {
                let cx = TestAppContext::single();
                let runtime = cx.update(|cx| Shell::attach(cx, |_, _| Ok(())).unwrap());
                let (_, records) = mount(
                    &cx,
                    &runtime,
                    overlay_definition(
                        Overlay::new().transition(transition.duration(Duration::from_secs(1))),
                    ),
                    false,
                    false,
                );
                let records = records.borrow();
                let record = &records[0];
                let mut visual = resize(&cx, record, size(px(400.), px(300.)), scale);
                let observed = record.observed.get();
                let bounds = observed.content.unwrap();
                assert_eq!(
                    observed.input,
                    input_region(bounds, size(px(400.), px(300.)))
                );
                visual.simulate_mouse_down(
                    bounds.center(),
                    gpui::MouseButton::Left,
                    gpui::Modifiers::none(),
                );
                assert_eq!(record.clicks.get(), 1);
                visual.simulate_mouse_down(
                    point(px(200.), px(150.)),
                    gpui::MouseButton::Left,
                    gpui::Modifiers::none(),
                );
                assert_eq!(
                    record.clicks.get(),
                    1,
                    "stationary hitboxes must not survive translation"
                );
                cx.update(|cx| runtime.shutdown(cx));
            }
        }
    }

    #[test]
    fn translated_offscreen_content_has_no_input_but_explicit_backdrop_stays_stationary() {
        for backdrop in [false, true] {
            let cx = TestAppContext::single();
            let runtime = cx.update(|cx| Shell::attach(cx, |_, _| Ok(())).unwrap());
            let overlay = Overlay::new().transition(
                Transition::slide_left()
                    .distance(1000.)
                    .duration(Duration::from_secs(1)),
            );
            let overlay = if backdrop {
                overlay.dismiss_on_backdrop()
            } else {
                overlay
            };
            let (_, records) = mount(&cx, &runtime, overlay_definition(overlay), false, false);
            let records = records.borrow();
            let record = &records[0];
            let mut visual = resize(&cx, record, size(px(400.), px(300.)), 1.5);
            let observed = record.observed.get();
            assert_eq!(
                observed.input,
                backdrop.then(|| Bounds::new(point(px(0.), px(0.)), size(px(400.), px(300.))))
            );
            if backdrop {
                visual.update(|window, _| {
                    assert!(window.painted_quads().iter().any(|quad| {
                        quad.background
                            .as_solid()
                            .is_some_and(|color| (color.a - 0.4).abs() < 0.001)
                    }));
                });
            }
            visual.simulate_mouse_down(
                point(px(200.), px(150.)),
                gpui::MouseButton::Left,
                gpui::Modifiers::none(),
            );
            assert_eq!(record.clicks.get(), 0);
            cx.update(|cx| runtime.shutdown(cx));
        }
    }

    #[test]
    fn fractional_translation_uses_the_painted_device_snapped_origin_and_viewport_mask() {
        for scale in [1., 1.5, 2.] {
            let cx = TestAppContext::single();
            let runtime = cx.update(|cx| Shell::attach(cx, |_, _| Ok(())).unwrap());
            let (_, records) = mount(
                &cx,
                &runtime,
                overlay_definition(
                    Overlay::new().transition(
                        Transition::slide_left()
                            .distance(230.3)
                            .duration(Duration::from_secs(1)),
                    ),
                ),
                false,
                false,
            );
            let records = records.borrow();
            let record = &records[0];
            let mut visual = resize(&cx, record, size(px(400.), px(300.)), scale);
            let observed = record.observed.get();
            let bounds = observed.content.unwrap();
            assert_eq!(
                observed.input,
                input_region(bounds, size(px(400.), px(300.)))
            );
            let input = observed.input.unwrap();
            assert!(input.left() >= bounds.left());
            assert_eq!(input.right(), px(400.));
            visual.update(|window, _| {
                let quads = window.painted_quads();
                assert_eq!(quads.len(), 1);
                assert!(
                    (quads[0].bounds.origin.x.0 / scale - f32::from(bounds.left())).abs() < 0.001
                );
                assert_eq!(quads[0].content_mask.bounds.right().0 / scale, 400.);
            });
            visual.simulate_mouse_down(
                input.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::none(),
            );
            assert_eq!(record.clicks.get(), 1);
            visual.simulate_mouse_down(
                point(px(200.), px(150.)),
                gpui::MouseButton::Left,
                gpui::Modifiers::none(),
            );
            assert_eq!(record.clicks.get(), 1);
            cx.update(|cx| runtime.shutdown(cx));
        }
    }

    #[test]
    fn reusable_style_survives_redraws_and_remounts_and_multiplies_animation_opacity() {
        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| Shell::attach(cx, |_, _| Ok(())).unwrap());
        let (handle, records) = mount(
            &cx,
            &runtime,
            overlay_definition(
                Overlay::new()
                    .bg(gpui::rgb(0x225588))
                    .opacity(0.5)
                    .transition(Transition::fade().duration(Duration::from_millis(100))),
            ),
            false,
            false,
        );
        let output = cx.update(|cx| cx.displays()[0].id());
        for generation in 0..2 {
            if generation != 0 {
                cx.update(|cx| handle.show(output, cx).unwrap());
                cx.run_until_parked();
            }
            let captured = records.borrow();
            let record = &captured[generation];
            let mut visual = resize(&cx, record, size(px(400.), px(300.)), 1.);
            cx.executor().advance_clock(Duration::from_millis(50));
            visual.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
            let sample = record.observed.get().sample.unwrap();
            let alpha = sample.opacity.unwrap() * 0.5;
            assert!(alpha > 0. && alpha < 0.5);
            visual.update(|window, _| {
                let quads = window.painted_quads();
                assert_eq!(
                    quads.len(),
                    2,
                    "only the caller's root and content paint; no automatic Glass"
                );
                for quad in quads {
                    assert!((quad.background.as_solid().unwrap().a - alpha).abs() < 0.001);
                }
            });
            visual.update(|window, _| window.refresh());
            cx.run_until_parked();
            assert!(record.renders.get() > 1);
            cx.update(|cx| handle.hide(output, cx).unwrap());
            cx.executor().advance_clock(Duration::from_millis(100));
            visual.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
            assert!(cx.update(|cx| cx.windows().is_empty()));
        }
        cx.update(|cx| runtime.shutdown(cx));
    }

    #[test]
    fn backdrop_and_escape_bubble_after_children_and_none_has_no_default_key_route() {
        for (keyboard, consume, close) in [
            (KeyboardInteractivity::OnDemand, false, true),
            (KeyboardInteractivity::OnDemand, true, false),
            (KeyboardInteractivity::None, false, false),
        ] {
            let cx = TestAppContext::single();
            let runtime = cx.update(|cx| Shell::attach(cx, |_, _| Ok(())).unwrap());
            let (_, records) = mount(
                &cx,
                &runtime,
                overlay_definition(
                    Overlay::new()
                        .keyboard_interactivity(keyboard)
                        .dismiss_on_backdrop(),
                ),
                true,
                consume,
            );
            let records = records.borrow();
            let record = &records[0];
            let mut visual = resize(&cx, record, size(px(400.), px(300.)), 1.);
            visual.simulate_mouse_down(
                point(px(200.), px(150.)),
                gpui::MouseButton::Left,
                gpui::Modifiers::none(),
            );
            assert_eq!(record.clicks.get(), 1);
            assert_eq!(
                cx.update(|cx| cx.windows().len()),
                1,
                "inside click must not reach backdrop"
            );
            visual.simulate_keystrokes("escape");
            cx.run_until_parked();
            assert_eq!(record.keys.get(), 1);
            assert_eq!(cx.update(|cx| cx.windows().is_empty()), close);
            if !close {
                use gpui::InputEvent;
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
                assert!(consumed);
                cx.run_until_parked();
                assert!(cx.update(|cx| cx.windows().is_empty()));
            }
            cx.update(|cx| runtime.shutdown(cx));
        }
    }

    #[test]
    fn bar_content_styling_does_not_change_native_allocation_during_exit() {
        for edge in [
            super::super::Anchor::TOP,
            super::super::Anchor::BOTTOM,
            super::super::Anchor::LEFT,
            super::super::Anchor::RIGHT,
        ] {
            for scale in [1., 1.5, 2.] {
                let cx = TestAppContext::single();
                let runtime = cx.update(|cx| Shell::attach(cx, |_, _| Ok(())).unwrap());
                let bar = Bar::new(Thickness::new(px(40.)).unwrap())
                    .edge(edge)
                    .h(px(500.))
                    .w(px(500.))
                    .exit_transition(Transition::fade_out().duration(Duration::from_secs(1)));
                let definition = bar.0;
                let config = definition.config().unwrap();
                let (_, records) = mount(&cx, &runtime, definition.clone(), false, false);
                let records = records.borrow();
                let record = &records[0];
                let horizontal =
                    edge == super::super::Anchor::TOP || edge == super::super::Anchor::BOTTOM;
                let viewport = if horizontal {
                    size(px(400.), px(52.))
                } else {
                    size(px(52.), px(300.))
                };
                let mut visual = resize(&cx, record, viewport, scale);
                let bounds = record.observed.get().content.unwrap();
                assert!(if horizontal {
                    bounds.size.height <= px(40.)
                } else {
                    bounds.size.width <= px(40.)
                });
                cx.update(|cx| record.instance.close(cx).unwrap());
                cx.executor().advance_clock(Duration::from_millis(500));
                visual.update(|window, cx| window.simulate_next_frame(cx));
                cx.run_until_parked();
                assert_eq!(cx.update(|cx| cx.windows().len()), 1);
                assert_eq!(definition.config().unwrap().size, config.size);
                assert_eq!(
                    definition.config().unwrap().options.exclusive_zone,
                    config.options.exclusive_zone
                );
                assert_eq!(
                    record.observed.get().input,
                    input_region(record.observed.get().content.unwrap(), viewport)
                );
                cx.update(|cx| runtime.shutdown(cx));
            }
        }
    }
}
