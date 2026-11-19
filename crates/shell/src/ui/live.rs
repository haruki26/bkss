use bkss_core::{MountedBinding, ReactiveSubscription, SourceState};
use gpui::{App, Context};

pub struct Live<T: Clone + 'static> {
    binding: MountedBinding<T>,
    _subscription: ReactiveSubscription,
}

impl<T: Clone + 'static> Live<T> {
    pub fn new<Owner: 'static>(binding: MountedBinding<T>, cx: &mut Context<Owner>) -> Self {
        let subscription = binding.reactive().observe(cx, |_, _, cx| cx.notify());
        Self {
            binding,
            _subscription: subscription,
        }
    }

    pub fn get(&self, cx: &App) -> Option<T> {
        match self.binding.reactive().get(cx) {
            SourceState::Loading => None,
            SourceState::Ready(value) => Some(value),
        }
    }
}
