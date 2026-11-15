use std::collections::HashMap;

use serde::Deserialize;
use zbus::zvariant::{OwnedValue, Structure, Type, Value};

use super::TrayError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayToggleState {
    Off,
    On,
    Indeterminate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayMenuItem {
    pub id: i32,
    pub label: String,
    pub visible: bool,
    pub enabled: bool,
    pub separator: bool,
    pub icon_name: Option<String>,
    pub icon_data: Option<Vec<u8>>,
    pub toggle_type: Option<String>,
    pub toggle_state: Option<TrayToggleState>,
    pub children: Vec<Self>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayMenu {
    pub revision: u32,
    pub items: Vec<TrayMenuItem>,
}

pub(super) const MAX_MENU_DEPTH: usize = 16;
pub(super) const MAX_MENU_NODES: usize = 512;
pub(super) const MAX_MENU_BYTES: usize = 1024 * 1024;

#[derive(Deserialize, Type)]
pub(super) struct LayoutNode(i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

impl TryFrom<OwnedValue> for LayoutNode {
    type Error = zbus::zvariant::Error;

    fn try_from(value: OwnedValue) -> Result<Self, Self::Error> {
        let structure = Value::from(value).downcast::<Structure>()?;
        let mut fields = structure.into_fields().into_iter();
        let id = i32::try_from(fields.next().ok_or(zbus::zvariant::Error::IncorrectType)?)?;
        let properties = HashMap::<String, OwnedValue>::try_from(
            fields.next().ok_or(zbus::zvariant::Error::IncorrectType)?,
        )?;
        let children = Vec::<OwnedValue>::try_from(
            fields.next().ok_or(zbus::zvariant::Error::IncorrectType)?,
        )?;
        if fields.next().is_some() {
            return Err(zbus::zvariant::Error::IncorrectType);
        }
        Ok(Self(id, properties, children))
    }
}

pub(super) fn parse_layout(revision: u32, root: LayoutNode) -> Result<TrayMenu, TrayError> {
    let mut count = 0;
    let mut bytes = 0;
    let items = parse_children(root.2, 1, &mut count, &mut bytes)?;
    Ok(TrayMenu { revision, items })
}

fn parse_children(
    children: Vec<OwnedValue>,
    depth: usize,
    count: &mut usize,
    bytes: &mut usize,
) -> Result<Vec<TrayMenuItem>, TrayError> {
    if depth > MAX_MENU_DEPTH {
        return Err(TrayError::Limit("menu depth"));
    }
    children
        .into_iter()
        .map(|child| {
            let node = LayoutNode::try_from(child)
                .map_err(|_| TrayError::Protocol("malformed DBusMenu child node"))?;
            parse_node(node, depth, count, bytes)
        })
        .collect()
}

fn parse_node(
    node: LayoutNode,
    depth: usize,
    count: &mut usize,
    bytes: &mut usize,
) -> Result<TrayMenuItem, TrayError> {
    *count += 1;
    if *count > MAX_MENU_NODES {
        return Err(TrayError::Limit("menu node count"));
    }
    let LayoutNode(id, properties, children) = node;
    let label = get::<String>(&properties, "label").unwrap_or_default();
    let visible = get::<bool>(&properties, "visible").unwrap_or(true);
    let enabled = get::<bool>(&properties, "enabled").unwrap_or(true);
    let separator = get::<String>(&properties, "type").as_deref() == Some("separator");
    let icon_name = get::<String>(&properties, "icon-name").filter(|name| !name.is_empty());
    let icon_data = get::<Vec<u8>>(&properties, "icon-data").filter(|data| !data.is_empty());
    let toggle_type = get::<String>(&properties, "toggle-type").filter(|value| !value.is_empty());
    let toggle_state = get::<i32>(&properties, "toggle-state").map(|value| match value {
        0 => TrayToggleState::Off,
        1 => TrayToggleState::On,
        _ => TrayToggleState::Indeterminate,
    });
    *bytes = bytes
        .saturating_add(label.len())
        .saturating_add(icon_name.as_ref().map_or(0, String::len))
        .saturating_add(icon_data.as_ref().map_or(0, Vec::len));
    if *bytes > MAX_MENU_BYTES {
        return Err(TrayError::Limit("menu payload"));
    }
    Ok(TrayMenuItem {
        id,
        label,
        visible,
        enabled,
        separator,
        icon_name,
        icon_data,
        toggle_type,
        toggle_state,
        children: parse_children(children, depth + 1, count, bytes)?,
    })
}

fn get<T>(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<T>
where
    T: TryFrom<OwnedValue>,
{
    properties.get(key)?.clone().try_into().ok()
}
