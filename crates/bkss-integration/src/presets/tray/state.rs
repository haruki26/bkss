use crate::{Operation, OperationError};

use super::{TrayError, menu::TrayMenu};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TrayItemId {
    pub owner: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayStatus {
    Passive,
    Active,
    NeedsAttention,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayIcon {
    pub width: u32,
    pub height: u32,
    /// RGBA pixels, row-major.
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayToolTip {
    pub title: String,
    pub description: String,
    pub icon_name: String,
    pub icon: Option<TrayIcon>,
}

#[derive(Clone)]
pub struct TrayItem {
    pub id: TrayItemId,
    pub title: String,
    pub status: TrayStatus,
    pub icon_name: String,
    pub icon: Option<TrayIcon>,
    pub attention_icon_name: String,
    pub attention_icon: Option<TrayIcon>,
    pub overlay_icon_name: String,
    pub overlay_icon: Option<TrayIcon>,
    pub tooltip: Option<TrayToolTip>,
    pub menu_path: Option<String>,
    pub item_is_menu: bool,
    pub menu: Option<TrayMenu>,
    actions: TrayActions,
}

impl TrayItem {
    pub fn activate(&self, x: i32, y: i32) -> Result<TrayOperation<()>, crate::DispatchError> {
        self.actions.dispatch(TrayCommand::Activate {
            item: self.id.clone(),
            x,
            y,
        })
    }

    pub fn secondary_activate(
        &self,
        x: i32,
        y: i32,
    ) -> Result<TrayOperation<()>, crate::DispatchError> {
        self.actions.dispatch(TrayCommand::SecondaryActivate {
            item: self.id.clone(),
            x,
            y,
        })
    }

    pub fn context_menu(&self, x: i32, y: i32) -> Result<TrayOperation<()>, crate::DispatchError> {
        self.actions.dispatch(TrayCommand::ContextMenu {
            item: self.id.clone(),
            x,
            y,
        })
    }

    pub fn scroll(
        &self,
        delta: i32,
        orientation: impl Into<String>,
    ) -> Result<TrayOperation<()>, crate::DispatchError> {
        self.actions.dispatch(TrayCommand::Scroll {
            item: self.id.clone(),
            delta,
            orientation: orientation.into(),
        })
    }

    pub fn menu_event(
        &self,
        id: i32,
        event: impl Into<String>,
        timestamp: u32,
    ) -> Result<TrayOperation<()>, crate::DispatchError> {
        self.actions.dispatch(TrayCommand::MenuEvent {
            item: self.id.clone(),
            id,
            event: event.into(),
            timestamp,
        })
    }

    pub fn about_to_show(&self, id: i32) -> Result<TrayOperation<bool>, crate::DispatchError> {
        self.actions.dispatch(TrayCommand::AboutToShow {
            item: self.id.clone(),
            id,
        })
    }
}

#[derive(Clone, Default)]
pub struct TrayState {
    pub items: Vec<TrayItem>,
}

#[derive(Clone)]
pub(super) struct TrayActions(
    crate::base::command::CommandHandle<TrayCommand, TrayReply, TrayError>,
);

impl TrayActions {
    pub(super) fn new(
        handle: crate::base::command::CommandHandle<TrayCommand, TrayReply, TrayError>,
    ) -> Self {
        Self(handle)
    }

    fn dispatch<T>(
        &self,
        command: TrayCommand,
    ) -> Result<Operation<T, TrayError>, crate::DispatchError>
    where
        T: From<TrayReply> + Send + 'static,
    {
        let operation = self.0.dispatch(command)?;
        Ok(Box::pin(async move {
            operation
                .await
                .map(|reply| T::from(reply))
                .map_err(|error| match error {
                    OperationError::Inactive => OperationError::Inactive,
                    OperationError::Backend(error) => OperationError::Backend(error),
                })
        }))
    }
}

pub type TrayOperation<T> = Operation<T, TrayError>;

#[derive(Debug)]
pub(super) enum TrayCommand {
    Activate {
        item: TrayItemId,
        x: i32,
        y: i32,
    },
    SecondaryActivate {
        item: TrayItemId,
        x: i32,
        y: i32,
    },
    ContextMenu {
        item: TrayItemId,
        x: i32,
        y: i32,
    },
    Scroll {
        item: TrayItemId,
        delta: i32,
        orientation: String,
    },
    MenuEvent {
        item: TrayItemId,
        id: i32,
        event: String,
        timestamp: u32,
    },
    AboutToShow {
        item: TrayItemId,
        id: i32,
    },
}

pub(super) enum TrayReply {
    Done,
    NeedsUpdate(bool),
}

impl From<TrayReply> for () {
    fn from(_: TrayReply) {}
}

impl From<TrayReply> for bool {
    fn from(reply: TrayReply) -> Self {
        matches!(reply, TrayReply::NeedsUpdate(true))
    }
}

impl TrayItem {
    pub(super) fn new(id: TrayItemId, actions: TrayActions) -> Self {
        Self {
            id,
            title: String::new(),
            status: TrayStatus::Unknown,
            icon_name: String::new(),
            icon: None,
            attention_icon_name: String::new(),
            attention_icon: None,
            overlay_icon_name: String::new(),
            overlay_icon: None,
            tooltip: None,
            menu_path: None,
            item_is_menu: false,
            menu: None,
            actions,
        }
    }
}
