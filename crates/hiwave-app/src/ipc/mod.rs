//! IPC (Inter-Process Communication) module for HiWave
//!
//! This module handles communication between the JavaScript frontend
//! and the Rust backend via WebView IPC.

pub mod commands;

use serde::{Deserialize, Serialize};

/// IPC message from JavaScript to Rust
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcMessage {
    // Navigation
    Navigate {
        url: String,
    },
    GoBack,
    GoForward,
    Reload,
    Stop,

    // Tab management
    CreateTab {
        url: Option<String>,
    },
    CloseTab {
        id: String,
    },
    ActivateTab {
        id: String,
    },
    GetTabs,
    GetActiveTab,
    UpdateActiveTabUrl {
        url: String,
    },

    // Workspace management
    CreateWorkspace {
        name: String,
    },
    DeleteWorkspace {
        id: String,
    },
    RenameWorkspace {
        id: String,
        name: String,
    },
    ActivateWorkspace {
        id: String,
    },
    GetWorkspaces,
    GetActiveWorkspace,

    // Shelf management
    GetShelf {
        scope: Option<String>,
    },
    AddToShelf {
        tab_id: String,
    },
    RestoreFromShelf {
        id: String,
        workspace_id: Option<String>,
    },
    DeleteFromShelf {
        id: String,
    },
    MoveTabToWorkspace {
        tab_id: String,
        workspace_id: String,
    },

    // Page locking
    LockPage {
        tab_id: String,
    },
    UnlockPage {
        tab_id: String,
    },
    GetStaleLocks {
        threshold_days: Option<u64>,
    },

    // Command palette
    SearchCommands {
        query: String,
    },
    ExecuteCommand {
        id: String,
    },
    OpenCommandPalette,
    CloseCommandPalette,

    // UI shortcuts (forwarded from content WebView)
    FocusAddressBar,
    OpenFind,
    ToggleSidebar,
    Refresh,
    ActivateTabByIndex {
        index: usize,
    },

    // Shield (ad blocking)
    GetShieldStats,
    ToggleShield {
        enabled: bool,
    },
    /// Block the domain of a tab and close it
    BlockAndClose {
        id: String,
    },
    /// Get user blocklist
    GetBlocklist,
    /// Unblock a domain
    UnblockDomain {
        domain: String,
    },

    // Vault (password manager)
    GetVaultStatus,
    UnlockVault {
        password: String,
    },
    LockVault,
    GetAllCredentials,
    SaveCredential {
        url: String,
        username: String,
        password: String,
    },
    DeleteCredential {
        id: i64,
    },

    // Focus mode
    EnterFocusMode,
    ExitFocusMode,
    ToggleFocusMode,
    GetFocusModeStatus,
    GetFocusModeConfig,
    SaveFocusModeConfig {
        config: serde_json::Value,
    },
    FocusScrollProgress {
        progress: f32,
    },
    FocusMediaPlaying {
        playing: bool,
    },
    FocusPageLoaded {
        url: String,
    },
    ShowFocusPeek,
    HideFocusPeek,
    AddToFocusBlocklist {
        domain: String,
    },
    RemoveFromFocusBlocklist {
        domain: String,
    },

    // Mode selector
    SetMode {
        mode: String,
    },

    // Settings UI
    OpenSettings,
    /// Sent by the Chrome UI when the document finishes initializing (WinCairo)
    ChromeReady,
    CloseSettings,

    // Misc
    GetConfig,
    Log {
        level: String,
        message: String,
    },
    OpenExternal {
        url: String,
    },
    // Downloads
    GetDownloads,
    ClearDownloads,
    OpenDownload {
        path: String,
    },
    ShowDownloadInFolder {
        path: String,
    },
    GetVisitHistory,
    ClearVisitHistory,

    // Sidebar layout
    SetSidebarOpen {
        open: bool,
    },
    SetRightPanelOpen {
        open: bool,
    },
    SaveSidebarWidth {
        width: u32,
    },
    GetSidebarWidth,
    /// Update sidebar width in real-time during drag (triggers layout update)
    SetSidebarWidth {
        width: f64,
    },

    FindInPage {
        query: String,
        case_sensitive: bool,
        direction: String,
    },
    FindInPageResult {
        result: serde_json::Value,
    },

    // Window controls
    StartWindowDrag,
    WindowMinimize,
    WindowToggleMaximize,
    WindowClose,

    // Dynamic resize
    ExpandChrome,
    ExpandChromeSmall,
    CollapseChrome,
    ExpandShelf,
    CollapseShelf,

    // Browser import
    GetBrowserProfiles {
        browser: String,
    },
    ImportBookmarks {
        browser: String,
        profile_path: String,
    },

    // Settings management
    GetSettings,
    SaveSettings {
        settings: serde_json::Value,
    },

    // Export/Import
    ExportData {
        include_settings: bool,
        include_workspaces: bool,
    },
    SaveExportToFile {
        data: String,
        suggested_name: String,
    },
    ImportData {
        data: String,
        replace: bool,
    },
    PickImportFile,

    // Data cleanup
    ClearBrowsingData {
        history: bool,
        downloads: bool,
        shelf: bool,
    },

    // Print
    PrintPage,

    // Zoom controls
    ZoomIn,
    ZoomOut,
    ResetZoom,

    // Tab audio state
    TabAudioStateChanged {
        playing: bool,
    },

    // Autofill (vault integration)
    TriggerAutofill,
    GetCredentialsForAutofill {
        domain: String,
    },
    ToggleTabMute {
        id: String,
    },

    // Cellar (recovered shelf items)
    GetCellar,
    RestoreFromCellar {
        id: String,
    },
    ClearCellar,

    // Analytics & Reports
    GetTodayStats,
    GetWeeklyReport,
    GetMonthlyReport,
    GetCustomReport {
        start_date: String,
        end_date: String,
    },
    GetTopDomains {
        limit: Option<usize>,
    },
    GetWorkspaceStats,
    GetAnalyticsSettings,
    UpdateAnalyticsSettings {
        enabled: bool,
        retention_days: i32,
        weekly_report: bool,
        report_day: String,
    },
    ClearAnalyticsData,
    ExportAnalyticsData {
        format: String, // "json" or "csv"
    },
    ResetAnalyticsData,
}

/// IPC response from Rust to JavaScript
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    Success { data: serde_json::Value },
    Error { message: String },
}

impl IpcResponse {
    pub fn success<T: Serialize>(data: T) -> Self {
        IpcResponse::Success {
            data: serde_json::to_value(data).unwrap_or(serde_json::Value::Null),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        IpcResponse::Error {
            message: message.into(),
        }
    }
}

/// Credential for autofill (includes password for form filling)
#[derive(Debug, Clone, Serialize)]
pub struct AutofillCredential {
    pub id: i64,
    pub domain: String,
    pub username: String,
    pub password: String,
}

/// Tab information for IPC responses
#[derive(Debug, Clone, Serialize)]
pub struct TabInfo {
    pub id: String,
    pub url: String,
    pub title: String,
    pub is_active: bool,
    pub is_loading: bool,
    pub favicon: Option<String>,
    pub locked: bool,
    pub last_visited: Option<u64>,
    pub is_playing_audio: bool,
    pub is_muted: bool,
}

/// Workspace information for IPC responses
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceInfo {
    pub id: String,
    pub name: String,
    pub tab_count: usize,
    pub is_active: bool,
    pub is_suspended: bool,
    pub locked_count: usize,
    pub tabs: Vec<WorkspaceTabSummary>,
}

/// Simplified tab metadata used in workspace lists
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceTabSummary {
    pub id: String,
    pub title: String,
    pub locked: bool,
    pub is_active: bool,
    pub url: String,
}

/// Command palette item for IPC responses
#[derive(Debug, Clone, Serialize)]
pub struct CommandItem {
    pub id: String,
    pub label: String,
    pub category: String,
    pub shortcut: Option<String>,
    pub score: i64,
}

/// Shield stats for IPC responses
#[derive(Debug, Clone, Serialize)]
pub struct ShieldStats {
    pub enabled: bool,
    pub requests_blocked: u64,
    pub trackers_blocked: u64,
}

/// Focus mode status for IPC responses
#[derive(Debug, Clone, Serialize)]
pub struct FocusModeStatus {
    pub active: bool,
    pub remaining_seconds: Option<u32>,
    pub total_seconds: Option<u32>,
}

/// The JavaScript bridge that gets injected into every page
pub const JS_BRIDGE: &str = r#"
(function() {
    // Create the hiwave namespace
    window.hiwave = {
        // Navigation
        navigate: (url) => window.ipc.postMessage(JSON.stringify({ cmd: 'navigate', url })),
        goBack: () => window.ipc.postMessage(JSON.stringify({ cmd: 'go_back' })),
        goForward: () => window.ipc.postMessage(JSON.stringify({ cmd: 'go_forward' })),
        reload: () => window.ipc.postMessage(JSON.stringify({ cmd: 'reload' })),
        stop: () => window.ipc.postMessage(JSON.stringify({ cmd: 'stop' })),

        // Tab management
        createTab: (url) => window.ipc.postMessage(JSON.stringify({ cmd: 'create_tab', url })),
        closeTab: (id) => window.ipc.postMessage(JSON.stringify({ cmd: 'close_tab', id })),
        activateTab: (id) => window.ipc.postMessage(JSON.stringify({ cmd: 'activate_tab', id })),
        getTabs: () => window.ipc.postMessage(JSON.stringify({ cmd: 'get_tabs' })),
        getActiveTab: () => window.ipc.postMessage(JSON.stringify({ cmd: 'get_active_tab' })),

        // Workspace management
        createWorkspace: (name) => window.ipc.postMessage(JSON.stringify({ cmd: 'create_workspace', name })),
        deleteWorkspace: (id) => window.ipc.postMessage(JSON.stringify({ cmd: 'delete_workspace', id })),
        renameWorkspace: (id, name) => window.ipc.postMessage(JSON.stringify({ cmd: 'rename_workspace', id, name })),
        activateWorkspace: (id) => window.ipc.postMessage(JSON.stringify({ cmd: 'activate_workspace', id })),
        getWorkspaces: () => window.ipc.postMessage(JSON.stringify({ cmd: 'get_workspaces' })),
        getActiveWorkspace: () => window.ipc.postMessage(JSON.stringify({ cmd: 'get_active_workspace' })),

        // Command palette
        searchCommands: (query) => window.ipc.postMessage(JSON.stringify({ cmd: 'search_commands', query })),
        executeCommand: (id) => window.ipc.postMessage(JSON.stringify({ cmd: 'execute_command', id })),
        openCommandPalette: () => window.ipc.postMessage(JSON.stringify({ cmd: 'open_command_palette' })),
        closeCommandPalette: () => window.ipc.postMessage(JSON.stringify({ cmd: 'close_command_palette' })),

        // Shield
        getShieldStats: () => window.ipc.postMessage(JSON.stringify({ cmd: 'get_shield_stats' })),
        toggleShield: (enabled) => window.ipc.postMessage(JSON.stringify({ cmd: 'toggle_shield', enabled })),
        blockAndClose: (id) => window.ipc.postMessage(JSON.stringify({ cmd: 'block_and_close', id })),
        getBlocklist: () => window.ipc.postMessage(JSON.stringify({ cmd: 'get_blocklist' })),
        unblockDomain: (domain) => window.ipc.postMessage(JSON.stringify({ cmd: 'unblock_domain', domain })),

        // Vault
        unlockVault: (password) => window.ipc.postMessage(JSON.stringify({ cmd: 'unlock_vault', password })),
        lockVault: () => window.ipc.postMessage(JSON.stringify({ cmd: 'lock_vault' })),
        getCredentials: (domain) => window.ipc.postMessage(JSON.stringify({ cmd: 'get_credentials', domain })),
        saveCredential: (url, username, password) => window.ipc.postMessage(JSON.stringify({ cmd: 'save_credential', url, username, password })),

        // Focus mode
        startFocusMode: (durationMinutes) => window.ipc.postMessage(JSON.stringify({ cmd: 'start_focus_mode', duration_minutes: durationMinutes })),
        stopFocusMode: () => window.ipc.postMessage(JSON.stringify({ cmd: 'stop_focus_mode' })),
        getFocusModeStatus: () => window.ipc.postMessage(JSON.stringify({ cmd: 'get_focus_mode_status' })),

        // Misc
        getConfig: () => window.ipc.postMessage(JSON.stringify({ cmd: 'get_config' })),
        log: (level, message) => window.ipc.postMessage(JSON.stringify({ cmd: 'log', level, message })),
        getDownloads: () => window.ipc.postMessage(JSON.stringify({ cmd: 'get_downloads' })),
        clearDownloads: () => window.ipc.postMessage(JSON.stringify({ cmd: 'clear_downloads' })),
        openDownload: (path) => window.ipc.postMessage(JSON.stringify({ cmd: 'open_download', path })),
        showDownloadInFolder: (path) => window.ipc.postMessage(JSON.stringify({ cmd: 'show_download_in_folder', path })),

        // Response handlers
        _handlers: {},
        _nextId: 1,

        // Register a handler for responses
        onResponse: (handler) => {
            const id = hiwave._nextId++;
            hiwave._handlers[id] = handler;
            return id;
        },

        // Remove a handler
        removeHandler: (id) => {
            delete hiwave._handlers[id];
        }
    };

    // Listen for responses from Rust
    window.addEventListener('hiwave-response', (event) => {
        const response = event.detail;
        Object.values(hiwave._handlers).forEach(handler => {
            try {
                handler(response);
            } catch (e) {
                console.error('Handler error:', e);
            }
        });
    });

    console.log('HiWave IPC bridge initialized');
})();
"#;
