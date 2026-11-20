//! Cross-platform application directory lookup.
//!
//! Two modes:
//!
//! - **Default** (no features): confex derives XDG / `%APPDATA%` / `~/Library`
//!   paths itself. Desktop only (Linux, macOS, Windows).
//! - **`istmo` feature**: delegates to `istmo_core::path`, which also covers
//!   Android (`Context.filesDir`) and iOS (`NSFileManager`). The per-app
//!   subdirectory is derived from the build-time `ISTMO_APP_BUNDLE_ID` env
//!   var (baked by `istmo-build::emit_app`), so the `app_name` argument of
//!   [`app_config_dir`] / [`app_data_dir`] is **ignored** in this mode —
//!   istmo already resolved the app identity.

use std::path::PathBuf;

/// Returns the base directory for user configuration files.
///
/// - **With `istmo` feature**: [`istmo_core::path::config_dir`] (already
///   includes the app's bundle id subdir).
/// - Linux/BSD: `$XDG_CONFIG_HOME` or `~/.config`
/// - macOS: `~/Library/Application Support`
/// - Windows: `%APPDATA%`
#[cfg(feature = "istmo")]
#[must_use]
pub fn config_base_dir() -> Option<PathBuf> {
    Some(istmo_core::path::config_dir().to_path_buf())
}

#[cfg(all(not(feature = "istmo"), target_os = "windows"))]
#[must_use]
pub fn config_base_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(all(not(feature = "istmo"), target_os = "macos"))]
#[must_use]
pub fn config_base_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join("Library").join("Application Support"))
}

#[cfg(all(
    not(feature = "istmo"),
    not(any(target_os = "windows", target_os = "macos"))
))]
#[must_use]
pub fn config_base_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".config")))
}

/// Returns the base directory for application data files.
///
/// - **With `istmo` feature**: [`istmo_core::path::data_dir`] (already
///   includes the app's bundle id subdir).
/// - Linux/BSD: `$XDG_DATA_HOME` or `~/.local/share`
/// - macOS: `~/Library/Application Support`
/// - Windows: `%LOCALAPPDATA%`
#[cfg(feature = "istmo")]
#[must_use]
pub fn data_base_dir() -> Option<PathBuf> {
    Some(istmo_core::path::data_dir().to_path_buf())
}

#[cfg(all(not(feature = "istmo"), target_os = "windows"))]
#[must_use]
pub fn data_base_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

#[cfg(all(not(feature = "istmo"), target_os = "macos"))]
#[must_use]
pub fn data_base_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join("Library").join("Application Support"))
}

#[cfg(all(
    not(feature = "istmo"),
    not(any(target_os = "windows", target_os = "macos"))
))]
#[must_use]
pub fn data_base_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".local").join("share")))
}

/// Returns the config directory for a specific application.
///
/// With the `istmo` feature on, `_app_name` is ignored — istmo resolved the
/// app identity at build time via `ISTMO_APP_BUNDLE_ID`.
#[cfg(feature = "istmo")]
#[must_use]
pub fn app_config_dir(_app_name: &str) -> Option<PathBuf> {
    Some(istmo_core::path::config_dir().to_path_buf())
}

#[cfg(not(feature = "istmo"))]
#[must_use]
pub fn app_config_dir(app_name: &str) -> Option<PathBuf> {
    config_base_dir().map(|d| d.join(app_name))
}

/// Returns the data directory for a specific application.
///
/// With the `istmo` feature on, `_app_name` is ignored — istmo resolved the
/// app identity at build time via `ISTMO_APP_BUNDLE_ID`.
#[cfg(feature = "istmo")]
#[must_use]
pub fn app_data_dir(_app_name: &str) -> Option<PathBuf> {
    Some(istmo_core::path::data_dir().to_path_buf())
}

#[cfg(not(feature = "istmo"))]
#[must_use]
pub fn app_data_dir(app_name: &str) -> Option<PathBuf> {
    data_base_dir().map(|d| d.join(app_name))
}

/// Returns the OS-managed cache directory for a specific application.
///
/// Only available with the `istmo` feature (desktop + mobile unified).
#[cfg(feature = "istmo")]
#[must_use]
pub fn app_cache_dir(_app_name: &str) -> Option<PathBuf> {
    Some(istmo_core::path::cache_dir().to_path_buf())
}

/// Returns the state directory for a specific application.
///
/// Only available with the `istmo` feature (desktop + mobile unified).
#[cfg(feature = "istmo")]
#[must_use]
pub fn app_state_dir(_app_name: &str) -> Option<PathBuf> {
    Some(istmo_core::path::state_dir().to_path_buf())
}

#[cfg(not(feature = "istmo"))]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
