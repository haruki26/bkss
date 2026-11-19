use std::{
    env, fs,
    path::{Path, PathBuf},
};

use bkss_integration::Secret;
use serde::Deserialize;

#[derive(Deserialize)]
struct FileSettings {
    weather: WeatherSettings,
}

#[derive(Deserialize)]
struct WeatherSettings {
    openweathermap_api_key: Option<String>,
}

pub struct Settings {
    pub weather_api_key: Option<Secret<String>>,
    pub error: Option<String>,
}

impl Settings {
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self {
                weather_api_key: None,
                error: None,
            };
        };
        match read_settings(&path) {
            Ok(settings) => settings,
            Err(error) => Self {
                weather_api_key: None,
                error: Some(error),
            },
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    settings_path_from(env::var_os("XDG_CONFIG_HOME"), env::var_os("HOME"))
}

fn settings_path_from(
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let absolute = |path: std::ffi::OsString| {
        let path = PathBuf::from(path);
        (!path.as_os_str().is_empty() && path.is_absolute()).then_some(path)
    };
    xdg_config_home
        .and_then(absolute)
        .or_else(|| home.and_then(absolute).map(|home| home.join(".config")))
        .map(|config_home| config_home.join("bkss/settings.json"))
}

fn read_settings(path: &Path) -> Result<Settings, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings {
                weather_api_key: None,
                error: None,
            });
        }
        Err(_) => return Err("Could not read shell settings".into()),
    };
    let parsed: FileSettings =
        serde_json::from_str(&text).map_err(|_| "Invalid shell settings".to_owned())?;
    if parsed
        .weather
        .openweathermap_api_key
        .as_deref()
        .is_some_and(|key| key.trim().is_empty())
    {
        return Err("Invalid shell settings".into());
    }
    Ok(Settings {
        weather_api_key: parsed.weather.openweathermap_api_key.map(Secret::new),
        error: None,
    })
}

pub fn weather_api_key(saved: Option<&Secret<String>>) -> Option<Secret<String>> {
    weather_api_key_from(saved, env::var("OPENWEATHERMAP_API_KEY").ok().as_deref())
}

fn weather_api_key_from(
    saved: Option<&Secret<String>>,
    environment: Option<&str>,
) -> Option<Secret<String>> {
    saved
        .map(|key| Secret::new(key.expose().clone()))
        .or_else(|| {
            environment
                .filter(|key| !key.trim().is_empty())
                .map(|key| Secret::new(key.to_owned()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_is_redacted() {
        let path = test_path("valid");
        fs::write(
            &path,
            r#"{"weather":{"openweathermap_api_key":"test-value"}}"#,
        )
        .unwrap();
        let settings = read_settings(&path).expect("read settings");
        let key = settings.weather_api_key.expect("key exists");
        assert!(!format!("{key:?}").contains("test-value"));
        fs::remove_file(path).unwrap();
    }

    fn test_path(name: &str) -> PathBuf {
        env::temp_dir().join(format!("shell-settings-{}-{name}.json", std::process::id()))
    }

    #[test]
    fn uses_only_nonempty_absolute_config_roots() {
        assert_eq!(
            settings_path_from(Some("/custom/config".into()), Some("/home/test".into())),
            Some(PathBuf::from("/custom/config/bkss/settings.json"))
        );
        assert_eq!(
            settings_path_from(None, Some("/home/test".into())),
            Some(PathBuf::from("/home/test/.config/bkss/settings.json"))
        );
        for invalid_xdg in ["", "relative/config"] {
            assert_eq!(
                settings_path_from(Some(invalid_xdg.into()), Some("/home/test".into())),
                Some(PathBuf::from("/home/test/.config/bkss/settings.json"))
            );
        }
        assert_eq!(
            settings_path_from(Some("relative/config".into()), Some("relative/home".into())),
            None
        );
        assert_eq!(settings_path_from(None, Some("".into())), None);
        assert_eq!(settings_path_from(None, None), None);
    }

    #[test]
    fn missing_settings_file_is_successful() {
        let settings = read_settings(&test_path("missing")).unwrap();
        assert!(settings.error.is_none());
        assert!(settings.weather_api_key.is_none());
    }

    #[test]
    fn malformed_json_and_missing_weather_key_are_sanitized_errors() {
        for (name, text) in [
            ("invalid", "{"),
            ("missing-key", "{}"),
            ("blank-key", r#"{"weather":{"openweathermap_api_key":" "}}"#),
        ] {
            let path = test_path(name);
            fs::write(&path, text).unwrap();
            assert!(matches!(
                read_settings(&path),
                Err(error) if error == "Invalid shell settings"
            ));
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn unreadable_settings_are_reported_without_file_contents() {
        let path = env::temp_dir().join(format!("shell-settings-{}-directory", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        assert!(matches!(
            read_settings(&path),
            Err(error) if error == "Could not read shell settings"
        ));
        fs::remove_dir(path).unwrap();
    }

    #[test]
    fn environment_fallback_is_used_only_without_saved_key() {
        let environment_key = Secret::new("environment-value".to_owned());
        assert_eq!(
            weather_api_key_from(None, Some("environment-value"))
                .map(|secret| secret.expose().clone()),
            Some(environment_key.expose().clone())
        );
        assert!(weather_api_key_from(None, Some("  ")).is_none());
        let saved = Secret::new("saved-value".to_owned());
        assert_eq!(
            weather_api_key_from(Some(&saved), Some("environment-value"))
                .map(|secret| secret.expose().clone()),
            Some(saved.expose().clone())
        );
    }
}
