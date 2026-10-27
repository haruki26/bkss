use super::Reactive;
use gpui::AppContext;

/// A source value distinguishes not-yet-ready from a ready domain value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceState<T> {
    Loading,
    Ready(T),
}

impl<T: Clone + 'static> Reactive<SourceState<T>> {
    /// Maps ready payloads while preserving `Loading`.
    pub fn map_ready<U, C, F>(&self, cx: &mut C, map: F) -> Reactive<SourceState<U>>
    where
        U: 'static,
        C: AppContext,
        F: Fn(T) -> U + 'static,
    {
        self.map(cx, move |state| match state {
            SourceState::Loading => SourceState::Loading,
            SourceState::Ready(value) => SourceState::Ready(map(value)),
        })
    }

    /// Combines ready payloads; the result remains loading until both inputs are ready.
    pub fn combine_ready<B, U, C, F>(
        &self,
        other: &Reactive<SourceState<B>>,
        cx: &mut C,
        combine: F,
    ) -> Reactive<SourceState<U>>
    where
        B: Clone + 'static,
        U: 'static,
        C: AppContext,
        F: Fn(T, B) -> U + 'static,
    {
        self.combine(other, cx, move |left, right| match (left, right) {
            (SourceState::Ready(left), SourceState::Ready(right)) => {
                SourceState::Ready(combine(left, right))
            }
            _ => SourceState::Loading,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::State;
    use super::SourceState;
    use gpui::TestAppContext;

    #[test]
    fn source_map_and_combine_preserve_loading_and_ready_none() {
        let cx = TestAppContext::single();
        let (left, right, mapped, combined) = cx.update(|cx| {
            let left = State::new(SourceState::<Option<u8>>::Loading, cx);
            let right = State::new(SourceState::Ready(4_u8), cx);
            let mapped = left.reactive().map_ready(cx, |value| value.map(|n| n + 1));
            let combined = left
                .reactive()
                .combine_ready(&right.reactive(), cx, |left, right| (left, right));
            (left, right, mapped, combined)
        });

        assert_eq!(mapped.get(&cx), SourceState::Loading);
        assert_eq!(combined.get(&cx), SourceState::Loading);

        cx.update(|cx| left.set(SourceState::Ready(None), cx));
        assert_eq!(mapped.get(&cx), SourceState::Ready(None));
        assert_eq!(combined.get(&cx), SourceState::Ready((None, 4)));

        cx.update(|cx| right.set(SourceState::Loading, cx));
        assert_eq!(combined.get(&cx), SourceState::Loading);
        cx.update(|cx| right.set(SourceState::Ready(6), cx));
        assert_eq!(combined.get(&cx), SourceState::Ready((None, 6)));
    }
}
