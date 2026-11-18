#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workspace {
    pub address: String,
    pub id: Option<i64>,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpecialWorkspace {
    pub address: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActiveWorkspace {
    Normal(Workspace),
    Special(SpecialWorkspace),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveWindow {
    pub address: String,
    pub title: String,
    pub class: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyprlandClient {
    pub address: String,
    pub class: String,
    pub workspace_address: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyprlandState {
    pub workspaces: Vec<Workspace>,
    pub special_workspaces: Vec<SpecialWorkspace>,
    pub clients: Vec<HyprlandClient>,
    pub active_workspace: Option<ActiveWorkspace>,
    pub active_window: Option<ActiveWindow>,
}
