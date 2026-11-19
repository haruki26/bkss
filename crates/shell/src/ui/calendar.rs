use std::rc::Rc;

use bkss_ui::effects::{GlassState, MountedGlass};
use chrono::{Datelike, Local, NaiveDate};
use gpui::{Context, DisplayId, IntoElement, Render, Window, div, prelude::*, px, rgb, rgba};

use crate::calendar_model::{current_month, month_grid, shift_month};

use super::{
    desktop::{Desktop, Panel, PanelLifetime},
    theme::{self, DATA_FONT, Icon, button, icon, row},
};

pub struct CalendarView {
    desktop: Rc<Desktop>,
    display: DisplayId,
    glass: MountedGlass,
    month: NaiveDate,
    selected: NaiveDate,
    _lifetime: PanelLifetime,
}

impl CalendarView {
    pub fn new(desktop: Rc<Desktop>, display: DisplayId, cx: &mut Context<Self>) -> Self {
        let today = Local::now().date_naive();
        Self {
            display,
            glass: MountedGlass::new(desktop.calendar_style.clone(), cx),
            month: current_month(),
            selected: today,
            _lifetime: desktop.lifetime(Panel::Calendar, display),
            desktop,
        }
    }

    fn select(&mut self, date: NaiveDate, cx: &mut Context<Self>) {
        self.selected = date;
        if date.month() != self.month.month() || date.year() != self.month.year() {
            self.month = date.with_day(1).unwrap_or(date);
        }
        cx.notify();
    }
}

impl Render for CalendarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let previous = entity.clone();
        let next = entity.clone();
        let today = entity.clone();
        let close = self.desktop.clone();
        let display = self.display;
        let now = Local::now().date_naive();
        let mut weeks = div().flex().flex_col().gap_1();
        for (week_index, week) in month_grid(self.month, now).into_iter().enumerate() {
            let mut days = row().gap_1();
            for (day_index, day) in week.into_iter().enumerate() {
                let entity = entity.clone();
                days = days.child(
                    button(
                        ("day", week_index * 7 + day_index),
                        day.date.format("%Y年%-m月%-d日").to_string(),
                        move |_, cx| entity.update(cx, |view, cx| view.select(day.date, cx)),
                    )
                    .flex_1()
                    .h(px(32.))
                    .px_0()
                    .rounded_md()
                    .text_size(px(13.))
                    .font_family(DATA_FONT)
                    .aria_selected(day.date == self.selected)
                    .text_color(rgb(match day_index {
                        0 => theme::DANGER,
                        6 => theme::BLUE,
                        _ => theme::TEXT,
                    }))
                    .when(day.is_other_month, |s| s.opacity(0.5))
                    .when(day.is_today, |s| {
                        s.bg(rgba(0xcba6f72e)).text_color(rgb(theme::PRIMARY))
                    })
                    .when(day.date == self.selected, |s| {
                        s.opacity(1.)
                            .bg(rgb(theme::PRIMARY))
                            .text_color(rgb(0x11111b))
                    })
                    .child(day.day.to_string()),
                );
            }
            weeks = weeks.child(days);
        }
        let weekdays = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
            .into_iter()
            .enumerate()
            .map(|(index, day)| {
                theme::muted(day)
                    .flex_1()
                    .text_center()
                    .text_size(px(11.))
                    .text_color(rgb(match index {
                        0 => theme::DANGER,
                        6 => theme::BLUE,
                        _ => theme::TEXT,
                    }))
            });

        self.desktop
            .panel_root(Panel::Calendar, display, window)
            .child(
                self.glass
                    .element(GlassState::default(), cx)
                    .w(px(320.))
                    .max_w_full()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        row()
                            .justify_between()
                            .child(
                                button("today", "今日に戻る", move |_, cx| {
                                    today.update(cx, |view, cx| {
                                        view.select(Local::now().date_naive(), cx);
                                    })
                                })
                                .child(self.month.format("%B %Y").to_string()),
                            )
                            .child(
                                row()
                                    .gap_1()
                                    .child(
                                        button("previous-month", "前の月", move |_, cx| {
                                            previous.update(cx, |view, cx| {
                                                view.month = shift_month(view.month, -1);
                                                cx.notify();
                                            })
                                        })
                                        .child(icon(Icon::Left)),
                                    )
                                    .child(
                                        button("next-month", "次の月", move |_, cx| {
                                            next.update(cx, |view, cx| {
                                                view.month = shift_month(view.month, 1);
                                                cx.notify();
                                            })
                                        })
                                        .child(icon(Icon::Right)),
                                    )
                                    .child(
                                        button(
                                            "close-calendar",
                                            "カレンダーを閉じる",
                                            move |_, cx| close.close(Panel::Calendar, display, cx),
                                        )
                                        .child(icon(Icon::Close)),
                                    ),
                            ),
                    )
                    .child(row().gap_1().children(weekdays))
                    .child(weeks),
            )
    }
}
