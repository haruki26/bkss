pub struct IconIndex {
    paths: std::collections::HashMap<String, std::path::PathBuf>,
    app_icons: std::collections::HashMap<String, Option<String>>,
}

impl IconIndex {
    pub fn load() -> Self {
        let mut paths = std::collections::HashMap::new();
        let mut roots = vec![
            std::path::PathBuf::from("/usr/share/pixmaps"),
            std::path::PathBuf::from("/usr/share/icons"),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(std::path::PathBuf::from(&home).join(".local/share/icons"));
            roots.push(std::path::PathBuf::from(home).join(".icons"));
        }
        for root in roots {
            index_icons(&root, 0, &mut paths);
        }
        let mut app_icons = std::collections::HashMap::new();
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| std::path::PathBuf::from(home).join(".local/share"))
            });
        let data_dirs = std::env::var_os("XDG_DATA_DIRS")
            .filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        for root in data_home
            .into_iter()
            .chain(std::env::split_paths(&data_dirs).filter(|path| path.is_absolute()))
        {
            index_desktop_entries(&root.join("applications"), "", &mut app_icons);
        }
        Self { paths, app_icons }
    }

    pub fn path(&self, name: &str) -> Option<std::path::PathBuf> {
        let path = std::path::Path::new(name);
        if path.is_absolute() {
            Some(path.to_path_buf())
        } else {
            self.paths.get(name).cloned()
        }
    }

    pub fn app_icon_name(&self, class: &str) -> &str {
        self.app_icons
            .get(class)
            .or_else(|| self.app_icons.get(&class.to_lowercase()))
            .and_then(|icon| icon.as_deref())
            .unwrap_or("application-x-executable")
    }
}

fn index_desktop_entries(
    root: &std::path::Path,
    prefix: &str,
    icons: &mut std::collections::HashMap<String, Option<String>>,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            index_desktop_entries(&path, &format!("{prefix}{name}-"), icons);
        } else if let Some(id) = name.strip_suffix(".desktop") {
            // A higher-priority entry without an icon still masks the same system ID.
            icons
                .entry(format!("{prefix}{id}"))
                .or_insert_with(|| read_desktop_entry_icon(&path));
        }
    }
}

const MAX_DESKTOP_ENTRY_BYTES: u64 = 1024 * 1024;

fn read_desktop_entry_icon(path: &std::path::Path) -> Option<String> {
    use std::io::Read;

    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_DESKTOP_ENTRY_BYTES {
        return None;
    }
    let mut contents = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_DESKTOP_ENTRY_BYTES + 1)
        .read_to_end(&mut contents)
        .ok()?;
    if contents.len() as u64 > MAX_DESKTOP_ENTRY_BYTES {
        return None;
    }
    desktop_entry_icon(std::str::from_utf8(&contents).ok()?)
}

fn desktop_entry_icon(contents: &str) -> Option<String> {
    let mut in_entry = false;
    let mut icon = None;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some((key, value)) = line.split_once('=') {
            match (key.trim(), value.trim()) {
                ("Hidden", "true") => return None,
                ("Icon", value) if !value.is_empty() => icon = Some(value.to_owned()),
                _ => {}
            }
        }
    }
    icon
}

fn index_icons(
    root: &std::path::Path,
    depth: usize,
    paths: &mut std::collections::HashMap<String, std::path::PathBuf>,
) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            index_icons(&path, depth + 1, paths);
        } else if path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| matches!(s, "svg" | "png"))
            && let Some(name) = path.file_stem().and_then(|s| s.to_str())
        {
            paths.insert(name.to_owned(), path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_classes_use_desktop_entry_icons_with_ags_case_rules_and_xdg_precedence() {
        let root = std::env::temp_dir().join(format!(
            "shell-app-icons-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let user = root.join("user/applications");
        let system = root.join("system/applications");
        std::fs::create_dir_all(user.join("vendor")).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        for (path, contents) in [
            (
                user.join("foo.desktop"),
                "[Desktop Entry]\nType=Application\nIcon=foo-icon\n[Desktop Action New]\nIcon=wrong-action-icon\n",
            ),
            (
                user.join("ExactCase.desktop"),
                "[Desktop Entry]\nType=Application\nIcon=exact-icon\n",
            ),
            (
                user.join("hidden.desktop"),
                "[Desktop Entry]\nHidden=true\nIcon=hidden-icon\n",
            ),
            (
                user.join("no-icon.desktop"),
                "[Desktop Entry]\nType=Application\n",
            ),
            (
                user.join("bad-utf8.desktop"),
                "[Desktop Entry]\nIcon=bad-icon\n",
            ),
            (
                user.join("vendor/nested.desktop"),
                "[Desktop Entry]\nIcon=/absolute/app.svg\n",
            ),
            (
                system.join("foo.desktop"),
                "[Desktop Entry]\nIcon=wrong-system-icon\n",
            ),
            (
                system.join("hidden.desktop"),
                "[Desktop Entry]\nIcon=wrong-system-icon\n",
            ),
            (
                system.join("no-icon.desktop"),
                "[Desktop Entry]\nIcon=wrong-system-icon\n",
            ),
            (
                system.join("bad-utf8.desktop"),
                "[Desktop Entry]\nIcon=wrong-system-icon\n",
            ),
        ] {
            std::fs::write(path, contents).unwrap();
        }
        std::fs::write(user.join("bad-utf8.desktop"), b"\xff").unwrap();
        std::fs::write(
            user.join("oversized.desktop"),
            vec![b'x'; MAX_DESKTOP_ENTRY_BYTES as usize + 1],
        )
        .unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(user.join("foo.desktop"), user.join("linked.desktop"))
                .unwrap();
            std::os::unix::fs::symlink("/dev/zero", user.join("special.desktop")).unwrap();
            std::fs::write(
                system.join("special.desktop"),
                "[Desktop Entry]\nIcon=wrong-system-icon\n",
            )
            .unwrap();
        }
        let mut index = IconIndex {
            paths: [
                ("foo-icon".into(), root.join("foo-icon.svg")),
                ("unmatched".into(), root.join("unmatched.svg")),
                (
                    "application-x-executable".into(),
                    root.join("executable.svg"),
                ),
            ]
            .into(),
            app_icons: Default::default(),
        };
        for applications in [&user, &system] {
            index_desktop_entries(applications, "", &mut index.app_icons);
        }
        for class in ["foo", "Foo", "FOO"] {
            assert_eq!(index.app_icon_name(class), "foo-icon");
            assert_eq!(
                index.path(index.app_icon_name(class)),
                Some(root.join("foo-icon.svg"))
            );
        }
        assert_eq!(index.app_icon_name("ExactCase"), "exact-icon");
        assert_eq!(index.app_icon_name("vendor-nested"), "/absolute/app.svg");
        for class in [
            "exactcase",
            "unmatched",
            "hidden",
            "no-icon",
            "bad-utf8",
            "oversized",
        ] {
            assert_eq!(index.app_icon_name(class), "application-x-executable");
            assert_eq!(
                index.path(index.app_icon_name(class)),
                Some(root.join("executable.svg"))
            );
        }
        #[cfg(unix)]
        {
            assert_eq!(index.app_icon_name("linked"), "foo-icon");
            assert_eq!(index.app_icon_name("special"), "application-x-executable");
        }
        // Notification and tray icon names must not be treated as client classes.
        assert_eq!(index.path("unmatched"), Some(root.join("unmatched.svg")));
        std::fs::remove_dir_all(root).unwrap();
    }
}
