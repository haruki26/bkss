use std::{cell::RefCell, io, process::ExitCode, rc::Rc};

use bkss_core::{
    Glass, GlassBorder, GlassColor, GlassInnerEffect, GlassPatch, GlassState, GlassStyle,
    GlassStyleError,
};
use gpui::{
    AppContext, Bounds, Context, Entity, FocusHandle, MouseButton, Render, Subscription, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};
use gpui_platform::application;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Press {
    Pointer,
    Key(&'static str),
}

#[derive(Default)]
struct InputState {
    hovered: bool,
    press: Option<Press>,
}

impl InputState {
    fn glass(&self, focused: bool) -> GlassState {
        GlassState {
            hovered: self.hovered,
            focused,
            pressed: focused
                && match self.press {
                    Some(Press::Pointer) => self.hovered,
                    Some(Press::Key(_)) => true,
                    None => false,
                },
        }
    }

    fn release(&mut self, source: Press, focused: bool) -> bool {
        if self.press != Some(source) {
            return false;
        }
        self.press = None;
        focused
            && match source {
                Press::Pointer => self.hovered,
                Press::Key(_) => true,
            }
    }

    fn blur(&mut self) {
        self.press = None;
    }
}

fn activation_key(key: &str) -> Option<&'static str> {
    match key {
        "space" | " " => Some("space"),
        "enter" => Some("enter"),
        _ => None,
    }
}

fn style() -> Result<GlassStyle, GlassStyleError> {
    Ok(GlassStyle::default()
        .with_tint(GlassColor::rgba(0.08, 0.22, 0.33, 0.75)?)
        .with_hover(GlassPatch::default().with_tint(GlassColor::rgba(0.18, 0.37, 0.48, 0.9)?))
        .with_pressed(
            GlassPatch::default()
                .with_tint(GlassColor::rgba(0.06, 0.15, 0.24, 0.95)?)
                .with_inner_shadow(Some(GlassInnerEffect::new(
                    GlassColor::rgba(0., 0., 0., 0.8)?,
                    [0., 3.],
                    8.,
                    2.,
                )?)),
        )
        .with_focus(GlassPatch::default().with_border(Some(GlassBorder::new(
            2.,
            GlassColor::rgba(0.6, 0.9, 1., 1.)?,
        )?))))
}

// This input wrapper belongs only to this example. Glass itself is decoration.
struct Control {
    label: &'static str,
    glass: Glass,
    focus: FocusHandle,
    input: InputState,
    activations: usize,
    _subscriptions: Vec<Subscription>,
}

impl Control {
    fn new(
        label: &'static str,
        style: GlassStyle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // track_focus uses this handle's tab metadata, not the div's tab_index.
        let focus = cx.focus_handle().tab_stop(true);
        let subscriptions = vec![
            cx.on_blur(&focus, window, |this, _, cx| {
                this.input.blur();
                cx.notify();
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.input.blur();
                    this.input.hovered = false;
                }
                cx.notify();
            }),
        ];
        Self {
            label,
            glass: Glass::new(style, cx),
            focus,
            input: InputState::default(),
            activations: 0,
            _subscriptions: subscriptions,
        }
    }

    fn is_focused(&self, window: &Window) -> bool {
        window.is_window_active() && self.focus.is_focused(window)
    }

    fn release(&mut self, source: Press, window: &Window, cx: &mut Context<Self>) {
        if self.input.release(source, self.is_focused(window)) {
            self.activations += 1;
        }
        cx.notify();
    }
}

impl Render for Control {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Focus callbacks run after paint; the current frame must read live focus.
        let state = self.input.glass(self.is_focused(window));
        div()
            .id(self.label)
            .role(gpui::Role::Button)
            .aria_label(self.label)
            .track_focus(&self.focus)
            .flex_1()
            .hover_listener_mode(gpui::HoverListenerMode::InputModalityIndependent)
            .on_hover(cx.listener(|this, hovered, _, cx| {
                this.input.hovered = *hovered;
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window, cx);
                    this.input.press = Some(Press::Pointer);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.release(Press::Pointer, window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    // Release outside cancels a pointer activation, even after dragging out.
                    if this.input.press == Some(Press::Pointer) {
                        this.input.press = None;
                        cx.notify();
                    }
                }),
            )
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if let Some(key) = activation_key(&event.keystroke.key) {
                    if !event.is_held && this.is_focused(window) {
                        // One source owns a press; another source never leaves a stale press behind.
                        this.input.press = Some(Press::Key(key));
                        cx.notify();
                    }
                    cx.stop_propagation();
                }
            }))
            .on_key_up(cx.listener(|this, event: &gpui::KeyUpEvent, window, cx| {
                if let Some(key) = activation_key(&event.keystroke.key) {
                    this.release(Press::Key(key), window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                self.glass
                    .element(state, cx)
                    .w_full()
                    .h(px(150.))
                    .p_5()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .child(div().text_size(px(22.)).child(self.label))
                    .child(div().text_size(px(13.)).child(format!(
                        "hover {} · pressed {} · focus {}",
                        state.hovered, state.pressed, state.focused
                    )))
                    .child(
                        div()
                            .text_size(px(14.))
                            .child(format!("Activations: {}", self.activations)),
                    ),
            )
    }
}

struct Demo {
    focus: FocusHandle,
    controls: [Entity<Control>; 2],
}

impl Render for Demo {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_8().bg(rgb(0x34495b)).text_color(rgb(0xf2f6fb)).font_family("sans-serif")
            .flex().flex_col().gap_4()
            .track_focus(&self.focus)
            // Keep navigation on the focus path after a background click blurs a control.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.focus.focus(window, cx)))
            .on_key_down(|event, window, cx| match event.keystroke.key.as_str() {
                "tab" => {
                    if event.keystroke.modifiers.shift { window.focus_prev(cx); } else { window.focus_next(cx); }
                    cx.stop_propagation();
                }
                "escape" => cx.quit(),
                _ => (),
            })
            .child(div().text_size(px(28.)).child("Glass input checks"))
            .child(div().text_size(px(14.)).child("An example-local wrapper sends explicit state flags. Glass does not handle input."))
            .child(div().flex().gap_4().children(self.controls.iter().cloned()))
            .child(div().text_size(px(14.)).child("Pointer: hover lightens the tint. Hold primary click to darken it; release inside adds one activation."))
            .child(div().text_size(px(14.)).child("Drag outside: pressed turns off. Release outside: no activation, no stuck press. Return inside to check."))
            .child(div().text_size(px(14.)).child("Keyboard: click a control or use Tab / Shift+Tab. A pale border marks focus, including while pressed."))
            .child(div().text_size(px(14.)).child("Hold Space or Enter: pressed is true. Release that key: one activation. Key repeat must not add more."))
            .child(div().text_size(px(14.)).child("Change focus while holding a key, click the background, or leave the window: pressed must reset."))
            .child(div().text_size(px(14.)).child("Mix pointer and keys: the latest press owns the state; releasing it must not revive an older press."))
            .child(div().text_size(px(12.)).child("Ordinary GPUI window · no layer-shell input claims · Escape closes this demo"))
    }
}

fn main() -> ExitCode {
    let style = match style() {
        Ok(style) => style,
        Err(error) => {
            eprintln!("glass_interactive: invalid example style: {error}");
            return ExitCode::FAILURE;
        }
    };
    let failure = Rc::new(RefCell::new(None));
    let app_failure = failure.clone();
    application().run(move |cx| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if let Err(error) = cx.open_window(
            WindowOptions {
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("BKSS Glass input checks".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(980.), px(660.)),
                    cx,
                ))),
                ..Default::default()
            },
            |window, cx| {
                let controls = [
                    cx.new(|cx| Control::new("First control", style.clone(), window, cx)),
                    cx.new(|cx| Control::new("Second control", style, window, cx)),
                ];
                let focus = controls[0].read(cx).focus.clone();
                focus.focus(window, cx);
                cx.new(|cx| Demo {
                    focus: cx.focus_handle(),
                    controls,
                })
            },
        ) {
            *app_failure.borrow_mut() = Some(io::Error::other(error));
            cx.quit();
        }
    });
    if let Some(error) = failure.borrow_mut().take() {
        eprintln!("glass_interactive: could not open window: {error}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        KeyDownEvent, KeyUpEvent, Keystroke, PlatformInput, TestAppContext, VisualTestContext,
    };

    #[derive(Debug, PartialEq)]
    struct FocusFrame {
        native: [bool; 2],
        painted: [bool; 2],
        pressed: [bool; 2],
    }

    fn focus_frame(cx: &mut VisualTestContext, controls: &[Entity<Control>; 2]) -> FocusFrame {
        cx.update(|window, cx| {
            let native = std::array::from_fn(|index| controls[index].read(cx).is_focused(window));
            let mut painted = [false; 2];
            let mut pressed = [false; 2];
            let pressed_tint: gpui::Hsla = gpui::Rgba {
                r: 0.06,
                g: 0.15,
                b: 0.24,
                a: 0.95,
            }
            .into();
            let midpoint = f32::from(window.bounds().size.width) * window.scale_factor() / 2.;
            // Only the explicit Glass focus patch paints borders in this demo.
            // Read the last actual scene, not a fresh render or the model flags.
            for quad in window.painted_quads() {
                let index = usize::from(quad.bounds.origin.x.0 >= midpoint);
                if quad.border_widths.top.0 > 0. && quad.border_color.a > 0. {
                    painted[index] = true;
                }
                if quad.background.as_solid() == Some(pressed_tint) {
                    pressed[index] = true;
                }
            }
            FocusFrame {
                native,
                painted,
                pressed,
            }
        })
    }

    #[test]
    fn launch_order_tab_paints_current_focus_without_another_input() {
        for (first_key, next_key) in [
            ("tab", "tab"),
            ("tab", "shift-tab"),
            ("shift-tab", "tab"),
            ("shift-tab", "shift-tab"),
            ("tab", "space"),
            ("tab", "enter"),
        ] {
            let mut cx = TestAppContext::single();
            let window = cx.update(|cx| {
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                            None,
                            size(px(980.), px(660.)),
                            cx,
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        let controls = [
                            cx.new(|cx| {
                                Control::new("First control", style().unwrap(), window, cx)
                            }),
                            cx.new(|cx| {
                                Control::new("Second control", style().unwrap(), window, cx)
                            }),
                        ];
                        // Match main: focus before constructing Demo or rendering its tree.
                        let focus = controls[0].read(cx).focus.clone();
                        focus.focus(window, cx);
                        cx.new(|cx| Demo {
                            focus: cx.focus_handle(),
                            controls,
                        })
                    },
                )
                .unwrap()
            });
            let mut visual = VisualTestContext::from_window(window.into(), &cx);
            visual.update(|window, _| window.activate_window());
            visual.run_until_parked();
            let demo = window.root(&mut cx).unwrap();
            let controls = visual.update(|_, cx| demo.read(cx).controls.clone());
            assert_eq!(
                focus_frame(&mut visual, &controls),
                FocusFrame {
                    native: [true, false],
                    painted: [true, false],
                    pressed: [false, false],
                }
            );

            assert!(key_down(&mut visual, first_key, false));
            let after_tab = focus_frame(&mut visual, &controls);
            assert_eq!(after_tab.native, [false, true]);
            assert_eq!(
                after_tab.painted, after_tab.native,
                "painted focus after {first_key}"
            );
            // Drain already scheduled work once more without sending new input.
            visual.run_until_parked();
            assert_eq!(focus_frame(&mut visual, &controls), after_tab);

            assert!(key_down(&mut visual, next_key, false));
            let after_next = focus_frame(&mut visual, &controls);
            assert_eq!(
                after_next.painted, after_next.native,
                "painted focus after {next_key}"
            );
            if matches!(next_key, "tab" | "shift-tab") {
                assert_eq!(after_next.native, [true, false]);
                assert_eq!(after_next.pressed, [false, false]);
            } else {
                assert_eq!(after_next.native, [false, true]);
                assert_eq!(after_next.pressed, [false, true]);
                key_up(&mut visual, next_key);
                assert_eq!(focus_frame(&mut visual, &controls).pressed, [false, false]);
                visual.update(|_, cx| {
                    assert_eq!(controls[0].read(cx).activations, 0);
                    assert_eq!(controls[1].read(cx).activations, 1);
                });
            }
        }
    }

    fn key_down(cx: &mut VisualTestContext, key: &str, is_held: bool) -> bool {
        let event = KeyDownEvent {
            keystroke: Keystroke::parse(key).unwrap(),
            is_held,
            prefer_character_input: false,
        };
        let consumed = cx.update(|window, cx| {
            !window
                .dispatch_event(PlatformInput::KeyDown(event), cx)
                .propagate
        });
        cx.run_until_parked();
        consumed
    }

    fn key_up(cx: &mut VisualTestContext, key: &str) {
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).unwrap(),
        });
    }

    fn assert_focus(
        cx: &mut VisualTestContext,
        controls: &[Entity<Control>; 2],
        focused: Option<usize>,
    ) {
        let frame = focus_frame(cx, controls);
        let expected = std::array::from_fn(|index| focused == Some(index));
        assert_eq!(frame.native, expected);
        assert_eq!(frame.painted, expected);
    }

    #[test]
    fn rendered_controls_follow_tab_order_and_keep_key_release_and_blur_behavior() {
        let mut cx = TestAppContext::single();
        let (demo, cx) = cx.add_window_view(|window, cx| Demo {
            focus: cx.focus_handle(),
            controls: [
                cx.new(|cx| Control::new("First control", style().unwrap(), window, cx)),
                cx.new(|cx| Control::new("Second control", style().unwrap(), window, cx)),
            ],
        });
        let controls = cx.update(|_, cx| demo.read(cx).controls.clone());
        cx.update(|window, cx| {
            window.activate_window();
            let focus = controls[0].read(cx).focus.clone();
            focus.focus(window, cx);
        });
        cx.run_until_parked();
        assert_focus(cx, &controls, Some(0));

        // Dispatch through the rendered focus path, not directly to focus_next.
        assert!(key_down(cx, "tab", false), "the Demo root must receive Tab");
        assert_focus(cx, &controls, Some(1));
        assert!(key_down(cx, "shift-tab", false));
        assert_focus(cx, &controls, Some(0));
        assert!(key_down(cx, "shift-tab", false));
        assert_focus(cx, &controls, Some(1));
        assert!(key_down(cx, "tab", false));
        assert_focus(cx, &controls, Some(0));

        for (count, key) in [(1, "space"), (2, "enter")] {
            assert!(key_down(cx, key, false));
            assert!(key_down(cx, key, true));
            assert!(key_down(cx, key, true));
            cx.update(|window, cx| {
                let control = controls[0].read(cx);
                assert!(control.input.glass(control.is_focused(window)).pressed);
                assert_eq!(control.activations, count - 1);
            });
            key_up(cx, key);
            key_up(cx, key);
            cx.update(|window, cx| {
                let control = controls[0].read(cx);
                assert!(!control.input.glass(control.is_focused(window)).pressed);
                assert_eq!(control.activations, count);
            });
        }

        assert!(key_down(cx, "space", false));
        assert!(key_down(cx, "tab", false));
        assert_focus(cx, &controls, Some(1));
        assert_eq!(focus_frame(cx, &controls).pressed, [false, false]);
        key_up(cx, "space");
        assert!(key_down(cx, "space", true));
        cx.update(|window, cx| {
            for control in &controls {
                let control = control.read(cx);
                assert!(!control.input.glass(control.is_focused(window)).pressed);
            }
            assert_eq!(controls[0].read(cx).activations, 2);
            assert_eq!(controls[1].read(cx).activations, 0);
        });

        assert!(key_down(cx, "enter", false));
        cx.simulate_click(gpui::point(px(12.), px(12.)), gpui::Modifiers::none());
        assert_focus(cx, &controls, None);
        key_up(cx, "enter");
        cx.update(|window, cx| {
            let control = controls[1].read(cx);
            assert!(!control.input.glass(control.is_focused(window)).pressed);
            assert_eq!(controls[1].read(cx).activations, 0);
        });
        assert!(key_down(cx, "tab", false));
        assert_focus(cx, &controls, Some(0));

        cx.simulate_click(gpui::point(px(12.), px(12.)), gpui::Modifiers::none());
        assert_focus(cx, &controls, None);
        assert!(key_down(cx, "shift-tab", false));
        assert_focus(cx, &controls, Some(1));
        assert!(key_down(cx, "tab", false));
        assert_focus(cx, &controls, Some(0));

        assert!(key_down(cx, "space", false));
        cx.deactivate_window();
        assert_focus(cx, &controls, None);
        assert_eq!(focus_frame(cx, &controls).pressed, [false, false]);
        cx.update(|window, cx| {
            let control = controls[0].read(cx);
            assert!(!control.input.glass(control.is_focused(window)).focused);
            assert!(!control.input.glass(control.is_focused(window)).pressed);
            assert_eq!(controls[0].read(cx).activations, 2);
        });
    }

    #[test]
    fn pointer_release_and_blur_do_not_leave_a_press() {
        let mut input = InputState {
            hovered: true,
            press: Some(Press::Pointer),
        };
        assert!(input.glass(true).pressed);
        input.hovered = false;
        assert!(!input.glass(true).pressed);
        assert!(!input.release(Press::Pointer, true));
        input.hovered = true;
        assert!(!input.glass(true).pressed);
        input.press = Some(Press::Key("space"));
        input.blur();
        assert!(!input.glass(false).focused);
        assert!(!input.glass(false).pressed);
        assert!(!input.release(Press::Key("space"), false));
    }

    #[test]
    fn live_focus_loss_hides_and_cancels_press_before_blur_cleanup() {
        for source in [Press::Pointer, Press::Key("space"), Press::Key("enter")] {
            let mut input = InputState {
                hovered: true,
                press: Some(source),
            };
            assert!(input.glass(true).pressed);
            // The cleanup callback has not run: only live focus changed.
            assert_eq!(input.press, Some(source));
            assert!(!input.glass(false).pressed);
            assert!(!input.glass(false).focused);
            assert!(!input.release(source, false));
            assert_eq!(input.press, None);
        }
    }

    #[test]
    fn latest_source_owns_the_press_and_only_matching_release_activates_once() {
        let mut input = InputState {
            hovered: true,
            press: Some(Press::Pointer),
        };
        input.press = Some(Press::Key("space"));
        assert!(!input.release(Press::Pointer, true));
        assert!(input.glass(true).pressed);
        input.press = Some(Press::Key("enter"));
        assert!(!input.release(Press::Key("space"), true));
        assert!(input.release(Press::Key("enter"), true));
        assert!(!input.release(Press::Key("enter"), true));
        assert!(!input.glass(true).pressed);
        assert!(style().is_ok());
    }
}
