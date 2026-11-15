//! Typed integrations that compose with BKSS's existing resource lifecycle.
//!
//! Presets expose typed values with an [`IntegrationView`] implementation. `Clock::new` returns a
//! source, so wrap it with [`SourceDescriptor::new`]; `UsageSource::new` and
//! [`Hyprland::new`] return descriptors directly. Descriptors are inert until
//! mounted. Clock updates align to their selected local-time boundary, usage
//! sources sample once per second, and a Hyprland descriptor watches the
//! supplied socket directory.
//!
//! Workspace changes run off-thread and return an awaitable result:
//!
//! ```no_run
//! use bkss_integration::Hyprland;
//! # async fn switch(socket_dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
//! Hyprland::switch_workspace(socket_dir, "2")?.await?;
//! # Ok(())
//! # }
//! ```
//!
//! A custom typed source uses the same lifecycle and element-rendering contract:
//!
//! ```
//! use bkss_integration::{IntegrationView, Source, SourceDescriptor, SourcePublisher};
//! use gpui::{AnyElement, App, Window, div, prelude::*};
//! use std::io;
//!
//! #[derive(Clone)]
//! struct Temperature(f32);
//! impl IntegrationView for Temperature {
//!     fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
//!         div().child(format!("{:.1}°", self.0)).into_any_element()
//!     }
//! }
//!
//! struct Thermometer;
//! impl Source for Thermometer {
//!     type Key = ();
//!     type Value = Temperature;
//!     type Error = io::Error;
//!     type Guard = ();
//!     fn key(&self) {}
//!     fn start(
//!         &self,
//!         publisher: SourcePublisher<Self>,
//!         cx: &mut App,
//!     ) -> Result<Self::Guard, Self::Error> {
//!         publisher.publish(Temperature(21.5), cx);
//!         Ok(())
//!     }
//! }
//!
//! let descriptor = SourceDescriptor::new(Thermometer);
//! ```
//!
//! The resource scope activates a descriptor on first mount and shares matching
//! source keys; the mounted binding owns the consumer lifetime.
pub mod base;
mod presets;

pub use base::{
    DispatchError, IntegrationView, Operation, OperationError, RespondError, Responder, Secret,
    WorkerGuard,
};
pub use bkss_core::{
    Source, SourceDescriptor, SourcePublisher, SourceState, ThreadPublisher, ThreadPublisherGuard,
};
pub use presets::{
    ActiveWindow, Audio, AudioControl, AudioControlError, AudioDevice, AudioDeviceState, AudioError,
    AudioState, AuthorizationAnswer, Backlight, BacklightError, BacklightSource, BacklightState,
    Battery, BatteryEndpoint, BatteryError, BatterySource, BatteryState, Bluetooth, BluetoothAdapter,
    BluetoothAgentDisplay, BluetoothAgentRequest, BluetoothAuthInputError, BluetoothConfig,
    BluetoothDevice, BluetoothError, BluetoothState, ChargeState, Clock, ClockPrecision, ClockState,
    ConfirmationAnswer, Gpu, GpuError, GpuMetrics, GpuSource, GpuState, GpuSupport, Hyprland,
    HyprlandError, HyprlandState, PasskeyAnswer, PinCodeAnswer, Usage, UsageKind, UsageSource, Wifi,
    WifiAccessPoint, WifiAuthContext, WifiAuthRequest, WifiCredentialField, WifiCredentials, WifiDevice,
    WifiDeviceState, WifiError, WifiOperations, WifiSavedConnection, WifiSecurity, WifiState, Workspace,
    WorkspaceSwitch,
};
