use std::rc::Rc;

use bkss_ui::effects::{GlassState, MountedGlass};
use gpui::{
    Context, DisplayId, IntoElement, Render, Subscription, Window, div, prelude::*, px, rgb, rgba,
};

use super::{
    desktop::{Desktop, Panel, PanelLifetime},
    theme::{self, DANGER, Icon, button, icon, label, row},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    Reboot,
    Poweroff,
}

pub struct PowerView {
    desktop: Rc<Desktop>,
    display: DisplayId,
    glass: MountedGlass,
    confirmation: Option<Choice>,
    _feedback_subscription: Subscription,
    _lifetime: PanelLifetime,
}

impl PowerView {
    pub fn new(desktop: Rc<Desktop>, display: DisplayId, cx: &mut Context<Self>) -> Self {
        Self {
            display,
            glass: MountedGlass::new(desktop.power_style.clone(), cx),
            confirmation: None,
            _feedback_subscription: cx.observe(&desktop.feedback, |_, _, cx| cx.notify()),
            _lifetime: desktop.lifetime(Panel::Power, display),
            desktop,
        }
    }
}

impl Render for PowerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let display = self.display;
        let close = self.desktop.clone();
        let lock = self.desktop.clone();
        let reboot = cx.entity();
        let poweroff = cx.entity();
        let cancel = cx.entity();
        let confirm = cx.entity();
        let mut content = div().flex().flex_col().gap_4().child(
            row()
                .justify_between()
                .child(
                    label("Power Menu")
                        .text_size(px(16.))
                        .font_weight(gpui::FontWeight::SEMIBOLD),
                )
                .child(
                    button(
                        "close-power",
                        "電源メニューを閉じる",
                        move |_, cx| close.close(Panel::Power, display, cx),
                    )
                    .child(icon(Icon::Close)),
                ),
        );
        if let Some(choice) = self.confirmation {
            content = content
                .child(
                    label(if choice == Choice::Poweroff {
                        "システムを終了しますか？"
                    } else {
                        "再起動しますか？"
                    })
                    .text_size(px(16.)),
                )
                .child(theme::muted("保存していない作業がないか確認してください。"))
                .child(
                    row()
                        .justify_end()
                        .child(
                            button("cancel-power", "キャンセル", move |_, cx| {
                                cancel.update(cx, |view, cx| {
                                    view.confirmation = None;
                                    cx.notify();
                                })
                            })
                            .child("キャンセル"),
                        )
                        .child(
                            button(
                                "confirm-power",
                                if choice == Choice::Poweroff {
                                    "システムを終了"
                                } else {
                                    "システムを再起動"
                                },
                                move |_, cx| {
                                    confirm.update(cx, |view, cx| {
                                        if view.confirmation != Some(choice) {
                                            return;
                                        }
                                        view.confirmation = None;
                                        if choice == Choice::Poweroff {
                                            view.desktop.poweroff(cx);
                                        } else {
                                            view.desktop.reboot(cx);
                                        }
                                        cx.notify();
                                    })
                                },
                            )
                            .bg(rgba(0xf38ba82c))
                            .text_color(rgb(DANGER))
                            .child(if choice == Choice::Poweroff {
                                "終了"
                            } else {
                                "再起動"
                            }),
                        ),
                );
        } else {
            content = content.child(
                row()
                    .gap_3()
                    .child(
                        button("power-off", "システムを終了", move |_, cx| {
                            poweroff.update(cx, |view, cx| {
                                view.confirmation = Some(Choice::Poweroff);
                                cx.notify();
                            })
                        })
                        .flex_1()
                        .h(px(80.))
                        .flex_col()
                        .gap_3()
                        .bg(rgba(0x31324480))
                        .text_color(rgb(DANGER))
                        .text_size(px(11.))
                        .child(icon(Icon::Power).size(px(24.)))
                        .child("Shutdown"),
                    )
                    .child(
                        button("power-reboot", "再起動", move |_, cx| {
                            reboot.update(cx, |view, cx| {
                                view.confirmation = Some(Choice::Reboot);
                                cx.notify();
                            })
                        })
                        .flex_1()
                        .h(px(80.))
                        .flex_col()
                        .gap_3()
                        .bg(rgba(0x31324480))
                        .text_color(rgb(theme::ACCENT))
                        .text_size(px(11.))
                        .child(icon(Icon::Reboot))
                        .child("Reboot"),
                    )
                    .child(
                        button("power-lock", "画面をロック", move |_, cx| {
                            lock.lock(cx);
                            lock.close(Panel::Power, display, cx);
                        })
                        .flex_1()
                        .h(px(80.))
                        .flex_col()
                        .gap_3()
                        .bg(rgba(0x31324480))
                        .text_color(rgb(theme::SECONDARY))
                        .text_size(px(11.))
                        .child(icon(Icon::Lock).size(px(24.)))
                        .child("Lock"),
                    ),
            );
        }
        self.desktop
            .panel_root(Panel::Power, display, window)
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "s" | "S" => view.confirmation = Some(Choice::Poweroff),
                    "r" | "R" => view.confirmation = Some(Choice::Reboot),
                    "l" | "L" => {
                        view.desktop.lock(cx);
                        view.desktop.close(Panel::Power, view.display, cx);
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                self.glass
                    .element(GlassState::default(), cx)
                    .w(px(420.))
                    .max_w_full()
                    .p_4()
                    .child(content)
                    .children(theme::error_banner(&self.desktop.feedback, cx)),
            )
    }
}
