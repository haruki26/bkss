use std::{
    error::Error,
    fmt,
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use bkss_core::{Source, SourceDescriptor, SourcePublisher, ThreadPublisher, ThreadPublisherGuard};
use bkss_integration::Secret;
use gpui::App;
use serde::Deserialize;

use crate::settings::{self, Settings};

const UNKNOWN_ICON: &str = "yh-weather-unknown-symbolic";
const UPDATE_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeatherState {
    pub icon_name: String,
    pub temperature: String,
    pub status: WeatherStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WeatherStatus {
    Loading,
    Ready,
    NotConfigured,
    Error,
}

impl Default for WeatherState {
    fn default() -> Self {
        Self {
            icon_name: UNKNOWN_ICON.into(),
            temperature: "--".into(),
            status: WeatherStatus::Loading,
        }
    }
}

#[derive(Debug)]
pub struct WeatherError(&'static str);

impl fmt::Display for WeatherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for WeatherError {}

pub struct WeatherSource {
    key: Option<Secret<String>>,
    settings_error: bool,
}

impl WeatherSource {
    pub fn new() -> SourceDescriptor<Self> {
        let settings = Settings::load();
        SourceDescriptor::new(Self {
            key: settings::weather_api_key(settings.weather_api_key.as_ref()),
            settings_error: settings.error.is_some(),
        })
    }
}

impl Source for WeatherSource {
    type Key = ();
    type Value = WeatherState;
    type Error = WeatherError;
    type Guard = WeatherGuard;

    fn key(&self) {}

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (publisher, bridge) = publisher.thread_bridge(|value| value, |error| error, cx);
        let key = self
            .key
            .as_ref()
            .map(|key| Secret::new(key.expose().clone()));
        let settings_error = self.settings_error;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker = thread::Builder::new()
            .name("shell-weather".into())
            .spawn(move || {
                let result =
                    weather_worker(publisher.clone(), key, settings_error, &worker_cancelled);
                if let Err(error) = result
                    && !worker_cancelled.load(Ordering::Acquire)
                {
                    let _ = publisher.fail(error);
                }
            })
            .map_err(|_| WeatherError("Could not start weather worker"))?;
        Ok(WeatherGuard {
            cancelled,
            bridge: Some(bridge),
            worker: Some(worker),
        })
    }
}

fn weather_worker(
    publisher: ThreadPublisher<WeatherState, WeatherError>,
    key: Option<Secret<String>>,
    settings_error: bool,
    cancelled: &AtomicBool,
) -> Result<(), WeatherError> {
    if settings_error {
        publish_weather(
            &publisher,
            WeatherState {
                temperature: "ERR".into(),
                status: WeatherStatus::Error,
                ..WeatherState::default()
            },
        )?;
        return Ok(());
    }
    let Some(key) = key else {
        publish_weather(
            &publisher,
            WeatherState {
                status: WeatherStatus::NotConfigured,
                ..WeatherState::default()
            },
        )?;
        return Ok(());
    };
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let result = resolve_location(cancelled).and_then(|coordinates| {
            let mut result = Err(WeatherError("Weather request failed"));
            for attempt in 0..3 {
                result = fetch_weather(key.expose(), coordinates, cancelled);
                if result.is_ok() || cancelled.load(Ordering::Acquire) {
                    break;
                }
                if attempt < 2 && !wait_or_cancel(Duration::from_secs(10), cancelled) {
                    return Err(WeatherError("Weather request cancelled"));
                }
            }
            result
        });
        if cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let state = result.unwrap_or_else(|_| WeatherState {
            temperature: "ERR".into(),
            status: WeatherStatus::Error,
            ..WeatherState::default()
        });
        publish_weather(&publisher, state)?;
        if !wait_or_cancel(UPDATE_INTERVAL, cancelled) {
            return Ok(());
        }
    }
}

fn publish_weather(
    publisher: &ThreadPublisher<WeatherState, WeatherError>,
    state: WeatherState,
) -> Result<(), WeatherError> {
    publisher
        .publish(state)
        .map_err(|_| WeatherError("Weather source is inactive"))
}

fn fetch_weather(
    key: &str,
    coordinates: (f64, f64),
    cancelled: &AtomicBool,
) -> Result<WeatherState, WeatherError> {
    if key.trim().is_empty() {
        return Err(WeatherError("Weather is not configured"));
    }
    let url = format!(
        "https://api.openweathermap.org/data/2.5/weather?lat={:.4}&lon={:.4}&appid={}&units=metric&lang=ja",
        coordinates.0,
        coordinates.1,
        percent_encode(key)
    );
    let output = curl(&url, cancelled)?;
    let response: WeatherResponse =
        serde_json::from_slice(&output).map_err(|_| WeatherError("Invalid weather response"))?;
    let weather = response
        .weather
        .first()
        .ok_or(WeatherError("Weather response has no condition"))?;
    let icon_name = weather_icon(&weather.icon).to_owned();
    let temperature = response
        .main
        .temp
        .filter(|temperature| temperature.is_finite())
        .map(|temperature| format!("{}℃", temperature.round() as i32))
        .ok_or(WeatherError("Weather response has no temperature"))?;
    Ok(WeatherState {
        icon_name,
        temperature,
        status: WeatherStatus::Ready,
    })
}

#[derive(Deserialize)]
struct WeatherResponse {
    weather: Vec<WeatherCondition>,
    main: WeatherTemperature,
}

#[derive(Deserialize)]
struct WeatherCondition {
    icon: String,
}

#[derive(Deserialize)]
struct WeatherTemperature {
    temp: Option<f64>,
}

fn weather_icon(code: &str) -> &'static str {
    match code {
        "01d" => "yh-weather-clear-day-symbolic",
        "01n" => "yh-weather-clear-night-symbolic",
        "02d" => "yh-weather-cloudy-day-symbolic",
        "02n" => "yh-weather-cloudy-night-symbolic",
        "03d" | "04d" => "yh-weather-cloudy-day-symbolic",
        "03n" | "04n" => "yh-weather-cloudy-night-symbolic",
        "09d" | "09n" => "yh-weather-shower-rain-symbolic",
        "10d" => "yh-weather-rain-day-symbolic",
        "10n" => "yh-weather-rain-night-symbolic",
        "11d" | "11n" => "yh-weather-thunder-symbolic",
        "13d" | "13n" => "yh-weather-snow-symbolic",
        "50d" | "50n" => "yh-weather-fog-symbolic",
        _ => UNKNOWN_ICON,
    }
}

fn resolve_location(cancelled: &AtomicBool) -> Result<(f64, f64), WeatherError> {
    if let Ok(location) =
        futures_lite::future::block_on(futures_lite::future::race(geoclue_location(), async {
            async_io::Timer::after(Duration::from_secs(15)).await;
            Err(WeatherError("GeoClue request timed out"))
        }))
    {
        return Ok(location);
    }
    let output = curl("https://ipwho.is/", cancelled)?;
    let location: IpLocation =
        serde_json::from_slice(&output).map_err(|_| WeatherError("Invalid location response"))?;
    match (location.success, location.latitude, location.longitude) {
        (Some(false), _, _) | (_, None, _) | (_, _, None) => {
            Err(WeatherError("Location is unavailable"))
        }
        (_, Some(latitude), Some(longitude)) if valid_coordinates(latitude, longitude) => {
            Ok((latitude, longitude))
        }
        _ => Err(WeatherError("Invalid location coordinates")),
    }
}

#[derive(Deserialize)]
struct IpLocation {
    success: Option<bool>,
    latitude: Option<f64>,
    longitude: Option<f64>,
}

fn valid_coordinates(latitude: f64, longitude: f64) -> bool {
    latitude.is_finite()
        && longitude.is_finite()
        && (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude)
}

async fn geoclue_location() -> Result<(f64, f64), WeatherError> {
    use zbus::{Connection, Proxy};

    let connection = Connection::system()
        .await
        .map_err(|_| WeatherError("Could not connect to GeoClue"))?;
    let manager = Proxy::new(
        &connection,
        "org.freedesktop.GeoClue2",
        "/org/freedesktop/GeoClue2/Manager",
        "org.freedesktop.GeoClue2.Manager",
    )
    .await
    .map_err(|_| WeatherError("Could not access GeoClue"))?;
    let client: zbus::zvariant::OwnedObjectPath = manager
        .call("GetClient", &())
        .await
        .map_err(|_| WeatherError("GeoClue client unavailable"))?;
    let client_proxy = Proxy::new(
        &connection,
        "org.freedesktop.GeoClue2",
        client.as_str(),
        "org.freedesktop.GeoClue2.Client",
    )
    .await
    .map_err(|_| WeatherError("GeoClue client unavailable"))?;
    client_proxy
        .set_property("DesktopId", "ags-topbar")
        .await
        .map_err(|_| WeatherError("Could not configure GeoClue"))?;
    client_proxy
        .set_property("RequestedAccuracyLevel", 4u32)
        .await
        .map_err(|_| WeatherError("Could not configure GeoClue"))?;
    client_proxy
        .call::<_, _, ()>("Start", &())
        .await
        .map_err(|_| WeatherError("GeoClue location request failed"))?;
    let location: zbus::zvariant::OwnedObjectPath = client_proxy
        .get_property("Location")
        .await
        .map_err(|_| WeatherError("GeoClue location unavailable"))?;
    let location_proxy = Proxy::new(
        &connection,
        "org.freedesktop.GeoClue2",
        location.as_str(),
        "org.freedesktop.GeoClue2.Location",
    )
    .await
    .map_err(|_| WeatherError("GeoClue location unavailable"))?;
    let latitude: f64 = location_proxy
        .get_property("Latitude")
        .await
        .map_err(|_| WeatherError("GeoClue location unavailable"))?;
    let longitude: f64 = location_proxy
        .get_property("Longitude")
        .await
        .map_err(|_| WeatherError("GeoClue location unavailable"))?;
    let _ = client_proxy.call::<_, _, ()>("Stop", &()).await;
    if valid_coordinates(latitude, longitude) {
        Ok((latitude, longitude))
    } else {
        Err(WeatherError("Invalid GeoClue coordinates"))
    }
}

fn curl(url: &str, cancelled: &AtomicBool) -> Result<Vec<u8>, WeatherError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(WeatherError("Weather request cancelled"));
    }
    let mut child = Command::new("curl")
        .args([
            "--config",
            "-",
            "--silent",
            "--show-error",
            "--fail",
            "--max-time",
            "20",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| WeatherError("Could not start curl"))?;
    let config = format!("url = \"{}\"\n", config_escape(url));
    if let Some(mut stdin) = child.stdin.take()
        && stdin.write_all(config.as_bytes()).is_err()
    {
        terminate(&mut child);
        return Err(WeatherError("Could not configure curl"));
    }
    let Some(mut stdout) = child.stdout.take() else {
        terminate(&mut child);
        return Err(WeatherError("curl output unavailable"));
    };
    let reader = thread::Builder::new()
        .name("shell-weather-output".into())
        .spawn(move || {
            let mut output = Vec::new();
            stdout.read_to_end(&mut output).map(|_| output)
        })
        .map_err(|_| {
            terminate(&mut child);
            WeatherError("Could not start curl output reader")
        })?;
    loop {
        if cancelled.load(Ordering::Acquire) {
            terminate(&mut child);
            let _ = reader.join();
            return Err(WeatherError("Weather request cancelled"));
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => {
                let _ = reader.join();
                return Err(WeatherError("HTTP request failed"));
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => {
                terminate(&mut child);
                let _ = reader.join();
                return Err(WeatherError("Could not wait for curl"));
            }
        }
    }
    reader
        .join()
        .map_err(|_| WeatherError("Could not read curl output"))?
        .map_err(|_| WeatherError("Could not read curl output"))
}

fn config_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn percent_encode(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn wait_or_cancel(duration: Duration, cancelled: &AtomicBool) -> bool {
    let end = std::time::Instant::now() + duration;
    while !cancelled.load(Ordering::Acquire) {
        let remaining = end.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return true;
        }
        thread::sleep(remaining.min(Duration::from_millis(100)));
    }
    false
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

pub struct WeatherGuard {
    cancelled: Arc<AtomicBool>,
    bridge: Option<ThreadPublisherGuard>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Drop for WeatherGuard {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.bridge.take();
        self.worker.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_weather_codes_and_encodes_query_secrets() {
        assert_eq!(weather_icon("13n"), "yh-weather-snow-symbolic");
        assert_eq!(weather_icon("unexpected"), UNKNOWN_ICON);
        assert_eq!(percent_encode("a+b&c"), "a%2Bb%26c");
        assert_eq!(config_escape("a\"b"), "a\\\"b");
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_coordinates() {
        assert!(valid_coordinates(0., 0.));
        assert!(valid_coordinates(-90., 180.));
        assert!(!valid_coordinates(f64::NAN, 0.));
        assert!(!valid_coordinates(90.1, 0.));
        assert!(!valid_coordinates(0., -180.1));
    }

    #[test]
    fn respects_cancellation_before_starting_http_request() {
        let cancelled = AtomicBool::new(true);
        assert!(curl("https://example.invalid", &cancelled).is_err());
    }
}
