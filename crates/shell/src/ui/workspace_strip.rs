use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use bkss_core::{MountedBinding, ReactiveSubscription, SourceState};
use bkss_integration::{ActiveWorkspace, HyprlandClient, HyprlandState};
use bkss_ui::effects::{GlassState, MountedGlass};
use gpui::{
    Animation, AnimationExt, AnyElement, App, Context, Div, IntoElement, Render, ScrollHandle,
    SpringAnimation, SpringConfig, Window, canvas, div, ease_in_out, point, prelude::*, px, rgb,
    rgba,
};

use super::{
    bars::{client_is_on_workspace, ordered_workspaces, workspace_is_active, workspace_target},
    desktop::Desktop,
    shared::{button_with_hover, muted},
    theme,
};

const ICON_SIZE: f32 = 18.;
const ICON_CELL: f32 = 24.;
const PILL_GAP: f32 = 6.;
const PADDING: f32 = 12.;
const INACTIVE_HEIGHT: f32 = 36.;
const TITLE_LIMIT: f32 = 280.;
const FADE: Duration = Duration::from_millis(180);

#[derive(Clone, Copy)]
struct Presence {
    id: u64,
    revision: u64,
    live: bool,
    from: f32,
    value: f32,
}

impl Presence {
    fn new(id: u64) -> Self {
        Self {
            id,
            revision: 0,
            live: true,
            from: 0.,
            value: 0.,
        }
    }

    fn target(&mut self, live: bool) {
        if self.live != live {
            self.revision = self.revision.wrapping_add(1);
            self.from = self.value;
            self.live = live;
        }
    }

    fn sample(self, progress: f32) -> f32 {
        let target = if self.live { 1. } else { 0. };
        self.from + (target - self.from) * progress.clamp(0., 1.)
    }

    fn animate<E: IntoElement + 'static>(
        self,
        element: E,
        duration: Duration,
        reports: Rc<RefCell<Vec<Report>>>,
        apply: impl Fn(E, f32) -> E + 'static,
    ) -> AnyElement {
        element
            .with_animation(
                gpui::ElementId::Name(format!("presence-{}-{}", self.id, self.revision).into()),
                Animation::new(duration).with_easing(ease_in_out),
                move |element, progress| {
                    let value = self.sample(progress);
                    reports
                        .borrow_mut()
                        .push(Report::Presence(self.id, self.revision, value));
                    apply(element, value)
                },
            )
            .into_any_element()
    }
}

#[derive(Clone)]
struct WindowIcon {
    client: HyprlandClient,
    presence: Presence,
}

#[derive(Clone)]
struct Title {
    text: String,
    presence: Presence,
}

#[derive(Clone)]
struct Pill {
    workspace: ActiveWorkspace,
    presence: Presence,
    active: bool,
    windows: Vec<WindowIcon>,
    titles: Vec<Title>,
    width: f32,
    target_width: f32,
}

struct DesiredPill {
    workspace: ActiveWorkspace,
    active: bool,
    windows: Vec<HyprlandClient>,
    title: String,
}

fn workspace_address(workspace: &ActiveWorkspace) -> &str {
    match workspace {
        ActiveWorkspace::Normal(workspace) => &workspace.address,
        ActiveWorkspace::Special(workspace) => &workspace.address,
    }
}

fn workspace_name(workspace: &ActiveWorkspace) -> &str {
    match workspace {
        ActiveWorkspace::Normal(workspace) => &workspace.name,
        ActiveWorkspace::Special(workspace) => &workspace.name,
    }
}

fn visual_label(workspace: &ActiveWorkspace) -> Option<&str> {
    match workspace {
        ActiveWorkspace::Normal(workspace) => Some(&workspace.name),
        ActiveWorkspace::Special(_) => None,
    }
}

fn pill_base_width(label_width: Option<f32>, windows: usize) -> f32 {
    PADDING * 2.
        + label_width.unwrap_or(0.)
        + windows as f32 * ICON_CELL
        + if label_width.is_some() && windows > 0 {
            8.
        } else {
            0.
        }
}

fn same_workspace(left: &ActiveWorkspace, right: &ActiveWorkspace) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
        && workspace_address(left) == workspace_address(right)
}

fn desired(state: &HyprlandState) -> (Vec<DesiredPill>, Option<(String, String)>) {
    let mut workspaces = state.workspaces.clone();
    if let Some(ActiveWorkspace::Normal(active)) = &state.active_workspace
        && !workspaces
            .iter()
            .any(|workspace| workspace.address == active.address)
    {
        workspaces.push(active.clone());
    }
    let mut focus = None;
    let mut pills = Vec::new();
    let mut candidates: Vec<_> = ordered_workspaces(&workspaces)
        .into_iter()
        .map(|workspace| {
            (
                ActiveWorkspace::Normal(workspace.clone()),
                workspace_is_active(workspace, state.active_workspace.as_ref()),
            )
        })
        .collect();
    if let Some(ActiveWorkspace::Special(active)) = &state.active_workspace {
        candidates.push((ActiveWorkspace::Special(active.clone()), true));
    }
    for (workspace, active) in candidates {
        let mut windows: Vec<_> = state
            .clients
            .iter()
            .filter(|client| match &workspace {
                ActiveWorkspace::Normal(workspace) => client_is_on_workspace(client, workspace),
                ActiveWorkspace::Special(workspace) => {
                    client.workspace_address.as_deref() == Some(workspace.address.as_str())
                }
            })
            .cloned()
            .collect();
        windows.sort_by(|left, right| left.address.cmp(&right.address));
        windows.dedup_by(|left, right| left.address == right.address);
        if !active && windows.is_empty() {
            continue;
        }
        // Separate Hyprland replies may straddle a focus/move event; never attach
        // a global title or indicator without an exact membership match.
        let title = state
            .active_window
            .as_ref()
            .filter(|window| {
                active
                    && windows
                        .iter()
                        .any(|client| client.address == window.address)
            })
            .map(|window| {
                focus = Some((
                    workspace_address(&workspace).to_owned(),
                    window.address.clone(),
                ));
                single_line(&window.title)
            })
            .unwrap_or_default();
        pills.push(DesiredPill {
            workspace,
            active,
            windows,
            title,
        });
    }
    (pills, focus)
}

#[derive(Default)]
struct Model {
    pills: Vec<Pill>,
    next_id: u64,
    revision: u64,
    focus: Option<(String, String)>,
    active: Option<String>,
}

enum Report {
    Presence(u64, u64, f32),
    Width(u64, f32, f32),
}

impl Model {
    fn switch_target(&self, id: u64) -> Option<String> {
        self.pills
            .iter()
            .find(|pill| pill.presence.id == id && pill.presence.live)
            .and_then(|pill| match &pill.workspace {
                ActiveWorkspace::Normal(workspace) => Some(workspace_target(workspace)),
                ActiveWorkspace::Special(_) => None,
            })
    }

    fn presence(&mut self) -> Presence {
        self.next_id = self.next_id.wrapping_add(1);
        Presence::new(self.next_id)
    }

    fn reconcile(&mut self, state: &HyprlandState) {
        self.revision = self.revision.wrapping_add(1);
        let (desired, focus) = desired(state);
        self.focus = focus;
        self.active = desired
            .iter()
            .find(|pill| pill.active)
            .map(|pill| workspace_address(&pill.workspace).to_owned());
        for pill in &mut self.pills {
            if !desired
                .iter()
                .any(|next| same_workspace(&next.workspace, &pill.workspace))
            {
                pill.presence.target(false);
                pill.active = false;
                for icon in &mut pill.windows {
                    icon.presence.target(false);
                }
                for title in &mut pill.titles {
                    title.presence.target(false);
                }
            }
        }
        for next in desired {
            let index = match self
                .pills
                .iter()
                .position(|pill| same_workspace(&pill.workspace, &next.workspace))
            {
                Some(index) => index,
                None => {
                    let presence = self.presence();
                    self.pills.push(Pill {
                        workspace: next.workspace.clone(),
                        presence,
                        active: next.active,
                        windows: Vec::new(),
                        titles: Vec::new(),
                        width: 0.,
                        target_width: 0.,
                    });
                    self.pills.len() - 1
                }
            };
            let pill = &mut self.pills[index];
            pill.workspace = next.workspace;
            pill.active = next.active;
            pill.presence.target(true);
            for icon in &mut pill.windows {
                icon.presence.target(
                    next.windows
                        .iter()
                        .any(|client| client.address == icon.client.address),
                );
            }
            for client in next.windows {
                if let Some(icon) = self.pills[index]
                    .windows
                    .iter_mut()
                    .find(|icon| icon.client.address == client.address)
                {
                    icon.client = client;
                } else {
                    let presence = self.presence();
                    self.pills[index]
                        .windows
                        .push(WindowIcon { client, presence });
                }
            }
            self.pills[index]
                .windows
                .sort_by(|left, right| left.client.address.cmp(&right.client.address));
            for title in &mut self.pills[index].titles {
                title
                    .presence
                    .target(!next.title.is_empty() && title.text == next.title);
            }
            if !next.title.is_empty()
                && !self.pills[index]
                    .titles
                    .iter()
                    .any(|title| title.text == next.title)
            {
                let presence = self.presence();
                self.pills[index].titles.push(Title {
                    text: next.title,
                    presence,
                });
            }
        }
        self.pills
            .sort_by(|left, right| match (&left.workspace, &right.workspace) {
                (ActiveWorkspace::Normal(left), ActiveWorkspace::Normal(right)) => (
                    left.id.is_none(),
                    left.id,
                    &left.name,
                    &left.address,
                )
                    .cmp(&(right.id.is_none(), right.id, &right.name, &right.address)),
                (ActiveWorkspace::Special(left), ActiveWorkspace::Special(right)) => {
                    (&left.name, &left.address).cmp(&(&right.name, &right.address))
                }
                (ActiveWorkspace::Normal(_), ActiveWorkspace::Special(_)) => {
                    std::cmp::Ordering::Less
                }
                (ActiveWorkspace::Special(_), ActiveWorkspace::Normal(_)) => {
                    std::cmp::Ordering::Greater
                }
            });
    }

    fn report(&mut self, report: Report) {
        match report {
            Report::Width(id, width, target) => {
                if let Some(pill) = self.pills.iter_mut().find(|pill| pill.presence.id == id) {
                    pill.width = width;
                    pill.target_width = target;
                }
            }
            Report::Presence(id, revision, value) => {
                for pill in &mut self.pills {
                    let motions = std::iter::once(&mut pill.presence)
                        .chain(pill.windows.iter_mut().map(|icon| &mut icon.presence))
                        .chain(pill.titles.iter_mut().map(|title| &mut title.presence));
                    for motion in motions {
                        if motion.id == id && motion.revision == revision {
                            motion.value = value;
                            return;
                        }
                    }
                }
            }
        }
    }

    fn finish(&mut self) -> bool {
        let before = self
            .pills
            .iter()
            .map(|pill| 1 + pill.windows.len() + pill.titles.len())
            .sum::<usize>();
        for pill in &mut self.pills {
            pill.windows
                .retain(|icon| icon.presence.live || icon.presence.value > 0.);
            pill.titles
                .retain(|title| title.presence.live || title.presence.value > 0.);
        }
        self.pills
            .retain(|pill| pill.presence.live || pill.presence.value > 0. || pill.width > 0.25);
        before
            != self
                .pills
                .iter()
                .map(|pill| 1 + pill.windows.len() + pill.titles.len())
                .sum::<usize>()
    }
}

pub struct WorkspaceStrip {
    desktop: Rc<Desktop>,
    binding: MountedBinding<HyprlandState>,
    _subscription: ReactiveSubscription,
    glass: MountedGlass,
    model: Model,
    scroll: ScrollHandle,
    reveal: bool,
    available: f32,
    indicator: Option<f32>,
    loading: bool,
}

impl WorkspaceStrip {
    pub fn new(
        desktop: Rc<Desktop>,
        binding: MountedBinding<HyprlandState>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            _subscription: binding
                .reactive()
                .observe(cx, |view, _, cx| view.reconcile(cx)),
            binding,
            glass: MountedGlass::new(desktop.bar_style.clone(), cx),
            desktop,
            model: Model::default(),
            scroll: ScrollHandle::new(),
            reveal: true,
            available: 0.,
            indicator: None,
            loading: true,
        };
        view.reconcile(cx);
        view
    }

    fn reconcile(&mut self, cx: &mut Context<Self>) {
        if let SourceState::Ready(state) = self.binding.reactive().get(cx) {
            self.loading = false;
            let previous = (self.model.active.clone(), self.model.focus.clone());
            self.model.reconcile(&state);
            if previous != (self.model.active.clone(), self.model.focus.clone()) {
                self.reveal = true;
            }
            cx.notify();
        }
    }
}

fn spring() -> SpringConfig {
    SpringConfig::new(300., 35., 1.)
}

fn single_line(text: &str) -> String {
    text.replace(['\n', '\r', '\t'], " ")
}

fn text_width(window: &Window, text: &str, bold: bool) -> f32 {
    let style = gpui::TextStyle {
        font_family: theme::FONT.into(),
        font_weight: if bold {
            gpui::FontWeight::SEMIBOLD
        } else {
            gpui::FontWeight::NORMAL
        },
        ..window.text_style()
    };
    f32::from(
        window
            .text_system()
            .shape_line(
                text.to_owned().into(),
                px(16.),
                &[style.to_run(text.len())],
                None,
            )
            .width,
    )
}

fn title_budget(available: f32, base: f32) -> f32 {
    (available - base - 8.).clamp(0., (available.max(0.) * 0.3).min(TITLE_LIMIT))
}

fn pill_button(
    id: u64,
    name: String,
    active: bool,
    interactive: bool,
    action: impl Fn(&mut Window, &mut App) + 'static,
) -> gpui::Stateful<Div> {
    button_with_hover(
        gpui::ElementId::Name(format!("workspace-{id}").into()),
        format!("ワークスペース {name}"),
        action,
        |style| style.bg(rgba(0xffffff0c)),
    )
    .w_full()
    .h_full()
    .min_h_0()
    .p_0()
    .rounded_full()
    .aria_selected(active)
    .when(!interactive, |button| {
        button.tab_stop(false).cursor_default()
    })
}

impl Render for WorkspaceStrip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reports = Rc::new(RefCell::new(Vec::new()));
        let focused_icon = Rc::new(Cell::new(None::<(f32, f32)>));
        let scroll = self.scroll.clone();
        let keyboard_scroll = scroll.clone();
        let mut strip = div()
            .id("workspace-strip")
            .relative()
            .w_full()
            .h(px(theme::BAR_HEIGHT))
            .min_w_0()
            .flex()
            .items_center()
            .overflow_x_scroll()
            .track_scroll(&scroll)
            .focusable()
            .tab_stop(true)
            .on_key_down(move |event, window, cx| {
                let offset = keyboard_scroll.offset().x;
                let max = keyboard_scroll.max_offset().x;
                let next = match event.keystroke.key.as_str() {
                    "left" => (offset + px(ICON_CELL)).min(px(0.)),
                    "right" => (offset - px(ICON_CELL)).max(-max),
                    "home" => px(0.),
                    "end" => -max,
                    _ => return,
                };
                keyboard_scroll.set_offset(point(next, px(0.)));
                cx.stop_propagation();
                window.refresh();
            });
        if self.model.pills.is_empty() && self.loading {
            strip = strip.child(muted("ワークスペースを取得中"));
        }
        for (index, pill) in self.model.pills.clone().into_iter().enumerate() {
            if self.reveal && pill.active {
                scroll.scroll_to_item(index);
            }
            let id = pill.presence.id;
            let name = single_line(workspace_name(&pill.workspace));
            let label = visual_label(&pill.workspace).map(|text| {
                let text = single_line(text);
                let width = text_width(window, &text, true).clamp(12., 80.);
                (text, width)
            });
            let label_width = label.as_ref().map(|(_, width)| *width);
            let count = pill
                .windows
                .iter()
                .filter(|icon| icon.presence.live)
                .count();
            let base = pill_base_width(label_width, count);
            let title_width = pill
                .titles
                .iter()
                .find(|title| title.presence.live)
                .map(|title| {
                    text_width(window, &title.text, false).min(title_budget(self.available, base))
                })
                .filter(|width| *width > 0.)
                .map(|width| width + 8.)
                .unwrap_or(0.);
            let target_width = if pill.presence.live {
                base + title_width
            } else {
                0.
            };
            let mut row = div()
                .h_full()
                .flex()
                .items_center()
                .flex_shrink_0()
                .px(px(PADDING))
                .when_some(label, |row, (text, width)| {
                    row.child(
                        div()
                            .w(px(width))
                            .flex_shrink_0()
                            .truncate()
                            .font_family(theme::DATA_FONT)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_size(px(16.))
                            .child(text),
                    )
                });
            let mut icons = div().flex().items_center().flex_shrink_0();
            for icon in &pill.windows {
                let desktop = self.desktop.clone();
                let class = icon.client.class.clone();
                let is_focused = self
                    .model
                    .focus
                    .as_ref()
                    .is_some_and(|(workspace, client)| {
                        workspace == workspace_address(&pill.workspace)
                            && client == &icon.client.address
                    })
                    && icon.presence.live;
                let measured = focused_icon.clone();
                let cell = div()
                    .relative()
                    .h(px(32.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center();
                icons = icons.child(icon.presence.animate(
                    cell,
                    Duration::from_millis(if icon.presence.live { 160 } else { 140 }),
                    reports.clone(),
                    move |cell, value| {
                        let size = ICON_SIZE * (0.94 + 0.06 * value);
                        cell.w(px(ICON_CELL * value))
                            .opacity(value)
                            .child(desktop.app_icon_for_class(&class, size))
                            .when(is_focused, |cell| {
                                cell.child(
                                    canvas(
                                        {
                                            let measured = measured.clone();
                                            move |bounds, _, _| {
                                                measured.set(Some((
                                                    f32::from(bounds.left()),
                                                    f32::from(bounds.right()),
                                                )))
                                            }
                                        },
                                        |_, _, _, _| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                            })
                    },
                ));
            }
            if !pill.windows.is_empty() {
                row = row.child(icons.ml(px(if label_width.is_some() { 8. } else { 0. })));
            }
            let mut titles = div()
                .relative()
                .h(px(32.))
                .flex_shrink_0()
                .overflow_hidden();
            for title in &pill.titles {
                titles = titles.child(
                    title.presence.animate(
                        div()
                            .absolute()
                            .left(px(8.))
                            .right_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_size(px(16.))
                                    .text_color(rgb(theme::ACCENT))
                                    .child(title.text.clone()),
                            ),
                        FADE,
                        reports.clone(),
                        |text, value| text.opacity(value),
                    ),
                );
            }
            row = row.child(
                titles.with_spring(
                    "title-width",
                    SpringAnimation::new(spring())
                        .to(px(title_width))
                        .with_epsilon(0.25),
                    |titles, width| titles.w(width.max(px(0.))),
                ),
            );
            let highlight = div()
                .absolute()
                .inset_0()
                .rounded_full()
                .bg(rgba(0xcba6f714))
                .with_spring(
                    "active-highlight",
                    SpringAnimation::new(spring()).to(pill.active).from(false),
                    |highlight, value| highlight.opacity(value.0.clamp(0., 1.)),
                );
            let view = cx.entity();
            let clickable =
                pill.presence.live && matches!(&pill.workspace, ActiveWorkspace::Normal(_));
            let button = pill_button(id, name, pill.active, clickable, move |_, cx| {
                let request = {
                    let view = view.read(cx);
                    view.model
                        .switch_target(id)
                        .map(|target| (view.desktop.clone(), target))
                };
                if let Some((desktop, target)) = request {
                    desktop.switch_workspace(&target, cx);
                }
            })
            .child(
                self.glass
                    .element(GlassState::default(), cx)
                    .relative()
                    .w_full()
                    .h_full()
                    .overflow_hidden()
                    .child(highlight)
                    .child(row),
            );
            let body = pill
                .presence
                .animate(button, FADE, reports.clone(), |button, value| {
                    button.opacity(value)
                });
            let height = div()
                .w_full()
                .flex()
                .items_center()
                .child(body)
                .with_spring(
                    "pill-height",
                    SpringAnimation::new(spring())
                        .to(px(if pill.active {
                            theme::BAR_HEIGHT
                        } else {
                            INACTIVE_HEIGHT
                        }))
                        .from(px(INACTIVE_HEIGHT)),
                    |height, value| height.h(value),
                );
            let width_reports = reports.clone();
            let target_slot_width = target_width + if pill.presence.live { PILL_GAP } else { 0. };
            strip = strip.child(
                div()
                    .id(gpui::ElementId::Name(format!("pill-slot-{id}").into()))
                    .h(px(theme::BAR_HEIGHT))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .child(height)
                    .with_spring(
                        gpui::ElementId::Name(format!("pill-width-{id}").into()),
                        SpringAnimation::new(spring())
                            .to(px(target_slot_width))
                            .from(px(0.))
                            .with_epsilon(0.25),
                        move |slot, width| {
                            width_reports.borrow_mut().push(Report::Width(
                                id,
                                f32::from(width),
                                target_slot_width,
                            ));
                            slot.w(width.max(px(0.)))
                                .pr(px(PILL_GAP.min(f32::from(width).max(0.))))
                        },
                    ),
            );
        }
        let indicator = div()
            .absolute()
            .top(px(31.))
            .w(px(12.))
            .h(px(2.))
            .rounded_full()
            .bg(rgb(theme::PRIMARY))
            .with_spring(
                "focus-position",
                SpringAnimation::new(SpringConfig::new(400., 40., 1.))
                    .to(px(self.indicator.unwrap_or(0.))),
                |indicator, x| indicator.left(x),
            );
        strip = strip.child(
            div().absolute().inset_0().child(indicator).with_spring(
                "focus-visibility",
                SpringAnimation::new(spring())
                    .to(self.model.focus.is_some() && self.indicator.is_some())
                    .from(false),
                |indicator, phase| indicator.opacity(phase.0.clamp(0., 1.)),
            ),
        );
        let view = cx.entity().downgrade();
        let state_revision = self.model.revision;
        // Commit measurements after prepaint, not while this view is laying out.
        strip.child(
            canvas(
                move |bounds, _, cx| {
                    let origin = f32::from(bounds.left());
                    let focused = focused_icon.get();
                    let measured = reports.replace(Vec::new());
                    let scroll = scroll.clone();
                    cx.defer(move |cx| {
                        let _ = view.update(cx, |view, cx| {
                            if view.model.revision != state_revision {
                                return;
                            }
                            for report in measured {
                                view.model.report(report);
                            }
                            let mut changed = view.model.finish();
                            let available = f32::from(scroll.bounds().size.width);
                            if (available - view.available).abs() > 0.5 {
                                view.available = available;
                                view.reveal = true;
                                changed = true;
                            }
                            let target =
                                focused.map(|(left, right)| (left + right) * 0.5 - origin - 6.);
                            if view.model.focus.is_some()
                                && let Some(target) = target
                                && view.indicator.is_none_or(|old| (old - target).abs() > 0.25)
                            {
                                view.indicator = Some(target);
                                changed = true;
                            }
                            if view.reveal {
                                if let Some((left, right)) = focused {
                                    let viewport = scroll.bounds();
                                    let offset = reveal_offset(
                                        f32::from(scroll.offset().x),
                                        left,
                                        right,
                                        f32::from(viewport.left()),
                                        f32::from(viewport.right()),
                                        f32::from(scroll.max_offset().x),
                                    );
                                    if (offset - f32::from(scroll.offset().x)).abs() > 0.25 {
                                        scroll.set_offset(point(px(offset), px(0.)));
                                        changed = true;
                                    }
                                }
                                view.reveal = !view
                                    .model
                                    .pills
                                    .iter()
                                    .all(|pill| (pill.width - pill.target_width).abs() <= 0.25);
                            }
                            if changed {
                                cx.notify();
                            }
                        });
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size(px(1.)),
        )
    }
}

fn reveal_offset(
    offset: f32,
    left: f32,
    right: f32,
    viewport_left: f32,
    viewport_right: f32,
    maximum: f32,
) -> f32 {
    let delta = if left < viewport_left {
        viewport_left - left
    } else if right > viewport_right {
        viewport_right - right
    } else {
        0.
    };
    (offset + delta).clamp(-maximum.max(0.), 0.)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bkss_integration::{ActiveWindow, SpecialWorkspace, Workspace};

    fn workspace(address: &str, id: i64) -> Workspace {
        Workspace {
            address: address.into(),
            id: Some(id),
            name: id.to_string(),
        }
    }

    fn client(address: &str, workspace: Option<&str>) -> HyprlandClient {
        HyprlandClient {
            address: address.into(),
            class: "same-app".into(),
            workspace_address: workspace.map(str::to_owned),
        }
    }

    fn state() -> HyprlandState {
        HyprlandState {
            workspaces: vec![
                workspace("one", 1),
                workspace("two", 2),
                workspace("empty", 3),
            ],
            special_workspaces: vec![SpecialWorkspace {
                address: "special".into(),
                name: "scratchpad".into(),
            }],
            clients: vec![
                client("b", Some("one")),
                client("a", Some("one")),
                client("c", Some("two")),
                client("special-window", Some("special")),
                client("unassigned", None),
            ],
            active_workspace: Some(ActiveWorkspace::Normal(workspace("one", 1))),
            active_window: Some(ActiveWindow {
                address: "a".into(),
                title: "First title".into(),
                class: "same-app".into(),
            }),
        }
    }

    fn finish_entries(model: &mut Model) {
        for pill in &mut model.pills {
            pill.presence.value = 1.;
            pill.width = 200.;
            for icon in &mut pill.windows {
                icon.presence.value = 1.;
            }
            for title in &mut pill.titles {
                title.presence.value = 1.;
            }
        }
    }

    fn normal_target(model: &Model, address: &str) -> Option<String> {
        model
            .pills
            .iter()
            .find(|pill| {
                matches!(&pill.workspace, ActiveWorkspace::Normal(_))
                    && workspace_address(&pill.workspace) == address
            })
            .and_then(|pill| model.switch_target(pill.presence.id))
    }

    #[test]
    fn normal_pills_keep_their_visibility_rules_with_only_the_active_special_added() {
        let mut state = state();
        let (pills, focus) = desired(&state);
        assert_eq!(
            pills
                .iter()
                .map(|pill| workspace_address(&pill.workspace))
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(
            pills[0]
                .windows
                .iter()
                .map(|icon| icon.address.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(pills[0].windows[0].class, pills[0].windows[1].class);
        assert_eq!(focus, Some(("one".into(), "a".into())));
        state.active_workspace = Some(ActiveWorkspace::Normal(workspace("missing-empty", 4)));
        let (pills, focus) = desired(&state);
        assert_eq!(
            pills
                .iter()
                .map(|pill| workspace_address(&pill.workspace))
                .collect::<Vec<_>>(),
            ["one", "two", "missing-empty"]
        );
        assert!(pills[2].active && pills[2].windows.is_empty());
        assert!(focus.is_none());
        state.active_workspace = Some(ActiveWorkspace::Special(
            state.special_workspaces[0].clone(),
        ));
        let (pills, focus) = desired(&state);
        assert_eq!(pills.len(), 3);
        assert!(
            pills
                .iter()
                .take(2)
                .all(|pill| !pill.active && pill.title.is_empty())
        );
        assert!(matches!(&pills[2].workspace, ActiveWorkspace::Special(_)));
        assert!(pills[2].active);
        assert!(focus.is_none());
    }

    #[test]
    fn special_visual_label_is_absent_without_reserving_normal_label_width_or_gap() {
        let normal = ActiveWorkspace::Normal(workspace("one", 1));
        let special = ActiveWorkspace::Special(SpecialWorkspace {
            address: "special".into(),
            name: "scratchpad".into(),
        });
        assert_eq!(visual_label(&normal), Some("1"));
        assert_eq!(visual_label(&special), None);
        assert_eq!(workspace_name(&special), "scratchpad");
        assert_eq!(
            pill_base_width(Some(16.), 2),
            PADDING * 2. + 16. + 8. + ICON_CELL * 2.
        );
        assert_eq!(pill_base_width(None, 2), PADDING * 2. + ICON_CELL * 2.);
        assert_eq!(pill_base_width(Some(16.), 0), PADDING * 2. + 16.);
        assert_eq!(pill_base_width(None, 0), PADDING * 2.);
        assert_eq!(
            pill_base_width(Some(16.), 2) - pill_base_width(None, 2),
            24.
        );
    }

    #[test]
    fn active_special_keeps_semantic_name_windows_title_and_focus_without_listing_other_specials() {
        let mut snapshot = state();
        snapshot.special_workspaces.push(SpecialWorkspace {
            address: "inactive-special".into(),
            name: "not-listed".into(),
        });
        snapshot.clients.extend([
            client("another-special-window", Some("special")),
            client("hidden-special-window", Some("inactive-special")),
        ]);
        snapshot.active_workspace = Some(ActiveWorkspace::Special(
            snapshot.special_workspaces[0].clone(),
        ));
        snapshot.active_window = Some(ActiveWindow {
            address: "special-window".into(),
            title: "Special title".into(),
            class: "same-app".into(),
        });
        let (pills, focus) = desired(&snapshot);
        assert_eq!(
            pills
                .iter()
                .map(|pill| workspace_address(&pill.workspace))
                .collect::<Vec<_>>(),
            ["one", "two", "special"]
        );
        let special = &pills[2];
        assert!(special.active);
        assert_eq!(workspace_name(&special.workspace), "scratchpad");
        assert_eq!(visual_label(&special.workspace), None);
        assert_eq!(
            special
                .windows
                .iter()
                .map(|icon| icon.address.as_str())
                .collect::<Vec<_>>(),
            ["another-special-window", "special-window"]
        );
        assert_eq!(special.title, "Special title");
        assert_eq!(focus, Some(("special".into(), "special-window".into())));
        assert_eq!(
            pills[0]
                .windows
                .iter()
                .map(|icon| icon.address.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(
            pills[1]
                .windows
                .iter()
                .map(|icon| icon.address.as_str())
                .collect::<Vec<_>>(),
            ["c"]
        );
        snapshot.active_window.as_mut().unwrap().address = "a".into();
        let (pills, focus) = desired(&snapshot);
        assert!(focus.is_none());
        assert!(pills.iter().all(|pill| pill.title.is_empty()));

        snapshot
            .clients
            .retain(|client| client.workspace_address.as_deref() != Some("special"));
        snapshot.special_workspaces.clear();
        let (pills, focus) = desired(&snapshot);
        assert!(pills[2].active && pills[2].windows.is_empty());
        assert_eq!(workspace_address(&pills[2].workspace), "special");
        assert!(focus.is_none());
    }

    #[test]
    fn special_transition_retains_and_revives_the_same_typed_pill_then_removes_it_after_exit() {
        let normal = state();
        let mut special = normal.clone();
        special.active_workspace = Some(ActiveWorkspace::Special(
            special.special_workspaces[0].clone(),
        ));
        special.active_window = Some(ActiveWindow {
            address: "special-window".into(),
            title: "Special title".into(),
            class: "same-app".into(),
        });
        let mut model = Model::default();
        model.reconcile(&normal);
        finish_entries(&mut model);
        let normal_id = model.pills[0].presence.id;
        model.reconcile(&special);
        assert_eq!(model.active.as_deref(), Some("special"));
        assert_eq!(
            model.focus,
            Some(("special".into(), "special-window".into()))
        );
        assert_eq!(model.pills[0].presence.id, normal_id);
        assert_eq!(normal_target(&model, "one").as_deref(), Some("1"));
        finish_entries(&mut model);
        let special_id = model.pills[2].presence.id;
        assert!(model.switch_target(special_id).is_none());
        model.reconcile(&normal);
        let leaving = model.pills[2].presence;
        let leaving_icon = model.pills[2].windows[0].presence;
        assert!(!leaving.live);
        assert!(model.switch_target(special_id).is_none());
        assert_eq!(model.active.as_deref(), Some("one"));
        model.report(Report::Presence(leaving.id, leaving.revision, 0.4));
        model.report(Report::Presence(
            leaving_icon.id,
            leaving_icon.revision,
            0.6,
        ));
        model.reconcile(&special);
        model.report(Report::Presence(leaving.id, leaving.revision, 0.));
        model.report(Report::Presence(leaving_icon.id, leaving_icon.revision, 0.));
        model.finish();
        assert_eq!(
            model
                .pills
                .iter()
                .filter(|pill| matches!(&pill.workspace, ActiveWorkspace::Special(_)))
                .count(),
            1
        );
        assert_eq!(model.pills[2].presence.id, special_id);
        assert_eq!(model.pills[2].presence.sample(0.), 0.4);
        assert_eq!(model.pills[2].windows[0].presence.sample(0.), 0.6);
        assert!(model.switch_target(special_id).is_none());
        model.reconcile(&normal);
        let leaving = model.pills[2].presence;
        model.report(Report::Presence(leaving.id, leaving.revision, 0.));
        model.report(Report::Width(leaving.id, 10., 0.));
        model.finish();
        assert_eq!(model.pills.len(), 3);
        model.report(Report::Width(leaving.id, 0., 0.));
        model.finish();
        assert_eq!(model.pills.len(), 2);
        assert!(model.switch_target(special_id).is_none());
        assert_eq!(normal_target(&model, "one").as_deref(), Some("1"));
    }

    #[test]
    fn typed_special_identity_cannot_dispatch_a_normal_workspace_selector() {
        let mut snapshot = state();
        snapshot.workspaces.push(Workspace {
            address: "named".into(),
            id: None,
            name: "scratchpad".into(),
        });
        snapshot.clients.push(client("named-window", Some("named")));
        snapshot.active_workspace = Some(ActiveWorkspace::Special(
            snapshot.special_workspaces[0].clone(),
        ));
        let mut model = Model::default();
        model.reconcile(&snapshot);
        assert_eq!(normal_target(&model, "one").as_deref(), Some("1"));
        assert_eq!(
            normal_target(&model, "named").as_deref(),
            Some("name:scratchpad")
        );
        let special = model
            .pills
            .iter()
            .find(|pill| matches!(&pill.workspace, ActiveWorkspace::Special(_)))
            .unwrap();
        assert!(model.switch_target(special.presence.id).is_none());
        assert!(!same_workspace(
            &ActiveWorkspace::Normal(workspace("special", 99)),
            &special.workspace,
        ));
    }

    #[test]
    fn mirrored_global_focus_requires_an_exact_client_membership_join() {
        let mut snapshot = state();
        let mut first = Model::default();
        let mut second = Model::default();
        first.reconcile(&snapshot);
        second.reconcile(&snapshot);
        assert_eq!(first.active, second.active);
        assert_eq!(first.focus, second.focus);
        for address in ["c", "unknown", "special-window"] {
            snapshot.active_window.as_mut().unwrap().address = address.into();
            let (pills, focus) = desired(&snapshot);
            assert!(focus.is_none());
            assert!(pills.iter().all(|pill| pill.title.is_empty()));
        }
        snapshot.active_window = None;
        assert!(desired(&snapshot).1.is_none());
    }

    #[test]
    fn rapid_focus_changes_keep_window_order_and_retarget_title_layers_without_duplicates() {
        let mut snapshot = state();
        let mut model = Model::default();
        model.reconcile(&snapshot);
        finish_entries(&mut model);
        let pill_id = model.pills[0].presence.id;
        let icon_ids: Vec<_> = model.pills[0]
            .windows
            .iter()
            .map(|icon| icon.presence.id)
            .collect();
        snapshot.clients.reverse();
        snapshot.active_window.as_mut().unwrap().address = "b".into();
        snapshot.active_window.as_mut().unwrap().title = "Second title".into();
        model.reconcile(&snapshot);
        assert_eq!(model.pills[0].presence.id, pill_id);
        assert_eq!(
            model.pills[0]
                .windows
                .iter()
                .map(|icon| icon.presence.id)
                .collect::<Vec<_>>(),
            icon_ids
        );
        assert_eq!(
            model.pills[0]
                .windows
                .iter()
                .map(|icon| icon.client.address.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(model.pills[0].titles.len(), 2);
        let old = model.pills[0].titles[0].presence;
        model.report(Report::Presence(old.id, old.revision, 0.4));
        let incoming = model.pills[0].titles[1].presence;
        model.report(Report::Presence(incoming.id, incoming.revision, 0.6));
        snapshot.active_window.as_mut().unwrap().title = "First title".into();
        model.reconcile(&snapshot);
        let revived = model.pills[0].titles[0].presence;
        assert!(revived.live);
        assert_eq!(revived.sample(0.), 0.4);
        model.report(Report::Presence(old.id, old.revision, 0.));
        model.finish();
        assert_eq!(model.pills[0].titles.len(), 2);
        assert_eq!(
            model.pills[0]
                .titles
                .iter()
                .filter(|title| title.presence.live)
                .count(),
            1
        );
        assert_eq!(model.focus, Some(("one".into(), "b".into())));
    }

    #[test]
    fn moved_windows_and_departing_pills_remain_until_native_completion_and_can_revive() {
        let mut snapshot = state();
        let mut model = Model::default();
        model.reconcile(&snapshot);
        finish_entries(&mut model);
        snapshot
            .clients
            .iter_mut()
            .find(|icon| icon.address == "a")
            .unwrap()
            .workspace_address = Some("two".into());
        model.reconcile(&snapshot);
        let old_icon = model.pills[0]
            .windows
            .iter()
            .find(|icon| icon.client.address == "a")
            .unwrap()
            .presence;
        assert!(!old_icon.live);
        assert_eq!(model.pills[0].windows.len(), 2);
        assert!(
            model.pills[1]
                .windows
                .iter()
                .any(|icon| icon.client.address == "a" && icon.presence.live)
        );
        assert!(model.focus.is_none());
        snapshot
            .clients
            .iter_mut()
            .find(|icon| icon.address == "a")
            .unwrap()
            .workspace_address = Some("one".into());
        model.reconcile(&snapshot);
        model.report(Report::Presence(old_icon.id, old_icon.revision, 0.));
        model.finish();
        assert_eq!(
            model.pills[0]
                .windows
                .iter()
                .filter(|icon| icon.client.address == "a")
                .count(),
            1
        );
        assert!(
            model.pills[0]
                .windows
                .iter()
                .find(|icon| icon.client.address == "a")
                .unwrap()
                .presence
                .live
        );

        snapshot
            .clients
            .retain(|icon| icon.workspace_address.as_deref() != Some("two"));
        model.reconcile(&snapshot);
        let departing = model.pills[1].presence;
        assert!(!departing.live);
        assert!(normal_target(&model, "two").is_none());
        model.report(Report::Presence(departing.id, departing.revision, 0.));
        assert!(!model.finish());
        assert_eq!(model.pills.len(), 2);
        model.report(Report::Width(departing.id, 0., 0.));
        assert!(model.finish());
        assert_eq!(model.pills.len(), 1);
        assert_eq!(normal_target(&model, "one").as_deref(), Some("1"));
        assert_eq!(model.active.as_deref(), Some("one"));
    }

    #[test]
    fn workspace_revival_rejects_old_completion_and_never_marks_clicks_active_optimistically() {
        let mut snapshot = state();
        let mut model = Model::default();
        model.reconcile(&snapshot);
        finish_entries(&mut model);
        let before = model.active.clone();
        assert_eq!(normal_target(&model, "two").as_deref(), Some("2"));
        assert_eq!(model.active, before);
        let clients = snapshot.clients.clone();
        snapshot
            .clients
            .retain(|icon| icon.workspace_address.as_deref() != Some("two"));
        model.reconcile(&snapshot);
        let old = model.pills[1].presence;
        model.report(Report::Presence(old.id, old.revision, 0.5));
        snapshot.clients = clients;
        model.reconcile(&snapshot);
        model.report(Report::Presence(old.id, old.revision, 0.));
        model.report(Report::Width(old.id, 0., 0.));
        model.finish();
        assert_eq!(
            model
                .pills
                .iter()
                .filter(|pill| workspace_address(&pill.workspace) == "two")
                .count(),
            1
        );
        assert_eq!(model.pills[1].presence.sample(0.), 0.5);
        assert_eq!(normal_target(&model, "two").as_deref(), Some("2"));
    }

    #[test]
    fn overflow_keeps_every_window_and_limits_only_the_title() {
        let mut snapshot = state();
        snapshot.clients = (0..128)
            .map(|index| client(&format!("window-{index:03}"), Some("one")))
            .collect();
        let (pills, _) = desired(&snapshot);
        assert_eq!(pills.len(), 1);
        assert_eq!(pills[0].windows.len(), 128);
        assert_eq!(pills[0].windows.last().unwrap().address, "window-127");
        assert_eq!(title_budget(400., 500.), 0.);
        assert_eq!(title_budget(1000., 80.), TITLE_LIMIT);
        assert!((title_budget(200., 100.) - 60.).abs() < 0.001);
        assert_eq!(reveal_offset(-100., 250., 274., 0., 240., 500.), -134.);
        assert_eq!(reveal_offset(-100., -20., 4., 0., 240., 500.), -80.);
        assert_eq!(reveal_offset(-100., 20., 44., 0., 240., 500.), -100.);
        assert_eq!(reveal_offset(0., 250., 274., 0., 240., 0.), 0.);
    }

    #[test]
    fn pill_construction_composes_hover_once_and_uses_native_animation_builders() {
        assert_eq!(theme::BAR_HEIGHT, 40.);
        assert_eq!(INACTIVE_HEIGHT, 36.);
        assert_eq!(ICON_SIZE, 18.);
        for active in [false, true] {
            for live in [false, true] {
                let mut pill = pill_button(1, "1".into(), active, live, |_, _| {});
                assert_eq!(pill.style().padding.top, Some(px(0.).into()));
                let reports = Rc::new(RefCell::new(Vec::new()));
                drop(
                    Presence::new(1)
                        .animate(pill, FADE, reports, |pill, value| pill.opacity(value)),
                );
                drop(div().with_spring(
                    "pill-width-1",
                    SpringAnimation::new(spring()).to(px(200.)),
                    |pill, width| pill.w(width),
                ));
                drop(div().with_spring(
                    "focus-position",
                    SpringAnimation::new(spring()).to(px(80.)),
                    |line, x| line.left(x),
                ));
            }
        }
        assert_eq!(
            single_line("Title\nwith\rline\tbreaks"),
            "Title with line breaks"
        );
    }
}
