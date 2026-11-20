use gpui::{Div, ParentElement, Styled, div, px, rgb};

use super::super::theme::MUTED;

pub fn label(text: impl Into<gpui::SharedString>) -> Div {
    div().text_size(px(13.)).child(text.into())
}

pub fn muted(text: impl Into<gpui::SharedString>) -> Div {
    label(text).text_color(rgb(MUTED))
}
