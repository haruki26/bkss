use super::controller::{Controller, Entry};
use crate::shell::{ShellError, runtime as shell_runtime};
use crate::{AnimationPlan, AnimationSample, Phase};
use gpui::{App, AppContext, DisplayId, Window};
use std::rc::Rc;

use super::super::animation::SurfaceAnimation;

impl Controller {
    pub(super) fn initialize_animation(
        self: &Rc<Self>,
        display: DisplayId,
        generation: u64,
        plan: AnimationPlan,
        cx: &mut App,
    ) -> Result<AnimationSample, ShellError> {
        let (is_current, existing) =
            self.entries
                .borrow()
                .get(&display)
                .map_or((false, None), |entry| match entry {
                    Entry::Pending {
                        generation: current,
                        animation,
                        ..
                    }
                    | Entry::Mounted {
                        generation: current,
                        animation,
                        ..
                    } if *current == generation => {
                        (true, animation.as_ref().map(SurfaceAnimation::value))
                    }
                    _ => (false, None),
                });
        if !is_current {
            return Err(ShellError::setup(
                super::controller::SurfaceRequestError::Unavailable,
            ));
        }
        if let Some(sample) = existing {
            return Ok(sample);
        }
        if !self.deferred_animation {
            return Err(ShellError::setup(
                super::controller::SurfaceRequestError::Unavailable,
            ));
        }

        let now = cx.background_executor().now();
        let animation = match SurfaceAnimation::new(plan, now) {
            Ok(animation) => animation,
            Err(error) => {
                self.fail_animation(error, cx);
                return Err(ShellError::setup(error));
            }
        };
        let sample = animation.value();
        let mounted = {
            let mut entries = self.entries.borrow_mut();
            match entries.get_mut(&display) {
                Some(Entry::Pending {
                    generation: current,
                    animation: slot,
                    ..
                }) if *current == generation && slot.is_none() => {
                    *slot = Some(animation);
                    false
                }
                Some(Entry::Mounted {
                    generation: current,
                    animation: slot,
                    ..
                }) if *current == generation && slot.is_none() => {
                    *slot = Some(animation);
                    true
                }
                Some(Entry::Pending {
                    generation: current,
                    animation: Some(animation),
                    ..
                })
                | Some(Entry::Mounted {
                    generation: current,
                    animation: Some(animation),
                    ..
                }) if *current == generation => return Ok(animation.value()),
                _ => {
                    return Err(ShellError::setup(
                        super::controller::SurfaceRequestError::Unavailable,
                    ));
                }
            }
        };
        if mounted {
            self.queue_frame(display, generation, cx);
        }
        Ok(sample)
    }

    pub(super) fn animation_sample(
        &self,
        display: DisplayId,
        generation: u64,
    ) -> Option<AnimationSample> {
        self.entries
            .borrow()
            .get(&display)
            .and_then(|entry| match entry {
                Entry::Pending {
                    generation: current,
                    animation,
                    ..
                }
                | Entry::Mounted {
                    generation: current,
                    animation,
                    ..
                } if *current == generation => animation.as_ref().map(SurfaceAnimation::value),
                _ => None,
            })
    }

    pub(super) fn reopen(self: &Rc<Self>, display: DisplayId, cx: &mut App) {
        let now = cx.background_executor().now();
        let generation = {
            let mut entries = self.entries.borrow_mut();
            let Some(Entry::Mounted {
                generation,
                animation: Some(animation),
                ..
            }) = entries.get_mut(&display)
            else {
                return;
            };
            if animation.phase() != Phase::Exit {
                return;
            }
            if let Err(error) = animation.request(Phase::Enter, now) {
                drop(entries);
                self.fail_animation(error, cx);
                return;
            }
            *generation
        };
        self.defer_refresh(display, generation, cx);
        self.queue_frame(display, generation, cx);
    }

    pub(super) fn defer_refresh(
        self: &Rc<Self>,
        display: DisplayId,
        generation: u64,
        cx: &mut App,
    ) {
        let window = match self.entries.borrow().get(&display) {
            Some(Entry::Mounted {
                generation: current,
                window,
                ..
            }) if *current == generation => *window,
            _ => return,
        };
        let weak = Rc::downgrade(self);
        cx.defer(move |cx| {
            let Some(controller) = weak.upgrade() else {
                return;
            };
            let is_current = matches!(
                controller.entries.borrow().get(&display),
                Some(Entry::Mounted { generation: current, window: current_window, .. })
                    if *current == generation && current_window.window_id() == window.window_id()
            );
            if is_current {
                let _ = cx.update_window(window, |_, window, _| window.refresh());
            }
        });
    }

    pub(super) fn fail_animation(&self, error: crate::RunnerError, cx: &mut App) {
        if let Some(runtime) = self.runtime.upgrade() {
            shell_runtime::record_setup_failure(&runtime, ShellError::setup(error));
            shell_runtime::stop_runtime(&runtime, cx);
            shell_runtime::request_host_quit(&runtime, cx);
        }
    }

    pub(super) fn queue_frame(self: &Rc<Self>, display: DisplayId, generation: u64, cx: &mut App) {
        let should_queue = {
            let mut entries = self.entries.borrow_mut();
            let Some(Entry::Mounted {
                generation: current,
                animation: Some(animation),
                ..
            }) = entries.get_mut(&display)
            else {
                return;
            };
            if *current != generation
                || !animation.value().running
                || animation.is_frame_pending()
                || self.stopped.get()
            {
                false
            } else {
                animation.set_frame_pending(true);
                true
            }
        };
        if should_queue {
            let weak = Rc::downgrade(self);
            cx.defer(move |cx| {
                if let Some(controller) = weak.upgrade() {
                    controller.schedule_frame(display, generation, cx);
                }
            });
        }
    }

    fn schedule_frame(self: &Rc<Self>, display: DisplayId, generation: u64, cx: &mut App) {
        let window = match self.entries.borrow().get(&display) {
            Some(Entry::Mounted {
                generation: current,
                window,
                animation: Some(animation),
            }) if *current == generation && animation.is_frame_pending() => *window,
            _ => return,
        };
        let weak = Rc::downgrade(self);
        if cx
            .update_window(window, move |_, window, _| {
                window.on_next_frame(move |window, app| {
                    if let Some(controller) = weak.upgrade() {
                        controller.advance_frame(display, generation, window, app);
                    }
                });
            })
            .is_err()
            && let Some(Entry::Mounted {
                generation: current,
                animation: Some(animation),
                ..
            }) = self.entries.borrow_mut().get_mut(&display)
            && *current == generation
        {
            animation.set_frame_pending(false);
        }
    }

    fn advance_frame(
        self: &Rc<Self>,
        display: DisplayId,
        generation: u64,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.stopped.get()
            || self
                .runtime
                .upgrade()
                .is_none_or(|runtime| runtime.stopped.get())
        {
            return;
        }
        let now = cx.background_executor().now();
        let (running, finish_exit, failure) = {
            let mut entries = self.entries.borrow_mut();
            let Some(Entry::Mounted {
                generation: current,
                animation: Some(animation),
                ..
            }) = entries.get_mut(&display)
            else {
                return;
            };
            if *current != generation || !animation.is_frame_pending() {
                return;
            }
            animation.set_frame_pending(false);
            match animation.sample(now) {
                Ok(sample) => (
                    sample.running,
                    sample
                        .completion
                        .is_some_and(|token| animation.is_exit_completion(token)),
                    None,
                ),
                Err(error) => (false, false, Some(error)),
            }
        };
        if let Some(error) = failure {
            self.fail_animation(error, cx);
            return;
        }
        window.refresh();
        if finish_exit {
            let removed = {
                let mut entries = self.entries.borrow_mut();
                matches!(entries.get(&display), Some(Entry::Mounted { generation: current, animation: Some(animation), .. }) if *current == generation && animation.phase() == Phase::Exit)
                    .then(|| entries.remove(&display))
                    .flatten()
            };
            if removed.is_some() {
                window.remove_window();
            }
        } else if running {
            let weak = Rc::downgrade(self);
            if let Some(Entry::Mounted {
                generation: current,
                animation: Some(animation),
                ..
            }) = self.entries.borrow_mut().get_mut(&display)
                && *current == generation
            {
                animation.set_frame_pending(true);
                window.on_next_frame(move |window, app| {
                    if let Some(controller) = weak.upgrade() {
                        controller.advance_frame(display, generation, window, app);
                    }
                });
            }
        }
    }
}
