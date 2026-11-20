use super::{GlassElement, GlassState, GlassStyle, render};
use crate::ReactiveSubscription;
use gpui::{App, AppContext, Context, Entity, Subscription};

/// Mounted Glass decoration. Keep this in a GPUI view, not in its render method.
///
/// Construction subscribes the owning view to all reactive style inputs. The style
/// retains derived reactive handles; dropping or rebinding Glass releases them and
/// their subscriptions. Glass does not install pointer or focus handlers.
pub struct Glass {
    style: GlassStyle,
    subscriptions: Vec<ReactiveSubscription>,
    redraw: Entity<()>,
    _owner_subscription: Subscription,
}

impl Glass {
    /// Mounts the decoration in the view that will render it.
    pub fn new<Owner: 'static>(style: GlassStyle, cx: &mut Context<Owner>) -> Self {
        // Reactive callbacks are synchronous. A separate entity allows a view to
        // publish its own style inputs while that view is already being updated.
        let redraw = cx.new(|_| ());
        let subscriptions = redraw.update(cx, |_, cx| style.observe(cx, |_, cx| cx.notify()));
        let owner_subscription = cx.observe(&redraw, |_, _, cx| cx.notify());
        Self {
            style,
            subscriptions,
            redraw,
            _owner_subscription: owner_subscription,
        }
    }

    /// Replaces the style and releases the previous style's subscriptions.
    pub fn rebind(&mut self, style: GlassStyle, cx: &mut App) {
        self.subscriptions.clear();
        self.subscriptions = self.redraw.update(cx, |_, cx| {
            let subscriptions = style.observe(cx, |_, cx| cx.notify());
            cx.notify();
            subscriptions
        });
        self.style = style;
    }

    /// Creates fresh GPUI layout and content for this render.
    ///
    /// Use GPUI's `Styled` and `ParentElement` traits, for example
    /// `glass.element(state, cx).p_4().child("Content")`. Children are not stored or
    /// consumed by the mounted owner. Padding is caller-owned; decoration neither
    /// reserves border space nor clips children. Opacity affects decoration only.
    /// GPUI types and composition are an intentional part of this API.
    pub fn element(&self, state: GlassState, cx: &impl AppContext) -> GlassElement {
        render::element(self.style.resolve(state, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GlassOpacity, State};
    use gpui::{AppContext, TestAppContext};
    use std::{cell::Cell, rc::Rc};

    struct Owner {
        glass: Glass,
    }

    #[test]
    fn mounted_inputs_notify_without_resubscribing_and_rebind_releases_old_inputs() {
        let mut cx = TestAppContext::single();
        let (state, owner, notices, _observer) = cx.update(|cx| {
            let state = State::new(GlassOpacity::new(1.).unwrap(), cx);
            let style = GlassStyle::default().with_opacity(state.reactive());
            let owner = cx.new(|cx| Owner {
                glass: Glass::new(style, cx),
            });
            let notices = Rc::new(Cell::new(0));
            let count = notices.clone();
            let observer = cx.observe(&owner, move |_, _| count.set(count.get() + 1));
            (state, owner, notices, observer)
        });
        for _ in 0..3 {
            owner.update(&mut cx, |owner, cx| {
                drop(owner.glass.element(GlassState::default(), cx));
                assert_eq!(owner.glass.subscriptions.len(), 1);
            });
        }
        cx.update(|cx| state.set(GlassOpacity::new(0.4).unwrap(), cx));
        assert_eq!(notices.get(), 1);
        owner.update(&mut cx, |owner, cx| {
            assert_eq!(
                owner
                    .glass
                    .style
                    .resolve(GlassState::default(), cx)
                    .opacity
                    .get(),
                0.4
            );
            owner.glass.rebind(GlassStyle::default(), cx);
            assert!(owner.glass.subscriptions.is_empty());
        });
        let before = notices.get();
        cx.update(|cx| state.set(GlassOpacity::new(0.2).unwrap(), cx));
        assert_eq!(notices.get(), before);
    }

    #[test]
    fn mounted_style_retains_derived_handles_until_rebind_or_drop() {
        for rebind in [false, true] {
            let mut cx = TestAppContext::single();
            let calls = Rc::new(Cell::new(0));
            let (state, owner) = cx.update(|cx| {
                let state = State::new(GlassOpacity::new(1.).unwrap(), cx);
                let count = calls.clone();
                let derived = state.reactive().map(cx, move |value| {
                    count.set(count.get() + 1);
                    value
                });
                let owner = cx.new(|cx| Owner {
                    glass: Glass::new(GlassStyle::default().with_opacity(derived), cx),
                });
                (state, owner)
            });
            cx.update(|cx| state.set(GlassOpacity::new(0.5).unwrap(), cx));
            assert_eq!(calls.get(), 2);
            if rebind {
                owner.update(&mut cx, |owner, cx| {
                    owner.glass.rebind(GlassStyle::default(), cx)
                });
            } else {
                drop(owner);
            }
            // GPUI releases dropped entities at the end of an app update.
            cx.update(|_| ());
            cx.update(|cx| state.set(GlassOpacity::new(0.).unwrap(), cx));
            assert_eq!(calls.get(), 2);
        }
    }
}
