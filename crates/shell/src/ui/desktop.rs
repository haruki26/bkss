use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io,
    rc::{Rc, Weak},
    time::Duration,
};

use bkss_core::{RuntimeHandle, Shell, ShellError};
use bkss_integration::{TrayItem, TrayItemId};
use bkss_ui::{
    Anchor, Bar, DisplaySelection, KeyboardInteractivity, Overlay, Placement, SurfaceHandle,
    Thickness, Transition, effects::GlassStyle,
};
use gpui::{
    AnyElement, App, AppContext, DisplayId, Div, Edges, Entity, Stateful, Window, div, img, point,
    prelude::*, px, rgb, size,
};

use crate::{actions::Actions, services::Services};

use super::{
    bars::{BottomBar, TopBar},
    calendar::CalendarView,
    commands::CommandListener,
    external_icon::IconIndex,
    feedback::{self, Feedback},
    icons::{self, Icon},
    notifications::{NotificationFeed, NotificationView, notification_panel_limit},
    power::PowerView,
    theme,
    tray::TrayView,
};

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub enum Panel {
    Notifications,
    Banners,
    Calendar,
    Power,
    Tray,
}

pub struct Desktop {
    pub services: Services,
    pub actions: Actions,
    pub feedback: Entity<Feedback>,
    pub bar_style: GlassStyle,
    pub panel_style: GlassStyle,
    pub calendar_style: GlassStyle,
    pub power_style: GlassStyle,
    icons: IconIndex,
    handles: RefCell<HashMap<(DisplayId, Panel), SurfaceHandle>>,
    open: RefCell<HashSet<(DisplayId, Panel)>>,
    feeds: RefCell<HashMap<DisplayId, Entity<NotificationFeed>>>,
    commands: RefCell<Option<Entity<CommandListener>>>,
    pub selected_tray: RefCell<HashMap<DisplayId, TrayItemId>>,
}

pub struct PanelLifetime {
    desktop: Weak<Desktop>,
    panel: Panel,
    display: DisplayId,
}

impl Drop for PanelLifetime {
    fn drop(&mut self) {
        if let Some(desktop) = self.desktop.upgrade() {
            desktop
                .open
                .borrow_mut()
                .remove(&(self.display, self.panel));
        }
    }
}

impl Desktop {
    fn ensure_panels(
        self: &Rc<Self>,
        display: DisplayId,
        runtime: &RuntimeHandle,
        cx: &mut App,
    ) -> Result<(), ShellError> {
        let missing = {
            let handles = self.handles.borrow();
            missing_panels(|panel| handles.contains_key(&(display, panel)))
        };
        for panel in missing {
            register_panel(self, panel, display, runtime, cx)?;
        }
        Ok(())
    }

    pub fn lifetime(self: &Rc<Self>, panel: Panel, display: DisplayId) -> PanelLifetime {
        PanelLifetime {
            desktop: Rc::downgrade(self),
            panel,
            display,
        }
    }

    pub fn is_open(&self, panel: Panel, display: DisplayId) -> bool {
        self.open.borrow().contains(&(display, panel))
    }

    pub fn toggle(self: &Rc<Self>, panel: Panel, display: DisplayId, cx: &mut App) {
        if self.is_open(panel, display) {
            self.close(panel, display, cx);
        } else {
            self.show(panel, display, cx);
        }
    }

    pub fn toggle_power_on_outputs(self: &Rc<Self>, cx: &mut App) -> io::Result<()> {
        let outputs = ready_power_outputs(self.handles.borrow().keys().copied())?;
        let close = outputs
            .iter()
            .any(|display| self.is_open(Panel::Power, *display));
        for display in outputs {
            if close {
                self.close(Panel::Power, display, cx);
            } else {
                self.show(Panel::Power, display, cx);
            }
        }
        Ok(())
    }

    pub fn show(self: &Rc<Self>, panel: Panel, display: DisplayId, cx: &mut App) {
        if panel != Panel::Banners {
            for other in [
                Panel::Notifications,
                Panel::Calendar,
                Panel::Power,
                Panel::Tray,
            ] {
                if other != panel {
                    self.close(other, display, cx);
                }
            }
        }
        if panel == Panel::Notifications {
            let feed = self.feeds.borrow().get(&display).cloned();
            if let Some(feed) = feed {
                feed.update(cx, |feed, cx| feed.hide_all_banners(cx));
            }
        }
        if let Some(handle) = self.handles.borrow().get(&(display, panel)).cloned() {
            match handle.show(display, cx) {
                Ok(()) => {
                    self.open.borrow_mut().insert((display, panel));
                    self.feedback.update(cx, |_, cx| cx.notify());
                }
                Err(error) => {
                    feedback::fail(&self.feedback, format!("画面を開けません: {error}"), cx)
                }
            }
        }
    }

    pub fn close(&self, panel: Panel, display: DisplayId, cx: &mut App) {
        self.open.borrow_mut().remove(&(display, panel));
        self.feedback.update(cx, |_, cx| cx.notify());
        if let Some(handle) = self.handles.borrow().get(&(display, panel)).cloned()
            && let Err(error) = handle.hide(display, cx)
        {
            feedback::fail(&self.feedback, format!("画面を閉じられません: {error}"), cx);
        }
    }

    pub fn panel_root(
        self: &Rc<Self>,
        panel: Panel,
        display: DisplayId,
        window: &Window,
    ) -> Stateful<Div> {
        let desktop = self.clone();
        let width = match panel {
            Panel::Calendar => 320.,
            Panel::Power => 420.,
            Panel::Tray => 300.,
            Panel::Notifications | Panel::Banners => theme::PANEL_WIDTH,
        };
        div()
            .id("panel-root")
            // BKSS measures with MaxContent; percentage-only roots leave
            // their children's percentage max-width circular.
            .w(px(width).min((window.viewport_size().width - px(24.)).max(px(1.))))
            .max_h(
                (window.viewport_size().height - px(24.))
                    .max(px(1.))
                    .min(px(theme::PANEL_MAX_HEIGHT)),
            )
            .overflow_y_scroll()
            .font_family(theme::FONT)
            .text_size(px(12.))
            .text_color(rgb(theme::TEXT))
            .on_key_down(move |event, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    desktop.close(panel, display, cx);
                }
            })
    }

    pub fn icon_path(&self, name: &str) -> Option<std::path::PathBuf> {
        self.icons.path(name)
    }

    pub fn app_icon(&self, name: &str, size: f32) -> AnyElement {
        if let Some(path) = self.icons.path(name) {
            img(path)
                .size(px(size))
                .flex_shrink_0()
                .with_fallback(move || icons::icon(Icon::App).size(px(size)).into_any_element())
                .into_any_element()
        } else {
            icons::icon(Icon::App).size(px(size)).into_any_element()
        }
    }

    pub fn app_icon_for_class(&self, class: &str, size: f32) -> AnyElement {
        self.app_icon(self.icons.app_icon_name(class), size)
    }

    pub fn switch_workspace(&self, name: &str, cx: &mut App) {
        feedback::dispatch(
            self.actions.switch_workspace(name),
            "ワークスペースを切り替えられません",
            &self.feedback,
            cx,
        );
    }

    pub fn lock(&self, cx: &mut App) {
        feedback::dispatch(
            Ok::<_, io::Error>(self.actions.lock()),
            "画面をロックできません",
            &self.feedback,
            cx,
        );
    }

    pub fn reboot(&self, cx: &mut App) {
        feedback::dispatch(
            Ok::<_, io::Error>(self.actions.reboot()),
            "再起動できません",
            &self.feedback,
            cx,
        );
    }

    pub fn poweroff(&self, cx: &mut App) {
        feedback::dispatch(
            Ok::<_, io::Error>(self.actions.poweroff()),
            "終了できません",
            &self.feedback,
            cx,
        );
    }

    pub fn open_tray(self: &Rc<Self>, display: DisplayId, item: TrayItem, cx: &mut App) {
        if item.menu_path.is_none() {
            feedback::dispatch(
                item.context_menu(0, 0),
                "メニューを開けません",
                &self.feedback,
                cx,
            );
            return;
        }
        self.selected_tray
            .borrow_mut()
            .insert(display, item.id.clone());
        let feedback = self.feedback.clone();
        super::tray::refresh_menu(&item, 0, &feedback, cx);
        self.show(Panel::Tray, display, cx);
        cx.refresh_windows();
    }

    fn feed(
        self: &Rc<Self>,
        display: DisplayId,
        runtime: &RuntimeHandle,
        cx: &mut App,
    ) -> Result<Entity<NotificationFeed>, ShellError> {
        if let Some(feed) = self.feeds.borrow().get(&display) {
            return Ok(feed.clone());
        }
        let feed = NotificationFeed::mount(self, display, runtime, cx)?;
        self.feeds.borrow_mut().insert(display, feed.clone());
        Ok(feed)
    }

    pub fn command_listener(
        self: &Rc<Self>,
        runtime: &RuntimeHandle,
        cx: &mut App,
    ) -> Result<Entity<CommandListener>, ShellError> {
        if let Some(listener) = self.commands.borrow().as_ref() {
            return Ok(listener.clone());
        }
        // CommandIpcState clones share a consuming stream, not a broadcast stream.
        let binding = runtime.mount(&self.services.power_menu_requests, cx)?;
        let listener = CommandListener::new(self, binding, cx);
        self.commands.borrow_mut().replace(listener.clone());
        Ok(listener)
    }
}

pub fn run() -> Result<(), ShellError> {
    Shell::run(move |runtime, cx| {
        let services = Services::new().map_err(ShellError::setup)?;
        let actions = Actions::new(services.hyprland_socket_dir.clone());
        let icons = IconIndex::load();
        let bar_style = theme::glass_style(20.).map_err(ShellError::setup)?;
        let panel_style = theme::glass_style(22.).map_err(ShellError::setup)?;
        let calendar_style = theme::glass_style(10.).map_err(ShellError::setup)?;
        let power_style = theme::glass_style(14.).map_err(ShellError::setup)?;
        let desktop = Rc::new(Desktop {
            services,
            actions,
            feedback: cx.new(|_| Feedback::default()),
            bar_style,
            panel_style,
            calendar_style,
            power_style,
            icons,
            handles: RefCell::new(HashMap::new()),
            open: RefCell::new(HashSet::new()),
            feeds: RefCell::new(HashMap::new()),
            commands: RefCell::new(None),
            selected_tray: RefCell::new(HashMap::new()),
        });
        // Wayland display discovery can still be empty here; ready bars own panel registration.
        let top = desktop.clone();
        Bar::new(Thickness::new(px(theme::BAR_HEIGHT)).map_err(ShellError::setup)?)
            .namespace("bkss-top")
            .edge(Anchor::TOP)
            .full_length()
            .px_0()
            .margins(Edges {
                top: px(theme::MARGIN),
                bottom: px(theme::MARGIN),
                left: px(16.),
                right: px(16.),
            })
            .outputs(DisplaySelection::AllConnected)
            .mount(
                &runtime,
                move |instance, weak, _, cx| {
                    let runtime = weak.upgrade().ok_or_else(stopped)?;
                    let display = instance.display_id();
                    top.ensure_panels(display, &runtime, cx)?;
                    TopBar::mount(top.clone(), display, &runtime, cx)
                },
                cx,
            )?;
        let bottom = desktop.clone();
        Bar::new(Thickness::new(px(theme::BAR_HEIGHT)).map_err(ShellError::setup)?)
            .namespace("bkss-bottom")
            .edge(Anchor::BOTTOM)
            .full_length()
            .px_0()
            .margins(Edges {
                top: px(theme::MARGIN),
                bottom: px(theme::MARGIN),
                left: px(20.),
                right: px(20.),
            })
            .outputs(DisplaySelection::AllConnected)
            .mount(
                &runtime,
                move |instance, weak, _, cx| {
                    let runtime = weak.upgrade().ok_or_else(stopped)?;
                    let display = instance.display_id();
                    bottom.ensure_panels(display, &runtime, cx)?;
                    let feed = bottom.feed(display, &runtime, cx)?;
                    BottomBar::mount(bottom.clone(), display, feed, &runtime, cx)
                },
                cx,
            )?;
        Ok(())
    })
}

fn ready_power_outputs(
    registered: impl IntoIterator<Item = (DisplayId, Panel)>,
) -> io::Result<Vec<DisplayId>> {
    let outputs: Vec<_> = registered
        .into_iter()
        .filter_map(|(display, panel)| (panel == Panel::Power).then_some(display))
        .collect();
    if outputs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "power menu output is not ready",
        ));
    }
    Ok(outputs)
}

fn missing_panels(mut is_registered: impl FnMut(Panel) -> bool) -> Vec<Panel> {
    [
        Panel::Notifications,
        Panel::Banners,
        Panel::Calendar,
        Panel::Power,
        Panel::Tray,
    ]
    .into_iter()
    .filter(|panel| !is_registered(*panel))
    .collect()
}

fn stopped() -> ShellError {
    ShellError::setup(io::Error::other("shell runtime stopped"))
}

fn register_panel(
    desktop: &Rc<Desktop>,
    panel: Panel,
    display: DisplayId,
    runtime: &RuntimeHandle,
    cx: &mut App,
) -> Result<(), ShellError> {
    let placement = match panel {
        Panel::Calendar => Placement::Anchor {
            edges: Anchor::BOTTOM | Anchor::RIGHT,
            offset: point(px(-12.), px(-12.)),
        },
        Panel::Power => Placement::Center,
        _ => Placement::Anchor {
            edges: Anchor::TOP | Anchor::RIGHT,
            offset: point(px(-12.), px(12.)),
        },
    };
    let namespace = match panel {
        Panel::Notifications => "bkss-notifications",
        Panel::Banners => "bkss-banners",
        Panel::Calendar => "bkss-calendar",
        Panel::Power => "bkss-power",
        Panel::Tray => "bkss-tray",
    };
    let declaration = Overlay::new()
        .namespace(namespace)
        .output(display)
        .placement(placement)
        .content_limit(size(px(440.), px(theme::PANEL_MAX_HEIGHT)))
        .keyboard_interactivity(if panel == Panel::Banners {
            KeyboardInteractivity::None
        } else {
            KeyboardInteractivity::OnDemand
        });
    let declaration = match panel {
        Panel::Notifications => {
            let display = cx.find_display(display).ok_or_else(|| {
                ShellError::setup(io::Error::new(
                    io::ErrorKind::NotFound,
                    "notification display unavailable",
                ))
            })?;
            declaration
                .surface_size(notification_panel_limit(display.bounds().size))
                .anchor(Anchor::TOP | Anchor::RIGHT)
                .margin(Edges {
                    top: px(12.),
                    right: px(12.),
                    bottom: px(0.),
                    left: px(0.),
                })
                .placement(Placement::Anchor {
                    edges: Anchor::TOP | Anchor::LEFT,
                    offset: point(px(0.), px(0.)),
                })
        }
        Panel::Calendar => declaration
            .enter_transition(Transition::slide_left().duration(Duration::from_millis(260)))
            .exit_transition(Transition::slide_left().duration(Duration::from_millis(220))),
        // Each banner owns its own motion; the surface must not animate the stack.
        Panel::Banners => declaration,
        Panel::Power => declaration.transition(
            Transition::slide_up()
                .distance(8.)
                .duration(Duration::from_millis(190)),
        ),
        Panel::Tray => declaration.transition(
            Transition::slide_left()
                .distance(18.)
                .duration(Duration::from_millis(190)),
        ),
    };
    let captured = desktop.clone();
    let handle = match panel {
        Panel::Notifications | Panel::Banners => declaration.mount(
            runtime,
            move |_, weak, window, cx| {
                let runtime = weak.upgrade().ok_or_else(stopped)?;
                let feed = captured.feed(display, &runtime, cx)?;
                Ok(cx.new(|cx| {
                    NotificationView::new(
                        captured.clone(),
                        display,
                        feed,
                        panel == Panel::Banners,
                        window,
                        cx,
                    )
                }))
            },
            cx,
        )?,
        Panel::Calendar => declaration.mount(
            runtime,
            move |_, _, _, cx| Ok(cx.new(|cx| CalendarView::new(captured.clone(), display, cx))),
            cx,
        )?,
        Panel::Power => declaration.mount(
            runtime,
            move |_, _, _, cx| Ok(cx.new(|cx| PowerView::new(captured.clone(), display, cx))),
            cx,
        )?,
        Panel::Tray => declaration.mount(
            runtime,
            move |_, weak, _, cx| {
                let runtime = weak.upgrade().ok_or_else(stopped)?;
                TrayView::mount(captured.clone(), display, &runtime, cx)
            },
            cx,
        )?,
    };
    desktop
        .handles
        .borrow_mut()
        .insert((display, panel), handle);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_bar_registers_panels_after_an_empty_startup_display_snapshot() {
        let mut registered = HashSet::new();
        assert_eq!(
            ready_power_outputs(registered.iter().copied())
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotConnected
        );
        let first_ready = DisplayId::new(7);
        let pending = missing_panels(|panel| registered.contains(&(first_ready, panel)));
        assert_eq!(pending.len(), 5);
        assert!(pending.contains(&Panel::Power));

        registered.insert((first_ready, Panel::Notifications));
        assert!(ready_power_outputs(registered.iter().copied()).is_err());
        assert_eq!(
            missing_panels(|panel| registered.contains(&(first_ready, panel))).len(),
            4
        );
        for panel in pending {
            registered.insert((first_ready, panel));
        }
        assert_eq!(registered.len(), 5);
        assert_eq!(
            ready_power_outputs(registered.iter().copied()).unwrap(),
            [first_ready]
        );
        assert!(missing_panels(|panel| registered.contains(&(first_ready, panel))).is_empty());

        let second_ready = DisplayId::new(8);
        assert_eq!(
            missing_panels(|panel| registered.contains(&(second_ready, panel))).len(),
            5
        );
    }
}
