use std::{pin::Pin, task::Poll};

use futures_lite::Stream;
use zbus::{
    Connection, Proxy,
    fdo::{self, DBusProxy},
    proxy::SignalStream,
};

use super::{
    TrayError,
    menu::{LayoutNode, parse_layout},
    state::{TrayActions, TrayIcon, TrayItem, TrayItemId, TrayStatus},
};

pub(super) const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
const MAX_ICON_SIDE: u32 = 512;
const MAX_ICON_BYTES: usize = 4 * 1024 * 1024;
const MAX_ICON_VARIANTS: usize = 32;
const MAX_ITEM_TEXT_BYTES: usize = 64 * 1024;

type ItemSignalStream = Pin<Box<SignalStream<'static>>>;
type ItemSignalStreams = Vec<(ItemSignalKind, ItemSignalStream)>;
type IconPixmap = (i32, i32, Vec<u8>);
type IconPixmaps = Vec<IconPixmap>;
type ToolTipProperty = (String, IconPixmaps, String, String);

#[derive(Clone, Copy, Debug)]
pub(super) enum ItemSignalKind {
    Item,
    Menu,
    Ended,
}

pub(super) struct ItemNotification {
    pub id: TrayItemId,
    pub kind: ItemSignalKind,
}

pub(super) struct ItemSignals {
    pub id: TrayItemId,
    streams: ItemSignalStreams,
}

impl ItemSignals {
    pub(super) async fn subscribe(
        connection: &Connection,
        id: TrayItemId,
    ) -> Result<Self, TrayError> {
        let proxy = Proxy::new(
            connection,
            id.owner.as_str(),
            id.path.as_str(),
            ITEM_INTERFACE,
        )
        .await?;
        let mut streams = Vec::new();
        for signal in [
            "NewTitle",
            "NewStatus",
            "NewIcon",
            "NewAttentionIcon",
            "NewOverlayIcon",
            "NewToolTip",
            "NewMenu",
        ] {
            streams.push((
                ItemSignalKind::Item,
                Box::pin(proxy.receive_signal(signal).await?),
            ));
        }
        let properties = Proxy::new(
            connection,
            id.owner.as_str(),
            id.path.as_str(),
            "org.freedesktop.DBus.Properties",
        )
        .await?;
        streams.push((
            ItemSignalKind::Item,
            Box::pin(properties.receive_signal("PropertiesChanged").await?),
        ));
        Ok(Self { id, streams })
    }

    pub(super) async fn subscribe_menu(
        &mut self,
        connection: &Connection,
        path: &str,
    ) -> Result<(), TrayError> {
        self.clear_menu();
        let proxy = Proxy::new_owned(
            connection.clone(),
            self.id.owner.clone(),
            path.to_owned(),
            MENU_INTERFACE,
        )
        .await?;
        for signal in ["LayoutUpdated", "ItemsPropertiesUpdated"] {
            self.streams.push((
                ItemSignalKind::Menu,
                Box::pin(proxy.receive_signal(signal).await?),
            ));
        }
        Ok(())
    }

    pub(super) fn clear_menu(&mut self) {
        self.streams
            .retain(|(kind, _)| !matches!(kind, ItemSignalKind::Menu));
    }

    pub(super) fn poll_next(&mut self, cx: &mut std::task::Context<'_>) -> Poll<ItemNotification> {
        for index in (0..self.streams.len()).rev() {
            let (kind, stream) = &mut self.streams[index];
            match stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(_)) => {
                    return Poll::Ready(ItemNotification {
                        id: self.id.clone(),
                        kind: *kind,
                    });
                }
                Poll::Ready(None) => {
                    let id = self.id.clone();
                    self.streams.remove(index);
                    return Poll::Ready(ItemNotification {
                        id,
                        kind: ItemSignalKind::Ended,
                    });
                }
                Poll::Pending => {}
            }
        }
        Poll::Pending
    }
}

pub(super) async fn read_item(
    connection: &Connection,
    id: TrayItemId,
    actions: &TrayActions,
    signals: &mut ItemSignals,
) -> Result<TrayItem, TrayError> {
    let proxy = Proxy::new(
        connection,
        id.owner.as_str(),
        id.path.as_str(),
        ITEM_INTERFACE,
    )
    .await?;
    let mut item = TrayItem::new(id.clone(), actions.clone());
    item.title = proxy.get_property("Title").await?;
    item.icon_name = proxy.get_property("IconName").await?;
    item.attention_icon_name = optional_property(&proxy, "AttentionIconName")
        .await?
        .unwrap_or_default();
    item.overlay_icon_name = optional_property(&proxy, "OverlayIconName")
        .await?
        .unwrap_or_default();
    item.attention_icon = decode_icon(
        &proxy
            .get_property::<IconPixmaps>("AttentionIconPixmap")
            .await?,
    )?;
    item.overlay_icon = decode_icon(
        &proxy
            .get_property::<IconPixmaps>("OverlayIconPixmap")
            .await?,
    )?;
    item.item_is_menu = proxy.get_property("ItemIsMenu").await?;
    item.status = match proxy.get_property::<String>("Status").await?.as_str() {
        "Active" => TrayStatus::Active,
        "Passive" => TrayStatus::Passive,
        "NeedsAttention" => TrayStatus::NeedsAttention,
        _ => TrayStatus::Unknown,
    };
    item.icon = decode_icon(&proxy.get_property::<IconPixmaps>("IconPixmap").await?)?;
    let (icon_name, tooltip_pixmaps, title, description): ToolTipProperty =
        proxy.get_property("ToolTip").await?;
    for text in [
        &item.title,
        &item.icon_name,
        &item.attention_icon_name,
        &item.overlay_icon_name,
        &title,
        &description,
        &icon_name,
    ] {
        if text.len() > MAX_ITEM_TEXT_BYTES {
            return Err(TrayError::Limit("item text"));
        }
    }
    item.tooltip = Some(super::TrayToolTip {
        title,
        description,
        icon_name,
        icon: decode_icon(&tooltip_pixmaps)?,
    });
    let menu: zbus::zvariant::OwnedObjectPath = proxy.get_property("Menu").await?;
    let menu_path = menu.as_str();
    if menu_path != "/" {
        item.menu_path = Some(menu_path.to_owned());
        signals.subscribe_menu(connection, menu_path).await?;
        let menu_proxy = Proxy::new_owned(
            connection.clone(),
            item.id.owner.clone(),
            menu,
            MENU_INTERFACE,
        )
        .await?;
        let (revision, layout) = menu_proxy
            .call::<_, _, (u32, LayoutNode)>("GetLayout", &(0_i32, -1_i32, Vec::<String>::new()))
            .await?;
        item.menu = Some(parse_layout(revision, layout)?);
    } else {
        signals.clear_menu();
    }
    Ok(item)
}

pub(super) async fn normalize_id(
    connection: &Connection,
    registration: &str,
) -> Result<Option<TrayItemId>, TrayError> {
    let (service, path) = match registration.split_once('/') {
        Some((service, path)) if !service.is_empty() => (service, format!("/{path}")),
        _ => (registration, "/StatusNotifierItem".to_owned()),
    };
    let bus_name = zbus::names::BusName::try_from(service)
        .map_err(|_| TrayError::Protocol("invalid item bus name"))?;
    let owner = DBusProxy::new(connection)
        .await?
        .get_name_owner(bus_name)
        .await
        .map_err(|error| TrayError::Bus(Box::new(zbus::Error::FDO(Box::new(error)))));
    let owner = match owner {
        Ok(owner) => owner,
        Err(TrayError::Bus(error))
            if matches!(error.as_ref(), zbus::Error::FDO(error)
                if matches!(error.as_ref(), fdo::Error::NameHasNoOwner(_))) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    Ok(Some(TrayItemId {
        owner: owner.to_string(),
        path,
    }))
}

async fn optional_property<T>(proxy: &Proxy<'_>, name: &str) -> Result<Option<T>, TrayError>
where
    T: TryFrom<zbus::zvariant::OwnedValue>,
    T::Error: Into<zbus::Error>,
{
    match proxy.get_property(name).await {
        Ok(value) => Ok(Some(value)),
        Err(zbus::Error::FDO(error)) if matches!(*error, fdo::Error::UnknownProperty(_)) => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn decode_icon(pixmaps: &[IconPixmap]) -> Result<Option<TrayIcon>, TrayError> {
    if pixmaps.len() > MAX_ICON_VARIANTS {
        return Err(TrayError::Limit("icon variant count"));
    }
    let mut selected: Option<(usize, u32, u32, &[u8])> = None;
    let mut total_bytes = 0_usize;
    for (width, height, bytes) in pixmaps {
        if *width <= 0 || *height <= 0 {
            return Err(TrayError::Protocol("non-positive icon dimensions"));
        }
        let (width, height) = (*width as u32, *height as u32);
        let byte_count = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(TrayError::Limit("icon payload"))?;
        if width > MAX_ICON_SIDE || height > MAX_ICON_SIDE || byte_count > MAX_ICON_BYTES {
            return Err(TrayError::Limit("icon dimensions or payload"));
        }
        total_bytes = total_bytes
            .checked_add(byte_count)
            .filter(|total| *total <= MAX_ICON_BYTES)
            .ok_or(TrayError::Limit("icon payload"))?;
        if bytes.len() != byte_count {
            return Err(TrayError::Protocol(
                "icon byte count does not match dimensions",
            ));
        }
        if selected
            .as_ref()
            .is_none_or(|candidate| byte_count > candidate.0)
        {
            selected = Some((byte_count, width, height, bytes.as_slice()));
        }
    }
    Ok(selected.map(|(_, width, height, bytes)| TrayIcon {
        width,
        height,
        rgba: bytes
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[1], pixel[2], pixel[3], pixel[0]])
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argb_icon_pixels_are_bounded_and_converted_to_rgba() {
        let icon = decode_icon(&[(1, 1, vec![255, 10, 20, 30])])
            .expect("well-formed icon")
            .expect("icon present");
        assert_eq!(icon.rgba, [10, 20, 30, 255]);
        assert!(matches!(
            decode_icon(&[(1, 1, vec![1, 2, 3])]),
            Err(TrayError::Protocol(_))
        ));
        assert!(matches!(
            decode_icon(&[(i32::MAX, i32::MAX, Vec::new())]),
            Err(TrayError::Limit(_))
        ));
    }
}
