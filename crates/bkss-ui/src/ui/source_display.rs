use bkss_core::{MountedBinding, ReactiveSubscription, SourceState};
use bkss_integration::IntegrationView;
use gpui::{AnyElement, Context, IntoElement, Render, Window};

type ReadyRenderer<T> =
    Box<dyn FnMut(&T, &mut Window, &mut Context<SourceDisplay<T>>) -> AnyElement>;
type LoadingRenderer<T> = Box<dyn FnMut(&mut Window, &mut Context<SourceDisplay<T>>) -> AnyElement>;

/// A source's default integration view, or caller-provided GPUI content.
///
/// Create this view in a mounted content factory after calling
/// [`bkss_core::RuntimeHandle::mount`]. It owns the binding and redraw
/// subscription until the entity is released. It does not retain the runtime.
/// Layout, typography, colors, and interaction remain ordinary GPUI composition.
pub struct SourceDisplay<T: Clone + 'static> {
    binding: MountedBinding<T>,
    _subscription: ReactiveSubscription,
    ready: ReadyRenderer<T>,
    loading: LoadingRenderer<T>,
}

impl<T: Clone + 'static> SourceDisplay<T> {
    /// Uses [`IntegrationView::render`] for acquired values and “Loading…” before acquisition.
    /// Ready domain values such as empty lists are never treated as loading.
    pub fn new(binding: MountedBinding<T>, cx: &mut Context<Self>) -> Self
    where
        T: IntegrationView,
    {
        Self::new_with_render(binding, cx, |value, window, cx| value.render(window, cx))
    }

    /// Creates a view for any source value, without requiring [`IntegrationView`].
    ///
    /// ```
    /// use bkss_core::MountedBinding;
    /// use bkss_ui::SourceDisplay;
    /// use gpui::{Context, div, prelude::*};
    ///
    /// fn count_view(
    ///     binding: MountedBinding<u32>,
    ///     cx: &mut Context<SourceDisplay<u32>>,
    /// ) -> SourceDisplay<u32> {
    ///     SourceDisplay::new_with_render(binding, cx, |value, _, _| {
    ///         div().child(format!("Count: {value}"))
    ///     })
    /// }
    /// ```
    pub fn new_with_render<E: IntoElement>(
        binding: MountedBinding<T>,
        cx: &mut Context<Self>,
        mut render: impl FnMut(&T, &mut Window, &mut Context<Self>) -> E + 'static,
    ) -> Self {
        let subscription = binding.reactive().observe(cx, |_, _, cx| cx.notify());
        Self {
            binding,
            _subscription: subscription,
            ready: Box::new(move |value, window, cx| render(value, window, cx).into_any_element()),
            loading: Box::new(|_, _| "Loading…".into_any_element()),
        }
    }

    /// Replaces the default ready content with formatted text or any styled GPUI element.
    /// The callback runs on render and can use `cx.listener` for interaction.
    pub fn with_render<E: IntoElement>(
        mut self,
        mut render: impl FnMut(&T, &mut Window, &mut Context<Self>) -> E + 'static,
    ) -> Self {
        self.ready =
            Box::new(move |value, window, cx| render(value, window, cx).into_any_element());
        self
    }

    /// Replaces only the initial loading content; no previous activation is shown.
    pub fn with_loading<E: IntoElement>(
        mut self,
        mut render: impl FnMut(&mut Window, &mut Context<Self>) -> E + 'static,
    ) -> Self {
        self.loading = Box::new(move |window, cx| render(window, cx).into_any_element());
        self
    }
}

impl<T: Clone + 'static> Render for SourceDisplay<T> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.binding.reactive().get(cx) {
            SourceState::Loading => (self.loading)(window, cx),
            SourceState::Ready(value) => (self.ready)(&value, window, cx),
        }
    }
}
