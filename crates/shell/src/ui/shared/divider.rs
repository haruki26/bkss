use gpui::{Div, Styled, div, px, rgba};

pub fn divider() -> Div {
    div()
        .w(px(1.))
        .h(px(18.))
        .mx_1()
        .flex_shrink_0()
        .bg(rgba(0x585b70e6))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::BAR_HEIGHT;
    use gpui::px;

    #[test]
    fn bar_divider_has_a_single_height_that_fits_the_bar_content() {
        let mut divider = divider();
        assert_eq!(divider.style().size.height, Some(px(18.).into()));
        assert_eq!(divider.style().size.width, Some(px(1.).into()));
        assert_eq!(BAR_HEIGHT, 40.);
    }
}
