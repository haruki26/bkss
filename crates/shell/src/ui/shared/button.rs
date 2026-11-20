use gpui::{
    App, Div, InteractiveElement, Stateful, StatefulInteractiveElement, Styled, Window, div, px,
    rgb, rgba,
};

use crate::ui::theme::PRIMARY;

pub fn button(
    id: impl Into<gpui::ElementId>,
    accessible_label: impl Into<gpui::SharedString>,
    action: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    button_with_hover(id, accessible_label, action, |style| {
        style.bg(rgba(0xffffff16))
    })
}

pub fn button_with_hover(
    id: impl Into<gpui::ElementId>,
    accessible_label: impl Into<gpui::SharedString>,
    action: impl Fn(&mut Window, &mut App) + 'static,
    hover: impl FnOnce(gpui::StyleRefinement) -> gpui::StyleRefinement,
) -> Stateful<Div> {
    let action = std::rc::Rc::new(action);
    let keyboard_action = action.clone();
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(accessible_label)
        .focusable()
        .tab_stop(true)
        .cursor_pointer()
        .min_h(px(28.))
        .px_2()
        .py_1()
        .rounded_lg()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .text_size(px(12.))
        .hover(hover)
        .focus_visible(|s| s.bg(rgba(0xcba6f740)).text_color(rgb(PRIMARY)))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            action(window, cx);
        })
        .on_key_down(move |event, window, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
                keyboard_action(window, cx);
            }
        })
}
