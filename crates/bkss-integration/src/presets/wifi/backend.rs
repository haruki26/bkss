use super::{
    error::WifiError,
    state::{
        WifiAccessPoint, WifiDevice, WifiDeviceState, WifiSavedConnection, WifiSecurity, WifiState,
        ssid_display,
    },
};
use std::collections::HashMap;
use zbus::{Connection, Proxy, zvariant::OwnedObjectPath};

pub(crate) const NM: &str = "org.freedesktop.NetworkManager";
pub(crate) const NM_PATH: &str = "/org/freedesktop/NetworkManager";
pub(crate) const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const WIFI_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";

pub(crate) async fn snapshot(
    connection: &Connection,
    operations: super::state::WifiOperations,
    auth_requests: super::state::AuthRequests,
) -> Result<WifiState, WifiError> {
    let manager = Proxy::new(connection, NM, NM_PATH, NM_IFACE).await?;
    let networking_enabled = manager.get_property("NetworkingEnabled").await?;
    let radio_enabled = manager.get_property("WirelessEnabled").await?;
    let paths: Vec<OwnedObjectPath> = manager.call("GetDevices", &()).await?;
    let mut devices = Vec::new();
    let mut active_connection = None;
    for path in paths {
        let path = path.as_str();
        let device = Proxy::new(connection, NM, path, DEVICE_IFACE).await?;
        let kind: u32 = device.get_property("DeviceType").await?;
        if kind != 2 {
            continue;
        }
        let interface: String = device.get_property("Interface").await?;
        let state: u32 = device.get_property("State").await?;
        let active_path: OwnedObjectPath = device.get_property("ActiveConnection").await?;
        if active_path.as_str() != "/" {
            active_connection = Some(active_path.to_string());
        }
        let wireless = Proxy::new(connection, NM, path, WIFI_IFACE).await?;
        let ap_paths: Vec<OwnedObjectPath> = wireless.call("GetAllAccessPoints", &()).await?;
        let active_ap: OwnedObjectPath = wireless.get_property("ActiveAccessPoint").await?;
        let mut access_points = Vec::with_capacity(ap_paths.len());
        for ap_path in ap_paths {
            let ap = Proxy::new(connection, NM, ap_path.as_str(), AP_IFACE).await?;
            let ssid: Vec<u8> = ap.get_property("Ssid").await?;
            let strength: u8 = ap.get_property("Strength").await?;
            let frequency_mhz: u32 = ap.get_property("Frequency").await?;
            let wpa: u32 = ap.get_property("WpaFlags").await?;
            let rsn: u32 = ap.get_property("RsnFlags").await?;
            access_points.push(WifiAccessPoint {
                path: ap_path.to_string(),
                display_name: ssid_display(&ssid),
                ssid: ssid.into(),
                strength,
                frequency_mhz,
                security: security(wpa, rsn),
            });
        }
        devices.push(WifiDevice {
            path: path.to_owned(),
            interface,
            state: WifiDeviceState::from(state),
            access_points,
            active_access_point: (active_ap.as_str() != "/").then(|| active_ap.to_string()),
        });
    }
    let settings = Proxy::new(
        connection,
        NM,
        SETTINGS_PATH,
        "org.freedesktop.NetworkManager.Settings",
    )
    .await?;
    let saved_paths: Vec<OwnedObjectPath> = settings.call("ListConnections", &()).await?;
    let mut saved_connections = Vec::new();
    for path in saved_paths {
        let saved = Proxy::new(
            connection,
            NM,
            path.as_str(),
            "org.freedesktop.NetworkManager.Settings.Connection",
        )
        .await?;
        let settings: HashMap<String, HashMap<String, zbus::zvariant::OwnedValue>> =
            saved.call("GetSettings", &()).await?;
        let connection_settings = settings.get("connection");
        let connection_type = connection_settings
            .and_then(|values| values.get("type"))
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok());
        if connection_type.as_deref() != Some("802-11-wireless") {
            continue;
        }
        let id = connection_settings
            .and_then(|values| values.get("id"))
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
            .unwrap_or_else(|| "Unnamed connection".to_owned());
        let ssid = settings
            .get("802-11-wireless")
            .and_then(|values| values.get("ssid"))
            .and_then(|value| Vec::<u8>::try_from(value.try_clone().ok()?).ok())
            .map(Into::into);
        let display_name = ssid.as_deref().map(ssid_display);
        saved_connections.push(WifiSavedConnection {
            path: path.to_string(),
            id,
            ssid,
            display_name,
        });
    }
    Ok(WifiState {
        networking_enabled,
        radio_enabled,
        devices,
        saved_connections,
        active_connection,
        operations,
        auth_requests,
    })
}

pub(crate) async fn set_radio(connection: &Connection, enabled: bool) -> Result<(), WifiError> {
    let manager = Proxy::new(connection, NM, NM_PATH, NM_IFACE).await?;
    manager
        .set_property("WirelessEnabled", &enabled)
        .await
        .map_err(zbus::Error::from)?;
    Ok(())
}

pub(crate) async fn disconnect(
    connection: &Connection,
    device_path: &str,
) -> Result<(), WifiError> {
    let device = Proxy::new(connection, NM, device_path, DEVICE_IFACE).await?;
    device.call::<_, _, ()>("Disconnect", &()).await?;
    Ok(())
}

pub(crate) async fn connect_saved(
    connection: &Connection,
    connection_path: &str,
) -> Result<(), WifiError> {
    let manager = Proxy::new(connection, NM, NM_PATH, NM_IFACE).await?;
    let connection_path = object_path(connection_path, "activate saved connection")?;
    let default_path = object_path("/", "activate saved connection")?;
    let _: OwnedObjectPath = manager
        .call(
            "ActivateConnection",
            &(&connection_path, &default_path, &default_path),
        )
        .await?;
    Ok(())
}

pub(crate) async fn connect_new(
    connection: &Connection,
    access_point_path: &str,
) -> Result<(), WifiError> {
    let manager = Proxy::new(connection, NM, NM_PATH, NM_IFACE).await?;
    let paths: Vec<OwnedObjectPath> = manager.call("GetDevices", &()).await?;
    let mut selected = None;
    for path in paths {
        let device = Proxy::new(connection, NM, path.as_str(), DEVICE_IFACE).await?;
        let kind: u32 = device.get_property("DeviceType").await?;
        if kind != 2 {
            continue;
        }
        let wireless = Proxy::new(connection, NM, path.as_str(), WIFI_IFACE).await?;
        let access_points: Vec<OwnedObjectPath> = wireless.call("GetAllAccessPoints", &()).await?;
        if access_points
            .iter()
            .any(|path| path.as_str() == access_point_path)
        {
            selected = Some(path);
            break;
        }
    }
    let device_path = selected.ok_or_else(|| WifiError::Protocol {
        operation: "connect to access point",
        message: "access point is no longer available".into(),
    })?;
    let ap = Proxy::new(connection, NM, access_point_path, AP_IFACE).await?;
    let ssid: Vec<u8> = ap.get_property("Ssid").await?;
    let wpa: u32 = ap.get_property("WpaFlags").await?;
    let rsn: u32 = ap.get_property("RsnFlags").await?;
    let mut settings: HashMap<&str, HashMap<&str, zbus::zvariant::Value<'_>>> = HashMap::new();
    let mut connection_settings = HashMap::new();
    connection_settings.insert("id", zbus::zvariant::Value::from("BKSS temporary Wi-Fi"));
    connection_settings.insert("type", zbus::zvariant::Value::from("802-11-wireless"));
    connection_settings.insert("autoconnect", zbus::zvariant::Value::from(false));
    settings.insert("connection", connection_settings);
    let mut wireless = HashMap::new();
    wireless.insert("ssid", zbus::zvariant::Value::from(ssid));
    settings.insert("802-11-wireless", wireless);
    let flags = wpa | rsn;
    if flags != 0 {
        let mut security = HashMap::new();
        let key_mgmt = if flags & 0x200 != 0 {
            "wpa-eap"
        } else if rsn & 0x400 != 0 {
            "sae"
        } else if flags & 0x33 != 0 {
            "none"
        } else {
            "wpa-psk"
        };
        security.insert("key-mgmt", zbus::zvariant::Value::from(key_mgmt));
        settings.insert("802-11-wireless-security", security);
    }
    let mut options = HashMap::new();
    options.insert("persist", zbus::zvariant::Value::from("memory"));
    let access_point_path = object_path(access_point_path, "connect to access point")?;
    let (_connection_path, _active_connection, _result): (
        OwnedObjectPath,
        OwnedObjectPath,
        HashMap<String, zbus::zvariant::OwnedValue>,
    ) = manager
        .call(
            "AddAndActivateConnection2",
            &(settings, device_path, access_point_path, options),
        )
        .await?;
    Ok(())
}

fn object_path(path: &str, operation: &'static str) -> Result<OwnedObjectPath, WifiError> {
    OwnedObjectPath::try_from(path).map_err(|error| WifiError::Protocol {
        operation,
        message: format!("invalid NetworkManager object path: {error}"),
    })
}

fn security(wpa: u32, rsn: u32) -> WifiSecurity {
    let flags = wpa | rsn;
    if flags == 0 {
        WifiSecurity::Open
    } else if flags & !0x1fff != 0 {
        WifiSecurity::Unknown
    } else if flags & 0x1c00 != 0 {
        WifiSecurity::Wpa3
    } else if flags & 0x200 != 0 {
        WifiSecurity::Wpa
    } else if rsn & 0x100 != 0 {
        WifiSecurity::Wpa2
    } else if wpa & 0x100 != 0 {
        WifiSecurity::Wpa
    } else if flags & 0x33 != 0 {
        WifiSecurity::Wep
    } else {
        WifiSecurity::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_flags_are_mapped_without_assuming_utf8_ssids() {
        assert_eq!(security(0, 0), WifiSecurity::Open);
        assert_eq!(security(2, 0), WifiSecurity::Wep);
        assert_eq!(security(0x100, 0), WifiSecurity::Wpa);
        assert_eq!(security(0, 0x100), WifiSecurity::Wpa2);
        assert_eq!(security(0, 0x400), WifiSecurity::Wpa3);
        assert_eq!(security(0, 0x8000), WifiSecurity::Unknown);
    }
}
