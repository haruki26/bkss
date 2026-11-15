use zbus::{Connection, Proxy, zvariant::OwnedObjectPath};

use super::{BluetoothError, error::bus};

pub(super) async fn bluez_owner(connection: &Connection) -> Result<Option<String>, BluetoothError> {
    let dbus = Proxy::new(
        connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await
    .map_err(|source| bus("create D-Bus daemon proxy", source))?;
    let has_owner: bool = dbus
        .call("NameHasOwner", &("org.bluez",))
        .await
        .map_err(|source| bus("check BlueZ service owner", source))?;
    if !has_owner {
        return Ok(None);
    }
    dbus.call("GetNameOwner", &("org.bluez",))
        .await
        .map(Some)
        .map_err(|source| bus("get BlueZ service owner", source))
}

pub(super) async fn snapshot(
    connection: &Connection,
) -> Result<zbus::fdo::ManagedObjects, BluetoothError> {
    let owner = bluez_owner(connection).await?;
    if owner.is_none() {
        return Ok(Default::default());
    }
    let dbus = Proxy::new(
        connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await
    .map_err(|source| bus("create D-Bus daemon proxy", source))?;

    let manager = Proxy::new(
        connection,
        "org.bluez",
        "/",
        "org.freedesktop.DBus.ObjectManager",
    )
    .await
    .map_err(|source| bus("create BlueZ ObjectManager proxy", source))?;
    match manager.call("GetManagedObjects", &()).await {
        Ok(objects) => Ok(objects),
        Err(source) => {
            let owner: bool = dbus
                .call("NameHasOwner", &("org.bluez",))
                .await
                .map_err(|owner_error| bus("recheck BlueZ service owner", owner_error))?;
            if owner {
                Err(bus("get BlueZ managed objects", source))
            } else {
                Ok(Default::default())
            }
        }
    }
}

pub(super) async fn call(
    connection: &Connection,
    path: &str,
    interface: &'static str,
    method: &str,
) -> Result<(), BluetoothError> {
    let path = OwnedObjectPath::try_from(path)
        .map_err(|_| BluetoothError::InvalidObjectPath(path.into()))?;
    let proxy = Proxy::new(connection, "org.bluez", path, interface)
        .await
        .map_err(|source| bus("create BlueZ operation proxy", source))?;
    proxy
        .call(method, &())
        .await
        .map_err(|source| bus("call BlueZ operation", source))
}

pub(super) async fn set_property(
    connection: &Connection,
    path: &str,
    interface: &'static str,
    property: &str,
    value: bool,
) -> Result<(), BluetoothError> {
    let path = OwnedObjectPath::try_from(path)
        .map_err(|_| BluetoothError::InvalidObjectPath(path.into()))?;
    let proxy = Proxy::new(connection, "org.bluez", path, interface)
        .await
        .map_err(|source| bus("create BlueZ property proxy", source))?;
    proxy
        .set_property(property, value)
        .await
        .map_err(|source| bus("set BlueZ property", source.into()))
}

pub(super) async fn call_with_path(
    connection: &Connection,
    path: &str,
    interface: &'static str,
    method: &str,
    argument: String,
) -> Result<(), BluetoothError> {
    let path = OwnedObjectPath::try_from(path)
        .map_err(|_| BluetoothError::InvalidObjectPath(path.into()))?;
    let argument = OwnedObjectPath::try_from(argument.as_str())
        .map_err(|_| BluetoothError::InvalidObjectPath(argument))?;
    let proxy = Proxy::new(connection, "org.bluez", path, interface)
        .await
        .map_err(|source| bus("create BlueZ operation proxy", source))?;
    proxy
        .call(method, &(argument,))
        .await
        .map_err(|source| bus("call BlueZ operation", source))
}
