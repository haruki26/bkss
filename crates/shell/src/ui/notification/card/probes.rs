use super::*;
use crate::ui::{notification::center, shared::button};
use gpui::{
    AppContext, Bounds, Context, IntoElement, Pixels, Render, Size, Window, canvas, div, px, rgb,
    size,
};
use std::{cell::Cell, rc::Rc, time::Duration};

enum ProbePanel {
    Popup,
    Center,
    PopupBaseline,
}

struct CardProbe {
    width: Pixels,
    clickable: bool,
    panel: ProbePanel,
    summary: String,
    description: String,
    bounds: Rc<Cell<[Bounds<Pixels>; 2]>>,
}

impl Render for CardProbe {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let measure = |element: gpui::Div, index| {
            let captured = self.bounds.clone();
            element.relative().child(
                canvas(
                    move |bounds, _, _| {
                        let mut rows = captured.get();
                        rows[index] = bounds;
                        captured.set(rows);
                    },
                    |_, _, _, _| (),
                )
                .absolute()
                .inset_0(),
            )
        };
        let center = matches!(self.panel, ProbePanel::Center);
        let limit = size(self.width, px(760.));
        // The center's current viewport can disagree with its fixed layout limit.
        let width = if center { self.width / 2. } else { self.width };
        let mut panel = div().id("card-probe").w(width).overflow_y_scroll();
        let description = if matches!(self.panel, ProbePanel::PopupBaseline) {
            label(self.description.clone())
                .text_color(rgb(theme::TEXT))
                .line_clamp(2)
        } else {
            let width = notification_body_width(&mut panel, center.then_some(self.width), window);
            notification_body(self.description.clone(), width)
        };
        let body = notification_text_column(
            measure(label(self.summary.clone()), 0),
            Some(measure(description, 1)),
        );
        let body = if self.clickable {
            button("default-action", "Open notification", |_, _| ())
                .w_full()
                .justify_start()
                .px_0()
                .py_0()
                .child(body)
                .into_any_element()
        } else {
            body.into_any_element()
        };
        let card = div().child(div().w_full().p(CARD_PADDING).flex().flex_col().child(body));
        if center {
            let content = div().flex().flex_col().w_full().child(
                div()
                    .id("probe-list")
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(div().flex().flex_col().child(card)),
            );
            center::notification_surface(panel.child(content), limit)
        } else {
            panel.child(card)
        }
    }
}

fn measure_card(
    cx: &mut gpui::TestAppContext,
    width: Pixels,
    clickable: bool,
    panel: ProbePanel,
    summary: &str,
    description: &str,
) -> [Bounds<Pixels>; 2] {
    let bounds = Rc::new(Cell::new([Bounds::default(); 2]));
    let handle = cx.add_window(|_, _| CardProbe {
        width,
        clickable,
        panel,
        summary: summary.into(),
        description: description.into(),
        bounds: bounds.clone(),
    });
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    bounds.get()
}

#[test]
fn popup_body_measures_two_rows_at_the_padded_card_width() {
    let mut cx = gpui::TestAppContext::single();
    for clickable in [false, true] {
        let measure = |cx: &mut _, width| {
            measure_card(
                cx,
                px(width),
                clickable,
                ProbePanel::Popup,
                "Summary",
                "abcdefghijklm",
            )[1]
            .size
        };
        let narrow = measure(&mut cx, 48.);
        let wide = measure(&mut cx, 160.);
        assert_eq!(narrow.width, px(16.));
        assert!(wide.width <= px(128.));
        assert_eq!(narrow.height, wide.height * 2);
    }
}

#[test]
fn clickable_popup_keeps_the_baseline_title_and_constrains_only_the_body() {
    let mut cx = gpui::TestAppContext::single();
    let title = "a".repeat(52);
    let reference = measure_card(
        &mut cx,
        px(380.),
        true,
        ProbePanel::PopupBaseline,
        &title,
        "short",
    );
    let short = measure_card(&mut cx, px(380.), true, ProbePanel::Popup, &title, "short");
    let wrap = measure_card(
        &mut cx,
        px(380.),
        true,
        ProbePanel::Popup,
        &title,
        &"b".repeat(48),
    );
    assert_eq!(reference[0].size, size(px(406.), px(21.)));
    assert_eq!(short[0], reference[0]);
    assert_eq!(wrap[0], reference[0]);
    assert_eq!(short[1].size, size(px(348.), px(21.)));
    assert_eq!(wrap[1].size, size(px(348.), px(42.)));
}

#[test]
fn center_body_uses_its_fixed_width_and_matches_popup_line_budget() {
    let mut cx = gpui::TestAppContext::single();
    let title = "a".repeat(52);
    let one_row = "b".repeat(44);
    let wraps = format!("{}\nlater", "b".repeat(88));
    for clickable in [false, true] {
        for (body, height) in [(one_row.as_str(), 21.), (wraps.as_str(), 42.)] {
            let width = px(380.);
            let center = measure_card(&mut cx, width, clickable, ProbePanel::Center, &title, body);
            let popup = measure_card(&mut cx, width, clickable, ProbePanel::Popup, &title, body);
            assert_eq!(center[1].size, size(px(348.), px(height)));
            assert_eq!(center[1].size, popup[1].size);
            if clickable {
                assert_eq!(center[0].size, size(px(406.), px(21.)));
            }
        }
    }
}

struct PanelProbe {
    height: f32,
    limit: Size<Pixels>,
    glass: bkss_ui::effects::MountedGlass,
    panel: gpui::ScrollHandle,
    scroll: gpui::ScrollHandle,
}

impl PanelProbe {
    fn new(height: f32, limit: Size<Pixels>, cx: &mut Context<Self>) -> Self {
        Self {
            height,
            limit,
            glass: bkss_ui::effects::MountedGlass::new(
                crate::ui::theme::glass_style(22.).unwrap(),
                cx,
            ),
            panel: gpui::ScrollHandle::new(),
            scroll: gpui::ScrollHandle::new(),
        }
    }
}

impl Render for PanelProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .id("probe-list")
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .max_h(self.limit.height - px(40.))
            .flex()
            .flex_col()
            .gap_4()
            .pb_2();
        if self.height == 180. {
            list = list.child(div().h(px(self.height)).flex_shrink_0());
        } else {
            let cards = if self.height > 760. { 10 } else { 4 };
            let height = (self.height - 16. * (cards - 1) as f32) / cards as f32;
            for _ in 0..cards {
                list = list.child(
                    self.glass
                        .element(bkss_ui::effects::GlassState::default(), cx)
                        .w_full()
                        .h(px(height))
                        .flex_shrink_0(),
                );
            }
        }
        div().child(center::notification_surface(
            div()
                .id("probe-root")
                .overflow_y_scroll()
                .track_scroll(&self.panel)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w_full()
                        .child(div().h(px(28.)))
                        .child(list),
                ),
            self.limit,
        ))
    }
}

#[test]
#[ignore = "requires live Hyprland and Wayland; creates a separate transparent test layer"]
fn native_surface_follows_measured_content_after_initial_configures() {
    use std::{cell::RefCell, process::Command};

    fn layer_size(namespace: &str) -> Result<Size<Pixels>, String> {
        let output = Command::new("hyprctl")
            .args(["-j", "layers"])
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        let layers: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
        layers
            .as_object()
            .into_iter()
            .flat_map(|outputs| outputs.values())
            .filter_map(|output| output["levels"].as_object())
            .flat_map(|levels| levels.values())
            .filter_map(|level| level.as_array())
            .flatten()
            .find(|layer| layer["namespace"] == namespace)
            .and_then(|layer| {
                Some(size(
                    px(layer["w"].as_f64()? as f32),
                    px(layer["h"].as_f64()? as f32),
                ))
            })
            .ok_or_else(|| format!("test layer not found: {namespace}"))
    }

    let outcome = Rc::new(RefCell::new(None::<Result<(), String>>));
    let captured = outcome.clone();
    let namespace = format!("notification-size-test-{}", std::process::id());
    bkss_core::Shell::run(move |runtime, cx| {
        bkss_ui::Overlay::new()
            .namespace(namespace.clone())
            .surface_size(size(px(380.), px(760.)))
            .anchor(bkss_ui::Anchor::TOP | bkss_ui::Anchor::RIGHT)
            .placement(bkss_ui::Placement::Anchor {
                edges: bkss_ui::Anchor::TOP | bkss_ui::Anchor::LEFT,
                offset: gpui::point(px(0.), px(0.)),
            })
            .initially_visible(true)
            .mount(
                &runtime,
                move |_, _, window, cx| {
                    let limit = window.viewport_size();
                    let probe = cx.new(|cx| PanelProbe::new(180., limit, cx));
                    let probe_task = probe.clone();
                    let handle = window.window_handle();
                    let namespace = namespace.clone();
                    let outcome = captured.clone();
                    cx.spawn(async move |cx| {
                        let mut result = Ok(());
                        for (index, height) in
                            [180., 180., 420., 1100., 180.].into_iter().enumerate()
                        {
                            if index == 1 {
                                // Reproduce a configure restoring the old native size.
                                handle
                                    .update(cx, |_, window, _| window.resize(limit))
                                    .unwrap();
                            }
                            probe_task.update(cx, |probe, cx| {
                                probe.height = height;
                                cx.notify();
                            });
                            cx.background_executor()
                                .timer(Duration::from_millis(500))
                                .await;
                            let expected = size(limit.width, px(height + 48.).min(limit.height));
                            match layer_size(&namespace) {
                                Ok(actual) if actual == expected => {}
                                actual => {
                                    result = Err(format!(
                                        "content={height}, expected={expected:?}, native={actual:?}"
                                    ));
                                    break;
                                }
                            }
                        }
                        *outcome.borrow_mut() = Some(result);
                        cx.update(|cx| cx.quit());
                    })
                    .detach();
                    Ok(probe)
                },
                cx,
            )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(*outcome.borrow(), Some(Ok(())));
}
