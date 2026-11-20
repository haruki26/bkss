use std::{fmt::Display, future::Future};

use gpui::{App, Entity, IntoElement, ParentElement, Styled, rgb, rgba};

use super::tray::RefreshFeedback;
use super::{
    shared::{button, label, row},
    theme::DANGER,
};

#[derive(Default)]
pub struct Feedback {
    pub error: Option<String>,
    pub tray_refresh: RefreshFeedback,
}

pub fn fail(feedback: &Entity<Feedback>, message: impl Into<String>, cx: &mut App) {
    feedback.update(cx, |state, cx| {
        state.error = Some(message.into());
        cx.notify();
    });
}

pub fn dispatch<F, E, D>(
    operation: Result<F, D>,
    context: &'static str,
    feedback: &Entity<Feedback>,
    cx: &mut App,
) where
    F: Future<Output = Result<(), E>> + 'static,
    E: Display,
    D: Display,
{
    match operation {
        Err(error) => fail(feedback, format!("{context}: {error}"), cx),
        Ok(operation) => {
            let feedback = feedback.downgrade();
            cx.spawn(async move |cx| {
                if let Err(error) = operation.await {
                    let message = format!("{context}: {error}");
                    cx.update(|cx| {
                        if let Some(feedback) = feedback.upgrade() {
                            fail(&feedback, message, cx);
                        }
                    });
                }
            })
            .detach();
        }
    }
}

pub fn error_banner(feedback: &Entity<Feedback>, cx: &App) -> Option<impl IntoElement> {
    feedback.read(cx).error.as_ref().map(|message| {
        let feedback = feedback.clone();
        row()
            .w_full()
            .rounded_lg()
            .px_3()
            .py_2()
            .bg(rgba(0xf38ba824))
            .text_color(rgb(DANGER))
            .child(label(message.clone()).flex_1().min_w_0())
            .child(
                button("dismiss-error", "エラーを閉じる", move |_, cx| {
                    feedback.update(cx, |state, cx| {
                        state.error = None;
                        cx.notify();
                    });
                })
                .child("×"),
            )
    })
}
