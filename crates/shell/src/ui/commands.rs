use std::rc::{Rc, Weak};

use bkss_core::{MountedBinding, ReactiveSubscription, SourceState};
use bkss_integration::CommandIpcState;
use gpui::{App, AppContext, Context, Entity, Task};

use super::{desktop::Desktop, feedback};

pub struct CommandListener {
    binding: MountedBinding<CommandIpcState>,
    desktop: Weak<Desktop>,
    task: Option<Task<()>>,
    _subscription: ReactiveSubscription,
}

impl CommandListener {
    pub fn new(
        desktop: &Rc<Desktop>,
        binding: MountedBinding<CommandIpcState>,
        cx: &mut App,
    ) -> Entity<Self> {
        let listener = cx.new(|cx: &mut Context<Self>| Self {
            _subscription: binding
                .reactive()
                .observe(cx, |listener, _, cx| listener.start(cx)),
            binding,
            desktop: Rc::downgrade(desktop),
            task: None,
        });
        listener.update(cx, |listener, cx| listener.start(cx));
        listener
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.task.is_some() {
            return;
        }
        let SourceState::Ready(state) = self.binding.reactive().get(cx) else {
            return;
        };
        let desktop = self.desktop.clone();
        self.task = Some(cx.spawn(async move |_, cx| {
            while let Some(request) = state.next_request().await {
                cx.update(|cx| {
                    let response = if is_power_toggle(request.command(), request.payload()) {
                        if let Some(desktop) = desktop.upgrade() {
                            match desktop.toggle_power_on_outputs(cx) {
                                Ok(()) => b"ok".to_vec(),
                                Err(error) => format!("error: {error}").into_bytes(),
                            }
                        } else {
                            b"error: shell stopped".to_vec()
                        }
                    } else {
                        b"error: unsupported command".to_vec()
                    };
                    if let Err(error) = request.respond(response)
                        && let Some(desktop) = desktop.upgrade()
                    {
                        feedback::fail(
                            &desktop.feedback,
                            format!("外部からの操作に応答できません: {error}"),
                            cx,
                        );
                    }
                });
            }
        }));
    }
}

fn is_power_toggle(command: &str, payload: &[u8]) -> bool {
    command == "toggle" && payload == b"powermenu"
}

#[cfg(test)]
mod tests {
    use super::is_power_toggle;

    #[test]
    fn external_request_only_accepts_the_power_menu_payload() {
        assert!(is_power_toggle("toggle", b"powermenu"));
        assert!(!is_power_toggle("exec", b""));
        assert!(!is_power_toggle("toggle", b"powermenu\n"));
        assert!(!is_power_toggle("toggle", b""));
    }
}
