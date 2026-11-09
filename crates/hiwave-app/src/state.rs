//! Application state management
//!
//! This module contains the central state for the HiWave application,
//! including the browser shell, ad blocker, password vault, and configuration.
//!
//! Note: Many methods are currently unused in native-win32 mode but will be
//! wired up as native mode gains feature parity with hybrid mode.

#![allow(dead_code)]

use crate::import::ImportResult;
use hiwave_analytics::Analytics;
use hiwave_core::types::{TabId, WorkspaceId};
use hiwave_core::HiWaveResult;
use hiwave_shell::{BrowserShell, ShellSnapshot};
use hiwave_shield::AdBlocker;
use hiwave_vault::Vault;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use url::Url;

/// Cross-platform path wrapper that serializes with forward slashes
///
/// This type ensures paths are stored in a portable format (forward slashes)
/// regardless of the operating system. When deserialized, paths are converted
/// back to the native format.
#[derive(Debug, Clone)]
pub struct PortablePath(PathBuf);

#[allow(dead_code)] // Methods used in tests and future phases
impl PortablePath {
    /// Create a new PortablePath from a PathBuf
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// Create a new PortablePath from a string
    pub fn from_str(s: &str) -> Self {
        Self(PathBuf::from(s))
    }

    /// Get a reference to the underlying Path
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Get the underlying PathBuf
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }

    /// Check if the path exists
    pub fn exists(&self) -> bool {
        self.0.exists()
    }

    /// Get the file name component
    pub fn file_name(&self) -> Option<&std::ffi::OsStr> {
        self.0.file_name()
    }

    /// Convert to a string representation (may fail for non-UTF8 paths)
    pub fn to_string_lossy(&self) -> std::borrow::Cow<'_, str> {
        self.0.to_string_lossy()
    }
}

impl From<PathBuf> for PortablePath {
    fn from(path: PathBuf) -> Self {
        Self(path)
    }
}

impl From<&Path> for PortablePath {
    fn from(path: &Path) -> Self {
        Self(path.to_path_buf())
    }
}

impl From<String> for PortablePath {
    fn from(s: String) -> Self {
        Self(PathBuf::from(s))
    }
}

impl AsRef<Path> for PortablePath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Serialize for PortablePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Always serialize with forward slashes for cross-platform compatibility
        let normalized = self
            .0
            .to_str()
            .ok_or_else(|| serde::ser::Error::custom("Path contains invalid UTF-8"))?
            .replace('\\', "/");
        serializer.serialize_str(&normalized)
    }
}

impl<'de> Deserialize<'de> for PortablePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        // PathBuf::from handles platform-specific separator conversion
        Ok(PortablePath(PathBuf::from(s)))
    }
}

/// User-defined blocklist for popup domains
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserBlocklist {
    /// Set of blocked domains
    pub domains: HashSet<String>,
}

impl UserBlocklist {
    /// Load blocklist from file
    pub fn load(path: &PathBuf) -> Self {
        if !path.exists() {
            return Self::default();
        }

        match std::fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save blocklist to file
    pub fn save(&self, path: &PathBuf) -> HiWaveResult<()> {
        let data = serde_json::to_string_pretty(self).map_err(|e| {
            hiwave_core::HiWaveError::Config(format!("Failed to serialize blocklist: {}", e))
        })?;
        std::fs::write(path, data)?;
        Ok(())
    }

    /// Add a domain to the blocklist
    pub fn add_domain(&mut self, domain: String) -> bool {
        self.domains.insert(domain)
    }

    /// Remove a domain from the blocklist
    pub fn remove_domain(&mut self, domain: &str) -> bool {
        self.domains.remove(domain)
    }

    /// Check if a URL should be blocked based on its domain
    pub fn should_block(&self, url: &str) -> bool {
        if let Ok(parsed) = Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                // Check exact domain match
                if self.domains.contains(host) {
                    return true;
                }
                // Check if any blocked domain is a suffix (e.g., block example.com also blocks sub.example.com)
                for blocked in &self.domains {
                    if host.ends_with(blocked)
                        && (host == blocked.as_str() || host.ends_with(&format!(".{}", blocked)))
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Extract domain from a URL
    pub fn extract_domain(url: &str) -> Option<String> {
        Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
    }
}

/// Focus mode configuration (persisted to settings)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusModeConfig {
    /// Whether auto-focus is enabled
    pub auto_enabled: bool,

    /// Seconds before auto-entering focus mode (default: 60)
    pub activation_delay_secs: u32,

    /// Show progress bar at top of screen
    pub show_progress_bar: bool,

    /// Hide cursor after inactivity
    pub hide_cursor: bool,

    /// Seconds before cursor hides (default: 3)
    pub cursor_hide_delay_secs: u32,

    /// Exit focus mode after inactivity (0 = disabled)
    pub inactivity_exit_mins: u32,

    /// Keyboard shortcut identifier (default: "F11")
    pub shortcut: String,

    /// Domains that never auto-focus
    pub blocklist: Vec<String>,
}

impl Default for FocusModeConfig {
    fn default() -> Self {
        Self {
            auto_enabled: true,
            activation_delay_secs: 60,
            show_progress_bar: true,
            hide_cursor: true,
            cursor_hide_delay_secs: 3,
            inactivity_exit_mins: 0,
            shortcut: "F11".to_string(),
            blocklist: vec![
                "youtube.com".to_string(),
                "netflix.com".to_string(),
                "twitch.tv".to_string(),
                "vimeo.com".to_string(),
                "dailymotion.com".to_string(),
                "hulu.com".to_string(),
                "disneyplus.com".to_string(),
                "primevideo.com".to_string(),
            ],
        }
    }
}

impl FocusModeConfig {
    /// Check if a URL's domain is in the blocklist
    pub fn is_blocklisted(&self, url: &str) -> bool {
        if let Ok(parsed) = Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                for blocked in &self.blocklist {
                    if host == blocked || host.ends_with(&format!(".{}", blocked)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Add a domain to the blocklist
    pub fn add_to_blocklist(&mut self, domain: String) -> bool {
        let domain = domain.trim().to_lowercase();
        if domain.is_empty() || self.blocklist.contains(&domain) {
            return false;
        }
        self.blocklist.push(domain);
        true
    }

    /// Remove a domain from the blocklist
    pub fn remove_from_blocklist(&mut self, domain: &str) -> bool {
        let domain = domain.trim().to_lowercase();
        if let Some(idx) = self.blocklist.iter().position(|d| d == &domain) {
            self.blocklist.remove(idx);
            return true;
        }
        false
    }
}

/// Focus mode runtime state (not persisted)
pub struct FocusModeState {
    /// Currently in focus mode
    pub active: bool,

    /// When focus mode was entered (for session duration tracking)
    pub entered_at: Option<Instant>,

    /// When current page was loaded (for auto-trigger timing)
    pub page_loaded_at: Option<Instant>,

    /// Last significant scroll timestamp (resets auto-trigger timer)
    pub last_scroll_at: Option<Instant>,

    /// Last user interaction timestamp (for inactivity exit)
    pub last_interaction_at: Option<Instant>,

    /// Whether peek UI is currently visible (mouse at top)
    pub peek_visible: bool,

    /// Current scroll progress (0.0 - 1.0)
    pub scroll_progress: f32,

    /// Whether media is currently playing in the content
    pub media_playing: bool,

    /// Current page URL (for blocklist checking)
    pub current_url: Option<String>,

    /// Whether auto-trigger has already fired for this page load
    /// (prevents re-triggering after user exits)
    pub auto_triggered_for_page: bool,
}

impl FocusModeState {
    pub fn new() -> Self {
        Self {
            active: false,
            entered_at: None,
            page_loaded_at: None,
            last_scroll_at: None,
            last_interaction_at: None,
            peek_visible: false,
            scroll_progress: 0.0,
            media_playing: false,
            current_url: None,
            auto_triggered_for_page: false,
        }
    }

    /// Enter focus mode
    pub fn enter(&mut self) {
        if self.active {
            return;
        }
        self.active = true;
        self.entered_at = Some(Instant::now());
        self.last_interaction_at = Some(Instant::now());
        self.peek_visible = false;
        tracing::info!("Entered focus mode");
    }

    /// Exit focus mode
    pub fn exit(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.entered_at = None;
        self.peek_visible = false;
        // Prevent auto-trigger from re-activating on same page after user exits
        self.auto_triggered_for_page = true;
        tracing::info!("Exited focus mode");
    }

    /// Toggle focus mode
    pub fn toggle(&mut self) {
        if self.active {
            self.exit();
        } else {
            self.enter();
        }
    }

    /// Show peek UI (mouse at top)
    pub fn show_peek(&mut self) {
        if self.active && !self.peek_visible {
            self.peek_visible = true;
            self.record_interaction();
        }
    }

    /// Hide peek UI
    pub fn hide_peek(&mut self) {
        self.peek_visible = false;
    }

    /// Record user interaction (for inactivity tracking)
    pub fn record_interaction(&mut self) {
        self.last_interaction_at = Some(Instant::now());
    }

    /// Record a significant scroll (resets auto-trigger timer)
    pub fn record_scroll(&mut self) {
        self.last_scroll_at = Some(Instant::now());
        self.record_interaction();
    }

    /// Record page navigation
    pub fn record_navigation(&mut self, url: &str) {
        // Exit focus mode on navigation
        if self.active {
            self.exit();
        }
        self.current_url = Some(url.to_string());
        self.page_loaded_at = Some(Instant::now());
        self.last_scroll_at = None;
        self.scroll_progress = 0.0;
        self.media_playing = false;
        // Reset auto-trigger flag for new page
        self.auto_triggered_for_page = false;
    }

    /// Update scroll progress
    pub fn update_scroll_progress(&mut self, progress: f32) {
        self.scroll_progress = progress.clamp(0.0, 1.0);
    }

    /// Update media playing state
    pub fn set_media_playing(&mut self, playing: bool) {
        self.media_playing = playing;
    }

    /// Check if auto-focus should trigger based on config and current state
    pub fn should_auto_enter(&self, config: &FocusModeConfig) -> bool {
        // Already active
        if self.active {
            return false;
        }

        // Already auto-triggered for this page (prevents re-trigger after user exits)
        if self.auto_triggered_for_page {
            return false;
        }

        // Auto-focus disabled
        if !config.auto_enabled {
            return false;
        }

        // Media is playing
        if self.media_playing {
            return false;
        }

        // Check if URL is blocklisted
        if let Some(ref url) = self.current_url {
            if config.is_blocklisted(url) {
                return false;
            }
        }

        // Check if enough time has passed since page load
        let Some(page_loaded) = self.page_loaded_at else {
            return false;
        };

        let elapsed = page_loaded.elapsed().as_secs() as u32;
        if elapsed < config.activation_delay_secs {
            return false;
        }

        // Check if there was recent scroll activity (user still exploring)
        if let Some(last_scroll) = self.last_scroll_at {
            // If scrolled within last 10 seconds, don't auto-enter
            if last_scroll.elapsed().as_secs() < 10 {
                return false;
            }
        }

        true
    }

    /// Check if should exit due to inactivity
    #[allow(dead_code)]
    pub fn should_inactivity_exit(&self, config: &FocusModeConfig) -> bool {
        if !self.active || config.inactivity_exit_mins == 0 {
            return false;
        }

        if let Some(last_interaction) = self.last_interaction_at {
            let inactivity_secs = last_interaction.elapsed().as_secs();
            let threshold_secs = (config.inactivity_exit_mins as u64) * 60;
            return inactivity_secs >= threshold_secs;
        }

        false
    }

    /// Get duration in focus mode (seconds)
    pub fn focus_duration_secs(&self) -> Option<u64> {
        self.entered_at.map(|t| t.elapsed().as_secs())
    }
}

impl Default for FocusModeState {
    fn default() -> Self {
        Self::new()
    }
}

// Legacy FocusMode alias for compatibility during transition
pub type FocusMode = FocusModeState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadItem {
    pub id: u64,
    pub url: String,
    pub file_name: String,
    pub status: DownloadStatus,
    /// Path to the downloaded file, stored in portable format (forward slashes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PortablePath>,
    pub progress: Option<f32>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub error: Option<String>,
}

#[allow(dead_code)] // Method used in tests and future IPC updates
impl DownloadItem {
    /// Get the path as a string for display or IPC
    pub fn path_string(&self) -> Option<String> {
        self.path.as_ref().map(|p| p.to_string_lossy().into_owned())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisitRecord {
    pub id: u64,
    pub url: String,
    pub title: Option<String>,
    pub workspace: Option<String>,
    pub timestamp: u64,
    pub count: u32,
}

/// Central application state
pub struct AppState {
    /// Browser shell (tabs, workspaces, command palette)
    pub shell: BrowserShell,

    /// Ad blocker
    pub shield: AdBlocker,

    /// Password vault
    pub vault: Vault,

    /// Analytics tracking
    pub analytics: Analytics,

    /// Configuration
    pub config: AppConfig,

    /// User-configurable settings
    pub user_settings: UserSettings,

    /// Focus mode state (runtime, not persisted)
    pub focus_mode: FocusMode,

    /// Focus mode configuration (persisted)
    pub focus_mode_config: FocusModeConfig,

    /// File name (in the data dir) the workspace state is saved to. The native shell uses its
    /// own file so it never reads or rewrites the default build's session.
    pub workspace_state_file: String,

    /// Set when the saved session could not be read and could not be moved aside: saving is
    /// refused for the rest of the run, so the unreadable file is never overwritten.
    workspace_state_save_blocked: bool,

    /// Shelved tabs by workspace name
    pub shelves: HashMap<String, Vec<ShelfItem>>,

    /// Cellar storage for aged-out shelf items (recoverable)
    pub cellar: Vec<CellarItem>,

    /// Download history (both in-progress and completed)
    pub downloads: Vec<DownloadItem>,

    /// Visit history entries
    pub visit_history: Vec<VisitRecord>,

    /// Monotonic download identifier generator
    pub next_download_id: u64,

    /// Monotonic visit identifier generator
    pub next_visit_id: u64,

    /// User-defined blocked domains (for popup blocking)
    pub user_blocklist: UserBlocklist,

    /// Recent popup requests for flood protection (URL, timestamp)
    /// This is runtime-only state, not persisted
    pub recent_popups: Vec<(String, Instant)>,

    /// Zoom levels per tab (tab_id -> zoom_level, default 1.0)
    pub tab_zoom_levels: HashMap<String, f32>,

    /// Audio playing state per tab (tab_id -> is_playing)
    pub tab_audio_playing: HashMap<String, bool>,

    /// Muted state per tab (tab_id -> is_muted)
    pub tab_audio_muted: HashMap<String, bool>,

    /// Session start time (for analytics)
    pub session_start_time: Instant,
}

impl AppState {
    /// Create a new application state
    pub fn new(config: AppConfig) -> HiWaveResult<Self> {
        // Create data directory if it doesn't exist
        if let Some(parent) = config.data_dir.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::create_dir_all(&config.data_dir).ok();
        std::fs::create_dir_all(&config.download_dir).ok();

        // Initialize components
        let shell = BrowserShell::new();
        let shield = AdBlocker::with_filter_lists();
        let focus_mode = FocusMode::new();

        let vault_path = config.data_dir.join("vault.db");
        let vault = Vault::new(&vault_path)?;

        // Initialize analytics
        let analytics_path = config.data_dir.join("analytics.db");
        let analytics = Analytics::new(analytics_path)?;

        // Load user settings
        let settings_path = config.data_dir.join("settings.json");
        let user_settings = UserSettings::load(&settings_path);
        tracing::info!("Loaded user settings (theme: {})", user_settings.theme);

        // Load focus mode config
        let focus_config_path = config.data_dir.join("focus_mode.json");
        let focus_mode_config = Self::load_focus_config(&focus_config_path);

        // Load user blocklist
        let blocklist_path = config.data_dir.join("user-blocklist.json");
        let user_blocklist = UserBlocklist::load(&blocklist_path);
        if !user_blocklist.domains.is_empty() {
            tracing::info!(
                "Loaded {} user-blocked domains",
                user_blocklist.domains.len()
            );
        }

        Ok(Self {
            shell,
            shield,
            vault,
            analytics,
            config,
            user_settings,
            focus_mode,
            focus_mode_config,
            workspace_state_file: "workspace_state.json".to_string(),
            workspace_state_save_blocked: false,
            shelves: HashMap::new(),
            cellar: Vec::new(),
            downloads: Vec::new(),
            visit_history: Vec::new(),
            next_download_id: 1,
            next_visit_id: 1,
            user_blocklist,
            recent_popups: Vec::new(),
            tab_zoom_levels: HashMap::new(),
            tab_audio_playing: HashMap::new(),
            tab_audio_muted: HashMap::new(),
            session_start_time: Instant::now(),
        })
    }

    pub fn current_timestamp_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    }

    /// Get the path to the user blocklist file
    pub fn blocklist_path(&self) -> PathBuf {
        self.config.data_dir.join("user-blocklist.json")
    }

    /// Add a domain to the user blocklist and save
    pub fn block_domain(&mut self, domain: String) -> HiWaveResult<bool> {
        let added = self.user_blocklist.add_domain(domain.clone());
        if added {
            tracing::info!("Added domain to user blocklist: {}", domain);
            self.user_blocklist.save(&self.blocklist_path())?;
        }
        Ok(added)
    }

    /// Remove a domain from the user blocklist and save
    pub fn unblock_domain(&mut self, domain: &str) -> HiWaveResult<bool> {
        let removed = self.user_blocklist.remove_domain(domain);
        if removed {
            tracing::info!("Removed domain from user blocklist: {}", domain);
            self.user_blocklist.save(&self.blocklist_path())?;
        }
        Ok(removed)
    }

    /// Get the path to the settings file
    pub fn settings_path(&self) -> PathBuf {
        self.config.data_dir.join("settings.json")
    }

    /// Get a clone of current user settings
    pub fn get_settings(&self) -> UserSettings {
        self.user_settings.clone()
    }

    /// Save current user settings to disk
    pub fn save_settings(&self) -> HiWaveResult<()> {
        self.user_settings.save(&self.settings_path())
    }

    /// Update user settings and save to disk
    pub fn update_settings(&mut self, settings: UserSettings) -> HiWaveResult<()> {
        self.user_settings = settings;
        self.save_settings()
    }

    /// Get the path to the focus mode config file
    pub fn focus_config_path(&self) -> PathBuf {
        self.config.data_dir.join("focus_mode.json")
    }

    /// Load focus mode config from file
    fn load_focus_config(path: &std::path::Path) -> FocusModeConfig {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(config) = serde_json::from_str(&contents) {
                return config;
            }
        }
        FocusModeConfig::default()
    }

    /// Save focus mode config to disk
    pub fn save_focus_config(&self) -> HiWaveResult<()> {
        let path = self.focus_config_path();
        let json = serde_json::to_string_pretty(&self.focus_mode_config).map_err(|e| {
            hiwave_core::HiWaveError::Config(format!("Failed to serialize focus config: {}", e))
        })?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Update focus mode config and save to disk
    pub fn update_focus_config(&mut self, config: FocusModeConfig) -> HiWaveResult<()> {
        self.focus_mode_config = config;
        self.save_focus_config()
    }

    /// Generate an export of workspaces and optionally settings
    pub fn export_data(&self, include_settings: bool, include_workspaces: bool) -> HiWaveExport {
        let mut export = HiWaveExport::new();

        if include_settings {
            export.settings = Some(self.user_settings.clone());
        }

        if include_workspaces {
            // Export workspaces with their tabs
            for workspace in self.shell.list_workspaces() {
                let tabs: Vec<TabExport> = self
                    .shell
                    .list_tabs(Some(workspace.id))
                    .iter()
                    .map(|tab| TabExport {
                        url: tab.url.to_string(),
                        title: tab.title.clone(),
                        locked: tab.locked,
                    })
                    .collect();

                export.workspaces.push(WorkspaceExport {
                    name: workspace.name.clone(),
                    tabs,
                });
            }

            // Export shelf items
            export.shelf = self.shelves.clone();
        }

        export
    }

    /// Import data from an export, merging or replacing workspaces
    pub fn import_data(
        &mut self,
        data: &HiWaveExport,
        replace: bool,
    ) -> HiWaveResult<ImportResult> {
        let mut workspaces_created = 0;
        let mut tabs_created = 0;
        let mut errors = Vec::new();

        // Import settings if present
        if let Some(ref settings) = data.settings {
            self.user_settings = settings.clone();
            if let Err(e) = self.save_settings() {
                errors.push(format!("Failed to save settings: {}", e));
            }
        }

        // Import workspaces
        for ws_export in &data.workspaces {
            // Check if workspace already exists
            let existing = self
                .shell
                .list_workspaces()
                .iter()
                .find(|ws| ws.name == ws_export.name)
                .map(|ws| ws.id);

            let workspace_id = if let Some(id) = existing {
                if replace {
                    // Close all existing tabs in this workspace
                    let tabs: Vec<_> = self
                        .shell
                        .list_tabs(Some(id))
                        .iter()
                        .map(|t| t.id)
                        .collect();
                    for tab_id in tabs {
                        let _ = self.shell.close_tab(tab_id);
                    }
                }
                id
            } else {
                // Create new workspace
                let ws_id = self.shell.create_workspace(ws_export.name.clone());
                workspaces_created += 1;
                ws_id
            };

            // Import tabs
            for tab_export in &ws_export.tabs {
                let url = match url::Url::parse(&tab_export.url) {
                    Ok(u) => u,
                    Err(e) => {
                        errors.push(format!("Invalid URL '{}': {}", tab_export.url, e));
                        continue;
                    }
                };

                let tab = hiwave_core::types::TabInfo {
                    id: hiwave_core::types::TabId::new(),
                    url,
                    title: tab_export.title.clone(),
                    favicon: None,
                    workspace_id,
                    suspended: true,
                    loading: false,
                    locked: tab_export.locked,
                    last_visited: Some(Self::current_timestamp_secs()),
                };

                if self.shell.create_tab(tab).is_ok() {
                    tabs_created += 1;
                }
            }
        }

        // Import shelf items
        for (workspace_name, items) in &data.shelf {
            self.shelves
                .entry(workspace_name.clone())
                .or_default()
                .extend(items.clone());
        }

        // Save state
        if let Err(e) = self.save_workspace_state() {
            errors.push(format!("Failed to save workspace state: {}", e));
        }

        Ok(ImportResult {
            success: errors.is_empty(),
            workspaces_created,
            tabs_created,
            errors,
        })
    }

    /// Clear all shelf items
    pub fn clear_shelf(&mut self) {
        self.shelves.clear();
    }

    /// Move a shelf item to the cellar (for recovery)
    #[allow(dead_code)]
    pub fn shelve_to_cellar(&mut self, workspace: &str, shelf_item: &ShelfItem) {
        let cellar_item = CellarItem {
            id: format!("cellar-{}", Self::current_timestamp_secs()),
            url: shelf_item.url.clone(),
            title: shelf_item.title.clone(),
            workspace: workspace.to_string(),
            originally_shelved: shelf_item.added_at,
            cellared_at: Self::current_timestamp_secs(),
        };
        self.cellar.push(cellar_item);

        // Keep cellar from growing unbounded (max 100 items)
        while self.cellar.len() > 100 {
            self.cellar.remove(0);
        }
    }

    /// Restore a cellar item back to the shelf
    pub fn restore_from_cellar(&mut self, cellar_id: &str) -> Option<ShelfItem> {
        let idx = self.cellar.iter().position(|item| item.id == cellar_id)?;
        let cellar_item = self.cellar.remove(idx);

        let shelf_item = ShelfItem {
            id: format!("shelf-{}", Self::current_timestamp_secs()),
            url: cellar_item.url,
            title: cellar_item.title,
            workspace: cellar_item.workspace.clone(),
            added_at: Self::current_timestamp_secs(),
        };

        // Add back to shelf
        self.shelves
            .entry(cellar_item.workspace)
            .or_default()
            .push(shelf_item.clone());

        Some(shelf_item)
    }

    /// Get all cellar items
    pub fn list_cellar(&self) -> &Vec<CellarItem> {
        &self.cellar
    }

    /// Clear the cellar
    pub fn clear_cellar(&mut self) {
        self.cellar.clear();
    }

    /// Check if a popup request should be allowed (flood protection)
    /// Returns true if allowed, false if blocked due to flood
    pub fn should_allow_popup(&mut self, url: &str) -> bool {
        use std::time::Duration;

        let now = Instant::now();
        let flood_window = Duration::from_secs(2);
        let duplicate_window = Duration::from_millis(500);
        let max_popups_per_window = 5;

        // Clean up old entries
        self.recent_popups
            .retain(|(_, timestamp)| now.duration_since(*timestamp) < flood_window);

        // Check for duplicate URL in short window (same URL requested twice quickly)
        let is_duplicate = self.recent_popups.iter().any(|(popup_url, timestamp)| {
            popup_url == url && now.duration_since(*timestamp) < duplicate_window
        });

        if is_duplicate {
            tracing::warn!(
                "Popup blocked (duplicate): {} - same URL requested within 500ms",
                url
            );
            return false;
        }

        // Check for flood (too many popups in time window)
        if self.recent_popups.len() >= max_popups_per_window {
            tracing::warn!(
                "Popup blocked (flood): {} - {} popups in last 2 seconds",
                url,
                self.recent_popups.len()
            );
            return false;
        }

        // Record this popup request
        self.recent_popups.push((url.to_string(), now));
        true
    }

    /// Get the zoom level for a tab (defaults to 1.0 if not set)
    pub fn get_zoom_level(&self, tab_id: &str) -> f32 {
        *self.tab_zoom_levels.get(tab_id).unwrap_or(&1.0)
    }

    /// Set the zoom level for a tab (clamped to 0.5 - 3.0)
    pub fn set_zoom_level(&mut self, tab_id: &str, level: f32) -> f32 {
        let clamped = level.clamp(0.5, 3.0);
        self.tab_zoom_levels.insert(tab_id.to_string(), clamped);
        clamped
    }

    /// Zoom in for a tab (step = 0.1, max 3.0)
    pub fn zoom_in(&mut self, tab_id: &str) -> f32 {
        let current = self.get_zoom_level(tab_id);
        self.set_zoom_level(tab_id, current + 0.1)
    }

    /// Zoom out for a tab (step = 0.1, min 0.5)
    pub fn zoom_out(&mut self, tab_id: &str) -> f32 {
        let current = self.get_zoom_level(tab_id);
        self.set_zoom_level(tab_id, current - 0.1)
    }

    /// Reset zoom to 100% for a tab
    pub fn reset_zoom(&mut self, tab_id: &str) -> f32 {
        self.set_zoom_level(tab_id, 1.0)
    }

    /// Remove zoom level entry when a tab is closed
    #[allow(dead_code)]
    pub fn remove_tab_zoom(&mut self, tab_id: &str) {
        self.tab_zoom_levels.remove(tab_id);
    }

    /// Check if a tab is playing audio
    pub fn is_tab_playing_audio(&self, tab_id: &str) -> bool {
        *self.tab_audio_playing.get(tab_id).unwrap_or(&false)
    }

    /// Set audio playing state for a tab
    pub fn set_tab_audio_playing(&mut self, tab_id: &str, playing: bool) {
        self.tab_audio_playing.insert(tab_id.to_string(), playing);
    }

    /// Check if a tab is muted
    pub fn is_tab_muted(&self, tab_id: &str) -> bool {
        *self.tab_audio_muted.get(tab_id).unwrap_or(&false)
    }

    /// Set muted state for a tab
    pub fn set_tab_muted(&mut self, tab_id: &str, muted: bool) {
        self.tab_audio_muted.insert(tab_id.to_string(), muted);
    }

    /// Toggle mute state for a tab
    pub fn toggle_tab_mute(&mut self, tab_id: &str) -> bool {
        let currently_muted = self.is_tab_muted(tab_id);
        self.set_tab_muted(tab_id, !currently_muted);
        !currently_muted
    }

    /// Remove audio state entries when a tab is closed
    #[allow(dead_code)]
    pub fn remove_tab_audio_state(&mut self, tab_id: &str) {
        self.tab_audio_playing.remove(tab_id);
        self.tab_audio_muted.remove(tab_id);
    }

    /// Create with default configuration
    pub fn with_defaults() -> HiWaveResult<Self> {
        let mut state = Self::new(AppConfig::default())?;
        if let Err(err) = state.load_workspace_state() {
            tracing::warn!("Failed to load workspace state: {}", err);
        }
        if let Err(err) = state.load_visit_history() {
            tracing::warn!("Failed to load visit history: {}", err);
        }
        Ok(state)
    }

    /// Like `with_defaults`, but the workspace state lives in `file` instead of
    /// `workspace_state.json`, so a shell that cannot restore the default build's tabs does
    /// not share (or delete) them.
    pub fn with_workspace_state_file(file: &str) -> HiWaveResult<Self> {
        let mut state = Self::new(AppConfig::default())?;
        state.workspace_state_file = file.to_string();
        if let Err(err) = state.load_workspace_state() {
            tracing::warn!("Failed to load workspace state: {}", err);
        }
        if let Err(err) = state.load_visit_history() {
            tracing::warn!("Failed to load visit history: {}", err);
        }
        Ok(state)
    }

    pub fn load_workspace_state(&mut self) -> HiWaveResult<bool> {
        let path = self.workspace_state_path();
        if !path.exists() {
            return Ok(false);
        }
        match self.read_workspace_state(&path) {
            Ok(()) => {
                self.ensure_download_dir();
                Ok(true)
            }
            Err(err) => {
                // A session file that cannot be read must not be overwritten by the empty
                // state the first save would write: keep it under another name first.
                self.set_workspace_state_aside(&path);
                Err(err)
            }
        }
    }

    fn read_workspace_state(&mut self, path: &std::path::Path) -> HiWaveResult<()> {
        let contents = std::fs::read_to_string(path)?;
        if let Ok(snapshot) = serde_json::from_str::<WorkspaceStateSnapshot>(&contents) {
            self.shell.load_snapshot(snapshot.shell)?;
            self.shelves = snapshot.shelves;
            self.cellar = snapshot.cellar;
            self.downloads = snapshot.downloads;
            self.next_download_id = self
                .downloads
                .iter()
                .map(|item| item.id)
                .max()
                .map(|max| max + 1)
                .unwrap_or(1);
            self.sync_shelf_keys();
        } else {
            let snapshot: ShellSnapshot = serde_json::from_str(&contents).map_err(|err| {
                hiwave_core::HiWaveError::Config(format!("Invalid workspace state: {}", err))
            })?;
            self.shell.load_snapshot(snapshot)?;
            self.shelves.clear();
            self.cellar.clear();
            self.downloads.clear();
            self.next_download_id = 1;
            self.sync_shelf_keys();
        }
        Ok(())
    }

    /// Rename an unreadable session file to `<name>.corrupt`, `.corrupt-1`, ... (never over an
    /// earlier one). If that fails too, block saving for this run so the file survives.
    fn set_workspace_state_aside(&mut self, path: &std::path::Path) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut target = path.with_file_name(format!("{name}.corrupt"));
        let mut n = 1;
        while target.exists() {
            target = path.with_file_name(format!("{name}.corrupt-{n}"));
            n += 1;
        }
        match std::fs::rename(path, &target) {
            Ok(()) => tracing::warn!(
                "The saved session could not be read; kept as {} and starting empty",
                target.display()
            ),
            Err(err) => {
                tracing::warn!(
                    "The saved session could not be read and could not be moved aside ({}); not saving it this run",
                    err
                );
                self.workspace_state_save_blocked = true;
            }
        }
    }

    pub fn save_workspace_state(&self) -> HiWaveResult<()> {
        if self.workspace_state_save_blocked {
            return Err(hiwave_core::HiWaveError::Config(
                "Not saving: the saved session could not be read or moved aside".to_string(),
            ));
        }
        let path = self.workspace_state_path();
        let snapshot = WorkspaceStateSnapshot {
            shell: self.shell.snapshot(),
            shelves: self.shelves.clone(),
            cellar: self.cellar.clone(),
            downloads: self.downloads.clone(),
        };
        let data = serde_json::to_string_pretty(&snapshot).map_err(|err| {
            hiwave_core::HiWaveError::Config(format!("Failed to save workspace state: {}", err))
        })?;
        // Write a temp file next to it and rename it over the real one: a crash or a full disk
        // mid-write leaves the old file, not a truncated one.
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = path.with_file_name(format!("{name}.tmp"));
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(data.as_bytes())?;
            file.sync_all()?;
        }
        if let Err(err) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err.into());
        }
        Ok(())
    }

    fn workspace_state_path(&self) -> PathBuf {
        self.config.data_dir.join(&self.workspace_state_file)
    }

    fn ensure_download_dir(&self) {
        if let Err(err) = fs::create_dir_all(&self.config.download_dir) {
            tracing::warn!("Failed to create download directory: {}", err);
        }
    }

    pub fn prepare_download_path(&self, url: &str) -> PathBuf {
        self.ensure_download_dir();
        let file_name = Self::suggested_download_name(url, self.next_download_id);
        Self::unique_file_path(&self.config.download_dir, &file_name)
    }

    pub fn register_download(&mut self, url: &str, target_path: &Path) {
        let id = self.next_download_id;
        self.next_download_id = self.next_download_id.saturating_add(1);
        let file_name = target_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("download-{}", id));
        self.downloads.push(DownloadItem {
            id,
            url: url.to_string(),
            file_name,
            status: DownloadStatus::InProgress,
            path: Some(PortablePath::from(target_path)),
            progress: None,
            started_at: Self::current_timestamp_secs(),
            finished_at: None,
            error: None,
        });
    }

    pub fn finalize_download(&mut self, url: &str, saved_path: Option<PathBuf>, success: bool) {
        // Convert saved_path to string for comparison
        let saved_path_str = saved_path
            .as_ref()
            .and_then(|p| p.to_str().map(|s| s.to_string()));

        if let Some(item) = self
            .downloads
            .iter_mut()
            .rev()
            .filter(|item| item.status == DownloadStatus::InProgress)
            .find(|item| {
                // Match by path if available, otherwise by URL
                if let Some(path_value) = &saved_path_str {
                    if let Some(item_path) = &item.path {
                        return item_path.to_string_lossy() == *path_value;
                    }
                }
                item.url == url
            })
        {
            item.status = if success {
                DownloadStatus::Completed
            } else {
                DownloadStatus::Failed
            };
            item.finished_at = Some(Self::current_timestamp_secs());
            if let Some(buffered_path) = saved_path {
                item.path = Some(PortablePath::from(buffered_path.clone()));
                if let Some(name) = buffered_path.file_name() {
                    item.file_name = name.to_string_lossy().into_owned();
                }
            }
            item.error = if success {
                None
            } else {
                Some("Download failed".to_string())
            };
        }
    }

    pub fn downloads_snapshot(&self) -> Vec<DownloadItem> {
        self.downloads.clone()
    }

    pub fn active_download_count(&self) -> usize {
        self.downloads
            .iter()
            .filter(|item| item.status == DownloadStatus::InProgress)
            .count()
    }

    pub fn clear_download_history(&mut self) {
        self.downloads.clear();
        self.next_download_id = 1;
    }

    fn visit_history_path(&self) -> PathBuf {
        self.config.data_dir.join("visit_history.json")
    }

    pub fn load_visit_history(&mut self) -> HiWaveResult<bool> {
        let path = self.visit_history_path();
        if !path.exists() {
            return Ok(false);
        }

        let contents = std::fs::read_to_string(&path)?;
        match serde_json::from_str::<VisitHistorySnapshot>(&contents) {
            Ok(snapshot) => {
                self.visit_history = snapshot.records;
                self.next_visit_id = snapshot.next_id.max(1);
                Ok(true)
            }
            Err(err) => {
                tracing::warn!("Invalid visit history data: {}", err);
                Ok(false)
            }
        }
    }

    pub fn save_visit_history(&self) -> HiWaveResult<()> {
        let snapshot = VisitHistorySnapshot {
            records: self.visit_history.clone(),
            next_id: self.next_visit_id,
        };
        let data = serde_json::to_string_pretty(&snapshot).map_err(|err| {
            hiwave_core::HiWaveError::Config(format!("Failed to save visit history: {}", err))
        })?;
        std::fs::write(self.visit_history_path(), data)?;
        Ok(())
    }

    /// Return a sorted snapshot of visit history (newest first)
    pub fn visit_history_snapshot(&self) -> Vec<VisitRecord> {
        let mut snapshot = self.visit_history.clone();
        snapshot.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        snapshot
    }

    fn sanitize_optional_text(value: Option<String>) -> Option<String> {
        value.and_then(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    }

    pub fn record_visit(
        &mut self,
        url: &str,
        title: Option<String>,
        workspace: Option<String>,
    ) -> HiWaveResult<()> {
        let timestamp = Self::current_timestamp_secs();
        let normalized_title = Self::sanitize_optional_text(title);
        let normalized_workspace = Self::sanitize_optional_text(workspace);

        if let Some(record) = self.visit_history.iter_mut().find(|entry| entry.url == url) {
            record.count = record.count.saturating_add(1);
            record.timestamp = timestamp;
            if let Some(title) = normalized_title.clone() {
                record.title = Some(title);
            }
            if normalized_workspace.is_some() {
                record.workspace = normalized_workspace.clone();
            }
        } else {
            let entry = VisitRecord {
                id: self.next_visit_id,
                url: url.to_string(),
                title: normalized_title.clone(),
                workspace: normalized_workspace.clone(),
                timestamp,
                count: 1,
            };
            self.next_visit_id = self.next_visit_id.saturating_add(1);
            self.visit_history.push(entry);
        }

        self.visit_history
            .sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        self.save_visit_history()
    }

    pub fn clear_visit_history(&mut self) -> HiWaveResult<()> {
        self.visit_history.clear();
        self.next_visit_id = 1;
        self.save_visit_history()
    }

    fn suggested_download_name(url: &str, idx: u64) -> String {
        if let Ok(parsed) = Url::parse(url) {
            if let Some(name) = parsed
                .path_segments()
                .and_then(|segments| segments.filter(|seg| !seg.is_empty()).next_back())
            {
                return name.to_string();
            }
            if let Some(host) = parsed.host_str() {
                return host.to_string();
            }
        }
        format!("download-{}", idx)
    }

    fn unique_file_path(dir: &Path, file_name: &str) -> PathBuf {
        let mut candidate = dir.join(file_name);
        if !candidate.exists() {
            return candidate;
        }
        let (base, extension) = match file_name.rfind('.') {
            Some(idx) if idx > 0 => (file_name[..idx].to_string(), file_name[idx..].to_string()),
            _ => (file_name.to_string(), String::new()),
        };
        let mut counter = 1;
        loop {
            let next_name = if extension.is_empty() {
                format!("{}-{}", base, counter)
            } else {
                format!("{}-{}{}", base, counter, extension)
            };
            candidate = dir.join(&next_name);
            if !candidate.exists() {
                return candidate;
            }
            counter += 1;
        }
    }

    pub fn shelf_items_for_active_workspace(&self) -> Vec<ShelfItem> {
        let name = self.shell.get_active_workspace().map(|ws| ws.name.clone());
        if let Some(name) = name {
            self.shelf_items_for_workspace(&name)
        } else {
            Vec::new()
        }
    }

    pub fn shelf_items_for_workspace(&self, workspace_name: &str) -> Vec<ShelfItem> {
        self.shelves
            .get(workspace_name)
            .cloned()
            .unwrap_or_default()
    }

    pub fn shelf_items_all(&self) -> Vec<ShelfItem> {
        let mut items: Vec<ShelfItem> = self
            .shelves
            .values()
            .flat_map(|list| list.iter().cloned())
            .collect();
        items.sort_by_key(|item| item.added_at);
        items
    }

    pub fn shelf_count_all(&self) -> usize {
        self.shelves.values().map(|list| list.len()).sum()
    }

    pub fn add_to_shelf(&mut self, tab_id: TabId) -> HiWaveResult<Option<ShelfItem>> {
        let tab = match self.shell.get_tab(tab_id) {
            Some(tab) => tab.clone(),
            None => return Ok(None),
        };
        let workspace_name = self
            .shell
            .get_workspace(tab.workspace_id)
            .map(|ws| ws.name.clone())
            .unwrap_or_else(|| "Default".to_string());
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let item = ShelfItem {
            id: format!("shelf-{}-{}", tab.id.0, timestamp),
            url: tab.url.to_string(),
            title: tab.title.clone(),
            workspace: workspace_name.clone(),
            added_at: timestamp,
        };
        self.shelves
            .entry(workspace_name.clone())
            .or_default()
            .push(item.clone());
        let _ = self.shell.close_tab(tab_id);

        // Track shelf addition
        if let Ok(parsed_url) = url::Url::parse(&item.url) {
            if let Some(domain) = parsed_url.host_str() {
                let workspace_id = tab.workspace_id.0.to_string();
                let _ = self
                    .analytics
                    .track_tab_to_shelf(domain, Some(&workspace_id));
            }
        }

        Ok(Some(item))
    }

    pub fn restore_from_shelf(
        &mut self,
        id: &str,
        target_workspace_id: Option<WorkspaceId>,
    ) -> HiWaveResult<Option<RestoredShelfItem>> {
        let mut found: Option<ShelfItem> = None;
        let mut source_workspace = None;

        for (workspace, items) in self.shelves.iter_mut() {
            if let Some(index) = items.iter().position(|item| item.id == id) {
                found = Some(items.remove(index));
                source_workspace = Some(workspace.clone());
                break;
            }
        }

        let item = match found {
            Some(item) => item,
            None => return Ok(None),
        };

        let workspace_id = if let Some(ws_id) = target_workspace_id {
            Some(ws_id)
        } else {
            let target_workspace = source_workspace.unwrap_or_else(|| item.workspace.clone());
            self.shell
                .list_workspaces()
                .iter()
                .find(|ws| ws.name == target_workspace)
                .map(|ws| ws.id)
                .or_else(|| self.shell.get_active_workspace().map(|ws| ws.id))
        };

        let workspace_id = match workspace_id {
            Some(id) => id,
            None => return Ok(None),
        };

        let url = url::Url::parse(&item.url)?;
        let tab = hiwave_core::types::TabInfo {
            id: hiwave_core::types::TabId::new(),
            url: url.clone(),
            title: item.title.clone(),
            favicon: None,
            workspace_id,
            suspended: false,
            loading: true,
            locked: false,
            last_visited: Some(Self::current_timestamp_secs()),
        };
        let tab_id = tab.id;
        let _ = self.shell.create_tab(tab);
        let _ = self.shell.set_active_workspace(workspace_id);
        let _ = self.shell.set_active_tab(tab_id);

        // Track shelf restoration
        if let Some(domain) = url.host_str() {
            let workspace_id_str = workspace_id.0.to_string();
            let _ = self
                .analytics
                .track_tab_from_shelf(domain, Some(&workspace_id_str));
        }

        Ok(Some(RestoredShelfItem {
            url: url.to_string(),
            workspace_id,
        }))
    }

    pub fn delete_from_shelf(&mut self, id: &str) -> bool {
        for items in self.shelves.values_mut() {
            if let Some(index) = items.iter().position(|item| item.id == id) {
                items.remove(index);
                return true;
            }
        }
        false
    }

    pub fn ensure_workspace_shelf(&mut self, workspace_name: &str) {
        self.shelves.entry(workspace_name.to_string()).or_default();
    }

    pub fn remove_workspace_shelf(&mut self, workspace_name: &str) {
        self.shelves.remove(workspace_name);
    }

    fn sync_shelf_keys(&mut self) {
        let mut workspace_names: Vec<String> = self
            .shell
            .list_workspaces()
            .iter()
            .map(|ws| ws.name.clone())
            .collect();
        workspace_names.sort();

        self.shelves
            .retain(|name, _| workspace_names.contains(name));
        for name in workspace_names {
            self.shelves.entry(name).or_default();
        }
    }

    pub fn lock_tab(&mut self, tab_id: TabId) -> HiWaveResult<()> {
        self.shell.lock_tab(tab_id)
    }

    pub fn unlock_tab(&mut self, tab_id: TabId) -> HiWaveResult<()> {
        self.shell.unlock_tab(tab_id)
    }

    pub fn touch_tab(&mut self, tab_id: TabId) -> HiWaveResult<()> {
        self.shell.touch_tab(tab_id)
    }

    pub fn get_stale_locks(&self, threshold_days: u64) -> Vec<hiwave_core::types::TabInfo> {
        self.shell.stale_locks(threshold_days)
    }

    pub fn workspace_locked_count(&self, workspace_id: WorkspaceId) -> usize {
        self.shell.workspace_locked_count(workspace_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShelfItem {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    pub workspace: String,
    pub added_at: u64,
}

/// An item that has aged out of the shelf (stored in the cellar for recovery)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellarItem {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    /// Which workspace this originally came from
    pub workspace: String,
    /// When the item was originally added to the shelf
    pub originally_shelved: u64,
    /// When the item moved to cellar (aged out)
    pub cellared_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceStateSnapshot {
    shell: ShellSnapshot,
    shelves: HashMap<String, Vec<ShelfItem>>,
    #[serde(default)]
    cellar: Vec<CellarItem>,
    #[serde(default)]
    downloads: Vec<DownloadItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VisitHistorySnapshot {
    records: Vec<VisitRecord>,
    next_id: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestoredShelfItem {
    pub url: String,
    pub workspace_id: WorkspaceId,
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Data directory for persistent storage
    #[serde(skip)]
    pub data_dir: PathBuf,

    /// Directory where downloads are stored
    #[serde(skip)]
    pub download_dir: PathBuf,

    /// Default search engine
    pub search_engine: String,

    /// Default homepage
    pub homepage: String,

    /// Enable ad blocking
    pub shield_enabled: bool,

    /// Focus mode default duration (minutes)
    pub focus_duration_minutes: u32,

    /// Focus mode break duration (minutes)
    pub focus_break_minutes: u32,

    /// Maximum tabs per workspace
    pub max_tabs_per_workspace: usize,

    /// Maximum workspaces
    pub max_workspaces: usize,

    /// Tab decay timeout (seconds before tab is considered stale)
    pub tab_decay_timeout_secs: u64,

    /// Auto-suspend timeout for workspaces (seconds)
    pub workspace_suspend_timeout_secs: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        // Get data directory
        let data_dir = dirs_or_default();
        let download_dir = data_dir.join("downloads");

        Self {
            data_dir,
            download_dir,
            search_engine: "https://duckduckgo.com/?q=".to_string(),
            homepage: "hiwave://newtab".to_string(),
            shield_enabled: true,
            focus_duration_minutes: 25,
            focus_break_minutes: 5,
            max_tabs_per_workspace: 100,
            max_workspaces: 10,
            tab_decay_timeout_secs: 7 * 24 * 60 * 60, // 7 days
            workspace_suspend_timeout_secs: 300,      // 5 minutes
        }
    }
}

/// Get data directory, with fallback
fn dirs_or_default() -> PathBuf {
    // HIWAVE_DATA_DIR overrides it: a portable install, and tests that must not
    // touch the real history, downloads and settings.
    if let Some(dir) = std::env::var_os("HIWAVE_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    // Try standard data directory
    if let Some(data_dir) = dirs::data_local_dir() {
        return data_dir.join("hiwave");
    }

    // Fallback to current directory
    PathBuf::from(".hiwave")
}

/// User-configurable settings (separate from AppConfig which has system paths)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    // General
    /// Theme: "dark", "light", or "system"
    pub theme: String,
    /// Default mode: "essentials", "balanced", or "zen"
    pub default_mode: String,
    /// Sidebar open by default on startup
    pub sidebar_default_open: bool,
    /// Auto-hide sidebar after N seconds (None = disabled)
    pub sidebar_auto_hide_secs: Option<u32>,
    /// Sidebar width in pixels (default: 220, min: 48, max: 400)
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: u32,
    /// Auto-hide URL bar after N seconds (None = disabled)
    pub url_bar_auto_hide_secs: Option<u32>,

    // Tab Behavior
    /// Days before tabs start visually fading (default: 7)
    pub tab_decay_days: u32,
    /// Days before tabs auto-move to shelf in Zen mode (None = disabled)
    pub auto_shelf_days: Option<u32>,
    /// Custom new tab URL (empty = default new tab page)
    pub new_tab_url: String,

    // Shield
    /// Shield (ad/tracker blocking) enabled by default
    pub shield_enabled: bool,

    // Focus Mode
    /// Default focus mode duration in minutes
    pub focus_duration_minutes: u32,
    /// Default focus mode break duration in minutes
    pub focus_break_minutes: u32,

    // Import Display
    /// How to display imported workspace sources: "abbreviation" (BRV:) or "symbol"
    pub import_prefix_style: String,

    // Advanced
    /// Enable developer tools
    pub devtools_enabled: bool,
    /// Workspace save interval in seconds
    pub workspace_save_interval_secs: u32,
}

// Serde default function for sidebar width
fn default_sidebar_width() -> u32 {
    220
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            // General
            theme: "dark".to_string(),
            default_mode: "balanced".to_string(),
            sidebar_default_open: true,
            sidebar_auto_hide_secs: None,
            sidebar_width: 220,
            url_bar_auto_hide_secs: None,

            // Tab Behavior
            tab_decay_days: 7,
            auto_shelf_days: None,
            new_tab_url: String::new(),

            // Shield
            shield_enabled: true,

            // Focus Mode
            focus_duration_minutes: 25,
            focus_break_minutes: 5,

            // Import Display
            import_prefix_style: "abbreviation".to_string(),

            // Advanced
            devtools_enabled: false,
            workspace_save_interval_secs: 30,
        }
    }
}

impl UserSettings {
    /// Load settings from the given path, returning defaults if not found
    pub fn load(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }

        match fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save settings to the given path
    pub fn save(&self, path: &Path) -> HiWaveResult<()> {
        let data = serde_json::to_string_pretty(self).map_err(|e| {
            hiwave_core::HiWaveError::Config(format!("Failed to serialize settings: {}", e))
        })?;
        fs::write(path, data)?;
        Ok(())
    }
}

/// Export format for backing up workspaces and settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiWaveExport {
    /// Export format version
    pub version: String,
    /// Unix timestamp when exported
    pub exported_at: u64,
    /// User settings (if included)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<UserSettings>,
    /// Workspaces with their tabs
    pub workspaces: Vec<WorkspaceExport>,
    /// Shelf items by workspace name
    pub shelf: HashMap<String, Vec<ShelfItem>>,
}

/// Workspace data for export
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceExport {
    pub name: String,
    pub tabs: Vec<TabExport>,
}

/// Tab data for export
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabExport {
    pub url: String,
    pub title: Option<String>,
    pub locked: bool,
}

impl HiWaveExport {
    pub fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self {
            version: "1.0".to_string(),
            exported_at: now,
            settings: None,
            workspaces: Vec::new(),
            shelf: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod workspace_state_file_tests {
    use super::*;

    fn fresh(name: &str) -> AppState {
        let dir = std::env::temp_dir().join(format!("hiwave_wsfile_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = AppConfig::default();
        config.data_dir = dir.clone();
        config.download_dir = dir.join("downloads");
        AppState::new(config).expect("state")
    }

    fn file(s: &AppState) -> PathBuf {
        s.config.data_dir.join(&s.workspace_state_file)
    }

    #[test]
    fn a_save_leaves_the_file_and_no_temp_file_and_loads_back() {
        let mut s = fresh("roundtrip");
        s.shell.create_workspace("Work".to_string());
        s.save_workspace_state().unwrap();
        assert!(file(&s).exists());
        assert!(!s.config.data_dir.join("workspace_state.json.tmp").exists(), "the temp file is renamed away");
        let mut again = AppState::new(s.config.clone()).unwrap();
        assert!(again.load_workspace_state().unwrap());
        assert_eq!(again.shell.list_workspaces().len(), 2);
    }

    #[test]
    fn an_empty_session_file_is_kept_aside_not_overwritten() {
        let mut s = fresh("empty");
        std::fs::write(file(&s), "").unwrap();
        assert!(s.load_workspace_state().is_err());
        assert!(!file(&s).exists(), "moved out of the way");
        assert!(s.config.data_dir.join("workspace_state.json.corrupt").exists());
        // The first save writes a fresh file and leaves the kept one alone.
        s.save_workspace_state().unwrap();
        assert!(file(&s).exists());
        assert_eq!(std::fs::read_to_string(s.config.data_dir.join("workspace_state.json.corrupt")).unwrap(), "");
    }

    #[test]
    fn a_corrupt_session_file_keeps_its_bytes_and_each_one_gets_its_own_name() {
        let mut s = fresh("corrupt");
        std::fs::write(file(&s), "{ this is not json").unwrap();
        assert!(s.load_workspace_state().is_err());
        let first = s.config.data_dir.join("workspace_state.json.corrupt");
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "{ this is not json");

        // A second bad file does not replace the first.
        std::fs::write(file(&s), "[1, 2").unwrap();
        assert!(s.load_workspace_state().is_err());
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "{ this is not json");
        assert_eq!(
            std::fs::read_to_string(s.config.data_dir.join("workspace_state.json.corrupt-1")).unwrap(),
            "[1, 2"
        );
    }

    #[test]
    fn a_file_that_is_not_utf8_is_kept_aside_too() {
        let mut s = fresh("binary");
        std::fs::write(file(&s), [0xff, 0xfe, 0x00, 0x9f]).unwrap();
        assert!(s.load_workspace_state().is_err());
        assert_eq!(
            std::fs::read(s.config.data_dir.join("workspace_state.json.corrupt")).unwrap(),
            vec![0xff, 0xfe, 0x00, 0x9f]
        );
    }

    #[test]
    fn a_missing_file_is_not_an_error_and_a_good_one_is_not_moved() {
        let mut s = fresh("missing");
        assert!(!s.load_workspace_state().unwrap());
        s.save_workspace_state().unwrap();
        let before = std::fs::read(file(&s)).unwrap();
        assert!(s.load_workspace_state().unwrap());
        assert_eq!(std::fs::read(file(&s)).unwrap(), before, "a readable file is left as it was");
        assert!(!s.config.data_dir.join("workspace_state.json.corrupt").exists());
    }

    #[test]
    fn saving_is_refused_when_the_unreadable_file_could_not_be_moved_aside() {
        let mut s = fresh("blocked");
        s.workspace_state_save_blocked = true;
        assert!(s.save_workspace_state().is_err());
        assert!(!file(&s).exists(), "nothing was written");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_portable_path_serializes_with_forward_slashes() {
        // Create a path with backslashes (simulating Windows)
        let path = PortablePath::new(PathBuf::from("downloads/subdir/test.pdf"));
        let json = serde_json::to_string(&path).unwrap();

        // Should serialize with forward slashes
        assert_eq!(json, "\"downloads/subdir/test.pdf\"");
    }

    #[test]
    fn test_portable_path_deserializes_correctly() {
        // Deserialize a path with forward slashes
        let json = "\"downloads/subdir/test.pdf\"";
        let path: PortablePath = serde_json::from_str(json).unwrap();

        // Should be a valid path
        assert!(path.to_string_lossy().contains("downloads"));
        assert!(path.to_string_lossy().contains("test.pdf"));
    }

    #[test]
    fn test_portable_path_roundtrip() {
        let original = PortablePath::new(PathBuf::from("some/nested/path/file.txt"));

        // Serialize
        let json = serde_json::to_string(&original).unwrap();

        // Deserialize
        let restored: PortablePath = serde_json::from_str(&json).unwrap();

        // Should match (comparing string representations)
        assert_eq!(
            original.to_string_lossy().replace('\\', "/"),
            restored.to_string_lossy().replace('\\', "/")
        );
    }

    #[test]
    fn test_download_item_with_portable_path() {
        let item = DownloadItem {
            id: 1,
            url: "https://example.com/file.pdf".to_string(),
            file_name: "file.pdf".to_string(),
            status: DownloadStatus::Completed,
            path: Some(PortablePath::new(PathBuf::from("downloads/file.pdf"))),
            progress: Some(1.0),
            started_at: 1000,
            finished_at: Some(2000),
            error: None,
        };

        // Serialize
        let json = serde_json::to_string(&item).unwrap();

        // Path should be serialized with forward slashes
        assert!(json.contains("\"downloads/file.pdf\""));

        // Deserialize
        let restored: DownloadItem = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, 1);
        assert!(restored.path.is_some());
    }

    #[test]
    fn test_download_item_path_string() {
        let item = DownloadItem {
            id: 1,
            url: "https://example.com/file.pdf".to_string(),
            file_name: "file.pdf".to_string(),
            status: DownloadStatus::Completed,
            path: Some(PortablePath::new(PathBuf::from("downloads/file.pdf"))),
            progress: Some(1.0),
            started_at: 1000,
            finished_at: Some(2000),
            error: None,
        };

        let path_str = item.path_string();
        assert!(path_str.is_some());
        assert!(path_str.unwrap().contains("downloads"));
    }

    #[test]
    fn test_portable_path_from_implementations() {
        // From PathBuf
        let p1: PortablePath = PathBuf::from("test/path").into();
        assert!(p1.to_string_lossy().contains("test"));

        // From &Path
        let path = Path::new("test/path");
        let p2: PortablePath = path.into();
        assert!(p2.to_string_lossy().contains("test"));

        // From String
        let p3: PortablePath = String::from("test/path").into();
        assert!(p3.to_string_lossy().contains("test"));
    }

    #[test]
    fn test_download_status_serialization() {
        assert_eq!(
            serde_json::to_string(&DownloadStatus::InProgress).unwrap(),
            "\"in_progress\""
        );
        assert_eq!(
            serde_json::to_string(&DownloadStatus::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&DownloadStatus::Failed).unwrap(),
            "\"failed\""
        );
    }
}
