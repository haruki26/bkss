/// Default GPUI presentation for an acquired integration state.
///
/// Return fresh elements on each render; compose text, icons, or other GPUI
/// content here. Rendering does not acquire resources or own subscriptions.
/// Keep it synchronous and nonblocking. Source descriptors remain typed data
/// acquisition declarations, separate from this state-level presentation.
///
/// GPUI's `svg()` can use assets supplied by the application. This crate does
/// not register an asset source or bundle icons.
pub trait IntegrationView {
    fn render(&self, window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::AnyElement;
}
