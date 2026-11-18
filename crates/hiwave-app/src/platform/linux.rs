// SPDX-License-Identifier: MPL-2.0
// Contains code contributed under MPL-2.0 by an outside contributor; see NOTICE.
//! Linux platform implementation
//!
//! GTK WebKit on Linux has moderate popup behavior. The xdg-open
//! command is used for external operations, but it doesn't support
//! file selection in folder (only opens the parent directory).

use super::{
    is_definitely_popup, is_legitimate_navigation, PlatformCapabilities, PlatformError,
    PlatformManager, PlatformResult, WebViewEngine,
};
use muda::{Menu, PredefinedMenuItem, Submenu};
use std::path::{Path, PathBuf};
use std::process::Command;
use tao::window::Window;
use tracing::{debug, warn};

/// Linux platform manager
pub struct LinuxPlatform {
    capabilities: PlatformCapabilities,
}

impl LinuxPlatform {
    pub fn new() -> Self {
        let default_download_dir = dirs::download_dir().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Downloads")
        });

        Self {
            capabilities: PlatformCapabilities {
                native_menu_required_for_clipboard: false,
                // xdg-open doesn't support file selection, only opens parent dir
                supports_file_selection_in_folder: false,
                webview_engine: WebViewEngine::WebKit,
                default_download_dir,
                platform_name: "Linux",
            },
        }
    }

    /// Create the Edit menu with clipboard operations
    fn create_edit_menu(&self, menu: &Menu) -> PlatformResult<()> {
        let edit_menu = Submenu::new("Edit", true);

        edit_menu
            .append(&PredefinedMenuItem::undo(None))
            .map_err(|e| PlatformError::MenuInitFailed(format!("Failed to add Undo: {}", e)))?;

        edit_menu
            .append(&PredefinedMenuItem::redo(None))
            .map_err(|e| PlatformError::MenuInitFailed(format!("Failed to add Redo: {}", e)))?;

        edit_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(|e| {
                PlatformError::MenuInitFailed(format!("Failed to add separator: {}", e))
            })?;

        edit_menu
            .append(&PredefinedMenuItem::cut(None))
            .map_err(|e| PlatformError::MenuInitFailed(format!("Failed to add Cut: {}", e)))?;

        edit_menu
            .append(&PredefinedMenuItem::copy(None))
            .map_err(|e| PlatformError::MenuInitFailed(format!("Failed to add Copy: {}", e)))?;

        edit_menu
            .append(&PredefinedMenuItem::paste(None))
            .map_err(|e| PlatformError::MenuInitFailed(format!("Failed to add Paste: {}", e)))?;

        edit_menu
            .append(&PredefinedMenuItem::select_all(None))
            .map_err(|e| {
                PlatformError::MenuInitFailed(format!("Failed to add Select All: {}", e))
            })?;

        menu.append(&edit_menu).map_err(|e| {
            PlatformError::MenuInitFailed(format!("Failed to append edit menu: {}", e))
        })?;

        Ok(())
    }
}

impl Default for LinuxPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformManager for LinuxPlatform {
    fn initialize_menu(&self, window: &Window, menu: &Menu) -> PlatformResult<()> {
        // Create Edit menu
        self.create_edit_menu(menu)?;

        // Initialize menu for GTK window
        #[cfg(target_os = "linux")]
        {
            use tao::platform::unix::WindowExtUnix;

            // GTK window and display may not be available in headless environments
            match (window.gtk_window(), window.default_vbox()) {
                (gtk_window, Some(vbox)) => {
                    // Initialize menu for GTK window using the vbox container when available
                    menu.init_for_gtk_window(gtk_window, Some(vbox))
                        .map_err(|e| {
                            PlatformError::MenuInitFailed(format!("GTK init failed: {}", e))
                        })?;
                }
                _ => {
                    warn!("GTK window not available for menu initialization");
                    // Don't fail - menu is optional on Linux
                }
            }
        }

        debug!("Linux menu initialized successfully");
        Ok(())
    }

    fn open_external(&self, url: &str) -> PlatformResult<()> {
        debug!("Opening external URL: {}", url);

        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| PlatformError::OpenExternalFailed(format!("{}: {}", url, e)))?;

        Ok(())
    }

    fn open_file(&self, path: &Path) -> PlatformResult<()> {
        if !path.exists() {
            return Err(PlatformError::FileNotFound(path.to_path_buf()));
        }

        debug!("Opening file: {:?}", path);

        Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| PlatformError::OpenFileFailed(format!("{:?}: {}", path, e)))?;

        Ok(())
    }

    fn show_in_folder(&self, path: &Path) -> PlatformResult<()> {
        if !path.exists() {
            return Err(PlatformError::FileNotFound(path.to_path_buf()));
        }

        debug!("Showing in file manager: {:?}", path);

        // xdg-open doesn't support file selection, so we open the parent directory
        let parent = path.parent().unwrap_or(path);

        Command::new("xdg-open")
            .arg(parent)
            .spawn()
            .map_err(|e| PlatformError::ShowInFolderFailed(format!("{:?}: {}", path, e)))?;

        Ok(())
    }

    fn capabilities(&self) -> &PlatformCapabilities {
        &self.capabilities
    }

    fn should_open_as_tab(&self, url: &str, _initiating_url: Option<&str>) -> bool {
        // Linux GTK WebKit has moderate behavior - use medium filtering

        // Quick rejection for obvious popups
        if is_definitely_popup(url) {
            debug!("Blocking popup (definitely): {}", url);
            return false;
        }

        // Block about: and data: URLs
        if url == "about:blank" || url == "about:srcdoc" || url.starts_with("data:") {
            debug!("Blocking about/data URL: {}", url);
            return false;
        }

        // Block javascript: URLs
        if url.starts_with("javascript:") {
            debug!("Blocking javascript URL: {}", url);
            return false;
        }

        // Medium filtering - more than Windows, less than macOS
        let blocked_patterns = [
            "googlesyndication.com",
            "doubleclick.net",
            "googleadservices.com",
            "facebook.com/tr",
            "connect.facebook.net",
            "/beacon",
            "/pixel",
        ];

        let url_lower = url.to_lowercase();
        for pattern in blocked_patterns {
            if url_lower.contains(pattern) {
                debug!("Blocking URL matching pattern '{}': {}", pattern, url);
                return false;
            }
        }

        // Allow if it looks legitimate
        if is_legitimate_navigation(url) {
            return true;
        }

        // Default: allow
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linux_capabilities() {
        let platform = LinuxPlatform::new();
        let caps = platform.capabilities();

        assert!(!caps.native_menu_required_for_clipboard);
        assert!(!caps.supports_file_selection_in_folder); // xdg-open limitation
        assert_eq!(caps.webview_engine, WebViewEngine::WebKit);
        assert_eq!(caps.platform_name, "Linux");
    }

    #[test]
    fn test_should_open_as_tab_blocks_popups() {
        let platform = LinuxPlatform::new();

        // Should block
        assert!(!platform.should_open_as_tab("about:blank", None));
        assert!(!platform.should_open_as_tab("javascript:void(0)", None));
        assert!(!platform.should_open_as_tab("https://doubleclick.net/ad", None));
    }

    #[test]
    fn test_should_open_as_tab_allows_legitimate() {
        let platform = LinuxPlatform::new();

        // Should allow
        assert!(platform.should_open_as_tab("https://github.com/pureflow", None));
        assert!(platform.should_open_as_tab("https://example.com/page", None));
    }

    #[test]
    fn test_webkit_is_aggressive() {
        let platform = LinuxPlatform::new();
        let caps = platform.capabilities();

        // Linux also uses WebKit, which is aggressive
        assert!(caps.webview_engine.is_aggressive_popup_opener());
    }
}
