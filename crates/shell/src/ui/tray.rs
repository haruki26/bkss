use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use bkss_core::{ReactiveSubscription, RuntimeHandle, ShellError};
use bkss_integration::{TrayItem, TrayItemId, TrayMenuItem, TrayState, TrayToggleState};
use bkss_ui::effects::{GlassState, MountedGlass};
use gpui::{
    App, AppContext, Context, DisplayId, Entity, IntoElement, Render, Subscription, Window, div,
    prelude::*, px, rgb, rgba,
};

use super::{
    desktop::{Desktop, Panel, PanelLifetime},
    live::Live,
    theme::{self, Icon, button, icon, label, row},
};

#[derive(Default)]
pub struct RefreshFeedback {
    generation: u64,
    requests: HashMap<(TrayItemId, i32), (u64, Option<String>)>,
}

impl RefreshFeedback {
    fn begin(&mut self, key: (TrayItemId, i32), cached: bool) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.requests.remove(&key);
        if !cached {
            self.requests.insert(key, (self.generation, None));
        }
        self.generation
    }

    fn finish(&mut self, key: &(TrayItemId, i32), generation: u64, error: Option<String>) {
        if let Some((current, message)) = self.requests.get_mut(key)
            && *current == generation
        {
            if error.is_some() {
                *message = error;
            } else {
                self.requests.remove(key);
            }
        }
    }

    fn message(&self, owner: &TrayItemId) -> Option<&str> {
        self.requests
            .iter()
            .filter(|((id, _), _)| id == owner)
            .filter_map(|((_, node), (_, message))| {
                message.as_deref().map(|message| (*node, message))
            })
            .min_by_key(|(node, _)| *node)
            .map(|(_, message)| message)
    }

    fn clear_available(&mut self, available: impl Fn(&TrayItemId, i32) -> bool) -> bool {
        let before = self.requests.len();
        self.requests.retain(|(id, node), _| !available(id, *node));
        before != self.requests.len()
    }
}

fn usable_items(items: &[TrayMenuItem]) -> bool {
    items.iter().any(|item| {
        item.visible
            && !item.separator
            && (item.enabled && !item.label.is_empty() || usable_items(&item.children))
    })
}

fn usable_menu(item: &TrayItem, node: i32) -> bool {
    fn children(items: &[TrayMenuItem], node: i32) -> Option<&[TrayMenuItem]> {
        for item in items {
            if item.id == node {
                return Some(&item.children);
            }
            if let Some(found) = children(&item.children, node) {
                return Some(found);
            }
        }
        None
    }
    item.menu.as_ref().is_some_and(|menu| {
        let items = if node == 0 {
            Some(menu.items.as_slice())
        } else {
            children(&menu.items, node)
        };
        items.is_some_and(usable_items)
    })
}

pub fn refresh_menu(item: &TrayItem, node: i32, feedback: &Entity<theme::Feedback>, cx: &mut App) {
    let key = (item.id.clone(), node);
    let generation = feedback.update(cx, |feedback, cx| {
        let generation = feedback
            .tray_refresh
            .begin(key.clone(), usable_menu(item, node));
        cx.notify();
        generation
    });
    match item.about_to_show(node) {
        Ok(operation) => {
            let feedback = feedback.downgrade();
            cx.spawn(async move |cx| {
                let error = operation
                    .await
                    .err()
                    .map(|error| format!("メニューを更新できません: {error}"));
                cx.update(|cx| {
                    if let Some(feedback) = feedback.upgrade() {
                        feedback.update(cx, |feedback, cx| {
                            feedback.tray_refresh.finish(&key, generation, error);
                            cx.notify();
                        });
                    }
                });
            })
            .detach();
        }
        Err(error) => feedback.update(cx, |feedback, cx| {
            feedback.tray_refresh.finish(
                &key,
                generation,
                Some(format!("メニューを更新できません: {error}")),
            );
            cx.notify();
        }),
    }
}

pub struct TrayView {
    desktop: Rc<Desktop>,
    display: DisplayId,
    glass: MountedGlass,
    tray: Live<TrayState>,
    expanded: HashSet<i32>,
    _tray_subscription: ReactiveSubscription,
    _feedback_subscription: Subscription,
    _lifetime: PanelLifetime,
}

impl TrayView {
    pub fn mount(
        desktop: Rc<Desktop>,
        display: DisplayId,
        runtime: &RuntimeHandle,
        cx: &mut App,
    ) -> Result<Entity<Self>, ShellError> {
        let tray = runtime.mount(&desktop.services.tray, cx)?;
        let view = cx.new(|cx: &mut Context<Self>| Self {
            display,
            glass: MountedGlass::new(desktop.panel_style.clone(), cx),
            _tray_subscription: tray.reactive().observe(cx, |view, _, cx| {
                view.sync_refresh_feedback(cx);
                cx.notify();
            }),
            tray: Live::new(tray, cx),
            expanded: HashSet::new(),
            _feedback_subscription: cx.observe(&desktop.feedback, |_, _, cx| cx.notify()),
            _lifetime: desktop.lifetime(Panel::Tray, display),
            desktop,
        });
        view.update(cx, |view, cx| view.sync_refresh_feedback(cx));
        Ok(view)
    }

    fn sync_refresh_feedback(&self, cx: &mut Context<Self>) {
        if let Some(state) = self.tray.get(cx) {
            self.desktop.feedback.update(cx, |feedback, cx| {
                if feedback.tray_refresh.clear_available(|id, node| {
                    state
                        .items
                        .iter()
                        .find(|item| &item.id == id)
                        .is_none_or(|item| usable_menu(item, node))
                }) {
                    cx.notify();
                }
            });
        }
    }

    fn menu_rows(
        &self,
        owner: &TrayItem,
        items: &[TrayMenuItem],
        depth: usize,
        cx: &Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        for item in items.iter().filter(|item| item.visible) {
            if item.separator {
                rows.push(
                    div()
                        .h(px(1.))
                        .my_1()
                        .bg(rgba(0xffffff18))
                        .into_any_element(),
                );
                continue;
            }
            let id = item.id;
            let entity = cx.entity();
            let action_owner = owner.clone();
            let has_children = !item.children.is_empty();
            let enabled = item.enabled;
            let checked = item.toggle_state == Some(TrayToggleState::On);
            let expanded = self.expanded.contains(&id);
            let desktop = self.desktop.clone();
            let text = item.label.replace('_', "");
            let menu_button = button(("tray-menu-item", id as u64), text.clone(), move |_, cx| {
                if !enabled {
                    return;
                }
                if has_children {
                    entity.update(cx, |view, cx| {
                        if !view.expanded.remove(&id) {
                            view.expanded.insert(id);
                        }
                        cx.notify();
                    });
                    refresh_menu(&action_owner, id, &desktop.feedback, cx);
                } else {
                    theme::dispatch(
                        action_owner.menu_event(id, "clicked", 0),
                        "メニューの操作を実行できません",
                        &desktop.feedback,
                        cx,
                    );
                }
            })
            .w_full()
            .justify_start()
            .pl(px(8. + depth as f32 * 12.))
            .when(has_children, |view| view.aria_expanded(expanded))
            .when(!enabled, |view| {
                view.tab_stop(false)
                    .cursor_default()
                    .aria_description("利用できません")
                    .text_color(rgb(0x6c7086))
            })
            .child(label(if checked { "✓" } else { "" }).w(px(12.)))
            .child(label(text).flex_1().min_w_0().truncate())
            .when(has_children, |view| {
                view.child(icon(if expanded { Icon::Left } else { Icon::Right }).size(px(12.)))
            });
            rows.push(menu_button.into_any_element());
            if expanded {
                rows.extend(self.menu_rows(owner, &item.children, depth + 1, cx));
            }
        }
        rows
    }
}

impl Render for TrayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self
            .desktop
            .selected_tray
            .borrow()
            .get(&self.display)
            .cloned();
        let item = self.tray.get(cx).and_then(|state| {
            state
                .items
                .into_iter()
                .find(|item| Some(&item.id) == selected.as_ref())
        });
        let close = self.desktop.clone();
        let refresh_error = item
            .as_ref()
            .filter(|item| !usable_menu(item, 0))
            .and_then(|item| {
                self.desktop
                    .feedback
                    .read(cx)
                    .tray_refresh
                    .message(&item.id)
            })
            .map(str::to_owned);
        let display = self.display;
        let title = item
            .as_ref()
            .map(|item| item.title.clone())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "トレイ".into());
        let mut content = div().flex().flex_col().gap_2().child(
            row()
                .justify_between()
                .child(label(title).truncate().flex_1())
                .child(
                    button("close-tray", "メニューを閉じる", move |_, cx| {
                        close.close(Panel::Tray, display, cx)
                    })
                    .child(icon(Icon::Close)),
                ),
        );
        if let Some(item) = item {
            if let Some(menu) = &item.menu {
                content = content.child(
                    div()
                        .id("tray-menu-scroll")
                        .max_h(
                            (window.viewport_size().height - px(120.))
                                .max(px(80.))
                                .min(px(theme::PANEL_MAX_HEIGHT - 70.)),
                        )
                        .overflow_y_scroll()
                        .children(self.menu_rows(&item, &menu.items, 0, cx)),
                );
            } else {
                let feedback = self.desktop.feedback.clone();
                content = content
                    .child(theme::muted("表示できるメニューはありません"))
                    .child(
                        button(
                            "tray-native-menu",
                            "アプリケーションのメニューを開く",
                            move |_, cx| {
                                theme::dispatch(
                                    item.context_menu(0, 0),
                                    "メニューを開けません",
                                    &feedback,
                                    cx,
                                );
                            },
                        )
                        .child("アプリケーションで開く"),
                    );
            }
        } else {
            content = content.child(theme::muted("このトレイ項目は終了しました"));
        }
        self.desktop.panel_root(Panel::Tray, display, window).child(
            self.glass
                .element(GlassState::default(), cx)
                .w(px(300.))
                .max_w_full()
                .p_3()
                .child(content)
                .children(
                    refresh_error
                        .map(|message| theme::muted(message).text_color(rgb(theme::DANGER))),
                )
                .children(theme::error_banner(&self.desktop.feedback, cx)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_refresh_feedback_is_scoped_and_cleared_by_cached_or_fresh_menu_data() {
        let id = TrayItemId {
            owner: "app".into(),
            path: "/tray".into(),
        };
        let other = TrayItemId {
            owner: "other".into(),
            path: "/tray".into(),
        };
        let key = (id.clone(), 0);
        let mut feedback = theme::Feedback {
            error: Some("unrelated operation failure".into()),
            ..Default::default()
        };
        let refresh = &mut feedback.tray_refresh;
        let stale = refresh.begin(key.clone(), false);
        let current = refresh.begin(key.clone(), false);
        refresh.finish(&key, stale, Some("stale failure".into()));
        assert_eq!(refresh.message(&id), None);
        refresh.finish(&key, current, Some("refresh failure".into()));
        assert_eq!(refresh.message(&id), Some("refresh failure"));
        let other_key = (other.clone(), 0);
        let other_token = refresh.begin(other_key.clone(), false);
        refresh.finish(&other_key, other_token, Some("other failure".into()));
        assert!(refresh.clear_available(|owner, _| owner == &id));
        refresh.finish(
            &key,
            current,
            Some("late failure after data arrived".into()),
        );
        assert_eq!(refresh.message(&id), None);
        assert_eq!(refresh.message(&other), Some("other failure"));
        let cached = refresh.begin(key.clone(), true);
        refresh.finish(&key, cached, Some("optional cached refresh failure".into()));
        assert_eq!(refresh.message(&id), None);
        let success = refresh.begin(other_key.clone(), false);
        refresh.finish(&other_key, success, None);
        assert_eq!(refresh.message(&other), None);
        assert_eq!(
            feedback.error.as_deref(),
            Some("unrelated operation failure")
        );
    }

    #[test]
    fn separators_and_unusable_menu_rows_do_not_hide_refresh_failures() {
        let mut item = TrayMenuItem {
            id: 1,
            label: "Open".into(),
            visible: true,
            enabled: true,
            separator: false,
            icon_name: None,
            icon_data: None,
            toggle_type: None,
            toggle_state: None,
            children: Vec::new(),
        };
        assert!(usable_items(&[item.clone()]));
        item.enabled = false;
        assert!(!usable_items(&[item.clone()]));
        item.enabled = true;
        item.separator = true;
        assert!(!usable_items(&[item.clone()]));
        item.separator = false;
        item.visible = false;
        assert!(!usable_items(&[item]));
        assert!(!usable_items(&[]));
    }
}
