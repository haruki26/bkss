#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workspace {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveWindow {
    pub address: String,
    pub title: String,
    pub class: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyprlandState {
    pub workspaces: Vec<Workspace>,
    pub active_workspace: Option<Workspace>,
    pub active_window: Option<ActiveWindow>,
}
