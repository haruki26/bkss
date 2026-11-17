/// A fresh Glass element with the default static decoration and ordinary GPUI
/// container styling and child composition. This route creates no subscriptions.
///
/// ```
/// use bkss_ui::Glass;
/// use gpui::prelude::*;
///
/// let card = Glass::new().rounded_xl().p_4().child("Content");
/// ```
///
/// Decoration settings, including its radius, belong to `GlassStyle`; GPUI
/// container styles do not replace them. For custom or reactive decoration,
/// retain [`super::MountedGlass`] in the owning view and call its `element`
/// method on every render:
///
/// ```
/// use bkss_ui::effects::{GlassState, GlassStyle, MountedGlass};
/// use gpui::{Context, prelude::*};
///
/// struct Content { glass: MountedGlass }
/// impl Content {
///     fn new(cx: &mut Context<Self>) -> Self {
///         Self { glass: MountedGlass::new(GlassStyle::default(), cx) }
///     }
///
///     fn content(&self, cx: &Context<Self>) -> impl gpui::IntoElement {
///         self.glass.element(GlassState::default(), cx).p_4().child("Content")
///     }
/// }
/// ```
///
/// Both routes use core's renderer. GPUI draws decoration and content;
/// background blur/refraction requires externally configured Hyprglass.
/// Independent per-card background sampling or intensity is not promised.
pub use bkss_core::GlassElement as Glass;
