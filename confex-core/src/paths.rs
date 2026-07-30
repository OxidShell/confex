use std::path::PathBuf;

/// Returns the base directory for user configuration files.
///
/// - Linux/BSD: `$XDG_CONFIG_HOME` or `~/.config`
/// - macOS: `~/Library/Application Support`
/// - Windows: `%APPDATA%`
#[must_use]
pub fn config_base_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|h| h.join("Library").join("Application Support"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".config")))
    }
}

/// Returns the base directory for application data files.
///
/// - Linux/BSD: `$XDG_DATA_HOME` or `~/.local/share`
/// - macOS: `~/Library/Application Support`
/// - Windows: `%LOCALAPPDATA%`
#[must_use]
pub fn data_base_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    }
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|h| h.join("Library").join("Application Support"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".local").join("share")))
    }
}

/// Returns the config directory for a specific application.
#[must_use]
pub fn app_config_dir(app_name: &str) -> Option<PathBuf> {
    config_base_dir().map(|d| d.join(app_name))
}

/// Returns the data directory for a specific application.
#[must_use]
pub fn app_data_dir(app_name: &str) -> Option<PathBuf> {
    data_base_dir().map(|d| d.join(app_name))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
