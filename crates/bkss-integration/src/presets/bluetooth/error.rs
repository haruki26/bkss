use std::{error::Error, fmt};

#[derive(Debug)]
pub enum BluetoothError {
    Bus {
        operation: &'static str,
        source: Box<zbus::Error>,
    },
    InvalidObjectPath(String),
    InvalidProperty {
        interface: &'static str,
        property: &'static str,
    },
    Worker(std::io::Error),
    Inactive,
}

impl fmt::Display for BluetoothError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus { operation, source } => write!(f, "{operation}: {source}"),
            Self::InvalidObjectPath(path) => write!(f, "invalid BlueZ object path: {path}"),
            Self::InvalidProperty {
                interface,
                property,
            } => {
                write!(f, "invalid {interface}.{property} property")
            }
            Self::Worker(source) => write!(f, "start Bluetooth worker: {source}"),
            Self::Inactive => f.write_str("Bluetooth activation is inactive"),
        }
    }
}

impl Error for BluetoothError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bus { source, .. } => Some(source.as_ref()),
            Self::Worker(source) => Some(source),
            Self::InvalidObjectPath(_) | Self::InvalidProperty { .. } | Self::Inactive => None,
        }
    }
}

pub(super) fn bus(operation: &'static str, source: zbus::Error) -> BluetoothError {
    BluetoothError::Bus {
        operation,
        source: Box::new(source),
    }
}
