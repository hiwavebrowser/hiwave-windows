//! IPC command handlers
//!
//! This module contains handlers for all IPC commands from the JavaScript frontend.

use super::{
    CommandItem, FocusModeStatus, IpcMessage, IpcResponse, ShieldStats, TabInfo, WorkspaceInfo,
    WorkspaceTabSummary,
};
use crate::import::{self, converter::ConversionConfig, Browser};
use crate::platform::{get_platform_manager, PlatformManager};
use crate::state::AppState;
use hiwave_core::types::TabInfo as CoreTabInfo;
use hiwave_core::types::{TabId, WorkspaceId};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{debug, error, info, warn};
use url::Url;

/// Global platform manager instance
static PLATFORM: OnceLock<Box<dyn PlatformManager>> = OnceLock::new();

/// Get the platform manager instance
fn platform() -> &'static dyn PlatformManager {
    PLATFORM.get_or_init(|| get_platform_manager()).as_ref()
}

/// Handle an IPC message and return a response
pub fn handle_message(state: &Arc<Mutex<AppState>>, message: IpcMessage) -> IpcResponse {
    match message {
        // Navigation commands
        IpcMessage::Navigate { url } => handle_navigate(state, &url),
        IpcMessage::GoBack => handle_go_back(state),
        IpcMessage::GoForward => handle_go_forward(state),
        IpcMessage::Reload => handle_reload(state),
        IpcMessage::Stop => handle_stop(state),

        // Tab commands
        IpcMessage::CreateTab { url } => handle_create_tab(state, url.as_deref()),
        IpcMessage::CloseTab { id } => handle_close_tab(state, &id),
        IpcMessage::ActivateTab { id } => handle_activate_tab(state, &id),
        IpcMessage::GetTabs => handle_get_tabs(state),
        IpcMessage::GetActiveTab => handle_get_active_tab(state),
        IpcMessage::UpdateActiveTabUrl { url } => handle_update_active_tab_url(state, &url),

        // Workspace commands
        IpcMessage::CreateWorkspace { name } => handle_create_workspace(state, &name),
        IpcMessage::DeleteWorkspace { id } => handle_delete_workspace(state, &id),
        IpcMessage::RenameWorkspace { id, name } => handle_rename_workspace(state, &id, &name),
        IpcMessage::ActivateWorkspace { id } => handle_activate_workspace(state, &id),
        IpcMessage::GetWorkspaces => handle_get_workspaces(state),
        IpcMessage::GetActiveWorkspace => handle_get_active_workspace(state),

        // Shelf commands
        IpcMessage::GetShelf { scope } => handle_get_shelf(state, scope.as_deref()),
        IpcMessage::AddToShelf { tab_id } => handle_add_to_shelf(state, &tab_id),
        IpcMessage::RestoreFromShelf { id, workspace_id } => {
            handle_restore_from_shelf(state, &id, workspace_id.as_deref())
        }
        IpcMessage::DeleteFromShelf { id } => handle_delete_from_shelf(state, &id),
        IpcMessage::MoveTabToWorkspace {
            tab_id,
            workspace_id,
        } => handle_move_tab_to_workspace(state, &tab_id, &workspace_id),

        // Page locking commands
        IpcMessage::LockPage { tab_id } => handle_lock_page(state, &tab_id),
        IpcMessage::UnlockPage { tab_id } => handle_unlock_page(state, &tab_id),
        IpcMessage::GetStaleLocks { threshold_days } => {
            handle_get_stale_locks(state, threshold_days)
        }

        // Command palette commands
        IpcMessage::SearchCommands { query } => handle_search_commands(state, &query),
        IpcMessage::ExecuteCommand { id } => handle_execute_command(state, &id),
        IpcMessage::OpenCommandPalette => handle_open_command_palette(state),
        IpcMessage::CloseCommandPalette => handle_close_command_palette(state),

        // Shield commands
        IpcMessage::GetShieldStats => handle_get_shield_stats(state),
        IpcMessage::ToggleShield { enabled } => handle_toggle_shield(state, enabled),
        IpcMessage::BlockAndClose { id } => handle_block_and_close(state, &id),
        IpcMessage::GetBlocklist => handle_get_blocklist(state),
        IpcMessage::UnblockDomain { domain } => handle_unblock_domain(state, &domain),

        // Vault commands
        IpcMessage::GetVaultStatus => handle_get_vault_status(state),
        IpcMessage::UnlockVault { password } => handle_unlock_vault(state, &password),
        IpcMessage::LockVault => handle_lock_vault(state),
        IpcMessage::GetAllCredentials => handle_get_all_credentials(state),
        IpcMessage::SaveCredential {
            url,
            username,
            password,
        } => handle_save_credential(state, &url, &username, &password),
        IpcMessage::DeleteCredential { id } => handle_delete_credential(state, id),

        // Focus mode commands
        IpcMessage::EnterFocusMode => handle_enter_focus_mode(state),
        IpcMessage::ExitFocusMode => handle_exit_focus_mode(state),
        IpcMessage::ToggleFocusMode => handle_toggle_focus_mode(state),
        IpcMessage::GetFocusModeStatus => handle_get_focus_mode_status(state),
        IpcMessage::GetFocusModeConfig => handle_get_focus_mode_config(state),
        IpcMessage::SaveFocusModeConfig { config } => handle_save_focus_mode_config(state, config),
        IpcMessage::FocusScrollProgress { progress } => {
            handle_focus_scroll_progress(state, progress)
        }
        IpcMessage::FocusMediaPlaying { playing } => handle_focus_media_playing(state, playing),
        IpcMessage::FocusPageLoaded { url } => handle_focus_page_loaded(state, &url),
        IpcMessage::ShowFocusPeek => handle_show_focus_peek(state),
        IpcMessage::HideFocusPeek => handle_hide_focus_peek(state),
        IpcMessage::AddToFocusBlocklist { domain } => handle_add_to_focus_blocklist(state, &domain),
        IpcMessage::RemoveFromFocusBlocklist { domain } => {
            handle_remove_from_focus_blocklist(state, &domain)
        }

        // Mode commands
        IpcMessage::SetMode { mode } => handle_set_mode(state, &mode),

        // Settings
        IpcMessage::OpenSettings => handle_open_settings(state),
        IpcMessage::ChromeReady => IpcResponse::success(serde_json::json!({ "handled": true })),
        IpcMessage::CloseSettings => IpcResponse::success(serde_json::json!({ "closed": true })),

        // Misc commands
        IpcMessage::GetConfig => handle_get_config(state),
        IpcMessage::Log { level, message } => handle_log(&level, &message),
        IpcMessage::OpenExternal { url } => handle_open_external(&url),
        IpcMessage::GetDownloads => handle_get_downloads(state),
        IpcMessage::ClearDownloads => handle_clear_downloads(state),
        IpcMessage::OpenDownload { path } => handle_open_download(state, &path),
        IpcMessage::ShowDownloadInFolder { path } => handle_show_download_in_folder(state, &path),
        IpcMessage::GetVisitHistory => handle_get_visit_history(state),
        IpcMessage::ClearVisitHistory => handle_clear_visit_history(state),
        IpcMessage::FindInPage { .. } => {
            IpcResponse::success(serde_json::json!({ "handled": true }))
        }
        IpcMessage::FindInPageResult { .. } => {
            IpcResponse::success(serde_json::json!({ "handled": true }))
        }
        IpcMessage::SetSidebarOpen { .. } => {
            IpcResponse::success(serde_json::json!({ "handled": true }))
        }
        IpcMessage::SetRightPanelOpen { .. } => {
            IpcResponse::success(serde_json::json!({ "handled": true }))
        }
        IpcMessage::SaveSidebarWidth { width } => handle_save_sidebar_width(state, width),
        IpcMessage::GetSidebarWidth => handle_get_sidebar_width(state),
        // SetSidebarWidth is handled in main.rs for real-time layout updates
        IpcMessage::SetSidebarWidth { .. } => {
            IpcResponse::success(serde_json::json!({ "handled": true }))
        }
        IpcMessage::StartWindowDrag
        | IpcMessage::WindowMinimize
        | IpcMessage::WindowToggleMaximize
        | IpcMessage::WindowClose => IpcResponse::success(serde_json::json!({ "handled": true })),

        // Dynamic resize commands (handled in main.rs IPC handlers)
        IpcMessage::ExpandChrome
        | IpcMessage::ExpandChromeSmall
        | IpcMessage::CollapseChrome
        | IpcMessage::ExpandShelf
        | IpcMessage::CollapseShelf => IpcResponse::success(serde_json::json!({ "handled": true })),

        // Browser import
        IpcMessage::GetBrowserProfiles { browser } => handle_get_browser_profiles(&browser),
        IpcMessage::ImportBookmarks {
            browser,
            profile_path,
        } => handle_import_bookmarks(state, &browser, &profile_path),

        // Settings management
        IpcMessage::GetSettings => handle_get_settings(state),
        IpcMessage::SaveSettings { settings } => handle_save_settings(state, settings),

        // Export/Import
        IpcMessage::ExportData {
            include_settings,
            include_workspaces,
        } => handle_export_data(state, include_settings, include_workspaces),
        IpcMessage::ImportData { data, replace } => handle_import_data(state, &data, replace),

        // Data cleanup
        IpcMessage::ClearBrowsingData {
            history,
            downloads,
            shelf,
        } => handle_clear_browsing_data(state, history, downloads, shelf),

        // Cellar commands
        IpcMessage::GetCellar => handle_get_cellar(state),
        IpcMessage::RestoreFromCellar { id } => handle_restore_from_cellar(state, &id),
        IpcMessage::ClearCellar => handle_clear_cellar(state),

        // Analytics & Reports commands
        IpcMessage::GetTodayStats => handle_get_today_stats(state),
        IpcMessage::GetWeeklyReport => handle_get_weekly_report(state),
        IpcMessage::GetMonthlyReport => handle_get_monthly_report(state),
        IpcMessage::GetCustomReport {
            start_date,
            end_date,
        } => handle_get_custom_report(state, &start_date, &end_date),
        IpcMessage::GetTopDomains { limit } => handle_get_top_domains(state, limit.unwrap_or(10)),
        IpcMessage::GetWorkspaceStats => handle_get_workspace_stats(state),
        IpcMessage::GetAnalyticsSettings => handle_get_analytics_settings(state),
        IpcMessage::UpdateAnalyticsSettings {
            enabled,
            retention_days,
            weekly_report,
            report_day,
        } => handle_update_analytics_settings(
            state,
            enabled,
            retention_days,
            weekly_report,
            &report_day,
        ),
        IpcMessage::ClearAnalyticsData => handle_clear_analytics_data(state),
        IpcMessage::ExportAnalyticsData { format } => handle_export_analytics_data(state, &format),
        IpcMessage::ResetAnalyticsData => handle_reset_analytics_data(state),

        // File dialog commands (handled via main.rs event loop, not here)
        IpcMessage::SaveExportToFile { .. } | IpcMessage::PickImportFile => {
            IpcResponse::success(serde_json::json!({}))
        }

        // Print (handled via main.rs - triggers window.print() in content WebView)
        IpcMessage::PrintPage => IpcResponse::success(serde_json::json!({ "triggered": true })),

        // Zoom controls (handled via main.rs - triggers zoom on content WebView)
        IpcMessage::ZoomIn => IpcResponse::success(serde_json::json!({ "action": "zoom_in" })),
        IpcMessage::ZoomOut => IpcResponse::success(serde_json::json!({ "action": "zoom_out" })),
        IpcMessage::ResetZoom => {
            IpcResponse::success(serde_json::json!({ "action": "reset_zoom" }))
        }

        // Tab audio state (handled via main.rs)
        IpcMessage::TabAudioStateChanged { .. } => {
            IpcResponse::success(serde_json::json!({ "action": "audio_state_changed" }))
        }
        IpcMessage::ToggleTabMute { .. } => {
            IpcResponse::success(serde_json::json!({ "action": "toggle_tab_mute" }))
        }

        // Autofill (handled via main.rs - needs to interact with content webview)
        IpcMessage::TriggerAutofill => {
            // This is handled in main.rs via UserEvent
            IpcResponse::success(serde_json::json!({ "action": "trigger_autofill" }))
        }
        // Autofill hardening: nothing is returned on this route.
        IpcMessage::GetCredentialsForAutofill { .. } => {
            IpcResponse::error("Autofill is unavailable on this route")
        }

        // UI shortcuts (handled via main.rs UserEvents)
        IpcMessage::FocusAddressBar => {
            IpcResponse::success(serde_json::json!({ "action": "focus_address_bar" }))
        }
        IpcMessage::OpenFind => IpcResponse::success(serde_json::json!({ "action": "open_find" })),
        IpcMessage::ToggleSidebar => {
            IpcResponse::success(serde_json::json!({ "action": "toggle_sidebar" }))
        }
        IpcMessage::Refresh => IpcResponse::success(serde_json::json!({ "action": "refresh" })),
        IpcMessage::ActivateTabByIndex { .. } => {
            IpcResponse::success(serde_json::json!({ "action": "activate_tab_by_index" }))
        }
    }
}

// Navigation handlers

fn handle_navigate(_state: &Arc<Mutex<AppState>>, url: &str) -> IpcResponse {
    info!("Navigate to: {}", url);
    // Navigation will be handled by the WebView directly
    IpcResponse::success(serde_json::json!({ "navigated": true, "url": url }))
}

fn handle_go_back(_state: &Arc<Mutex<AppState>>) -> IpcResponse {
    info!("Go back");
    IpcResponse::success(serde_json::json!({ "action": "go_back" }))
}

fn handle_go_forward(_state: &Arc<Mutex<AppState>>) -> IpcResponse {
    info!("Go forward");
    IpcResponse::success(serde_json::json!({ "action": "go_forward" }))
}

fn handle_reload(_state: &Arc<Mutex<AppState>>) -> IpcResponse {
    info!("Reload");
    IpcResponse::success(serde_json::json!({ "action": "reload" }))
}

fn handle_stop(_state: &Arc<Mutex<AppState>>) -> IpcResponse {
    info!("Stop");
    IpcResponse::success(serde_json::json!({ "action": "stop" }))
}

// Tab handlers

fn handle_create_tab(state: &Arc<Mutex<AppState>>, url: Option<&str>) -> IpcResponse {
    let mut state = state.lock().unwrap();

    // Get active workspace
    let workspace_id = match state.shell.get_active_workspace() {
        Some(ws) => ws.id,
        None => return IpcResponse::error("No active workspace"),
    };

    // Parse URL or use new tab URL
    let url_str = url.unwrap_or("hiwave://newtab");
    let parsed_url = match Url::parse(url_str) {
        Ok(u) => u,
        Err(_) => {
            // Try adding https://
            match Url::parse(&format!("https://{}", url_str)) {
                Ok(u) => u,
                Err(e) => return IpcResponse::error(format!("Invalid URL: {}", e)),
            }
        }
    };

    // Create TabInfo
    let tab_info = hiwave_core::types::TabInfo {
        id: TabId::new(),
        url: parsed_url,
        title: None,
        favicon: None,
        workspace_id,
        suspended: false,
        loading: true,
        locked: false,
        last_visited: None,
    };

    let tab_id = tab_info.id;

    match state.shell.create_tab(tab_info) {
        Ok(_) => {
            let _ = state.shell.set_active_tab(tab_id);
            info!("Created tab: {:?}", tab_id);

            // Track tab creation
            let workspace_id_str = workspace_id.0.to_string();
            if let Err(e) = state.analytics.track_tab_opened(Some(&workspace_id_str)) {
                warn!("Failed to track tab creation: {}", e);
            }

            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({ "tab_id": tab_id.0.to_string() }))
        }
        Err(e) => {
            error!("Failed to create tab: {}", e);
            IpcResponse::error(format!("Failed to create tab: {}", e))
        }
    }
}

fn handle_close_tab(state: &Arc<Mutex<AppState>>, id: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();

    let tab_id = match id.parse::<u64>() {
        Ok(n) => TabId(n),
        Err(_) => return IpcResponse::error("Invalid tab ID"),
    };

    // Get workspace before closing the tab
    let workspace_id = state
        .shell
        .get_tab(tab_id)
        .map(|tab| tab.workspace_id.0.to_string());

    match state.shell.close_tab(tab_id) {
        Ok(_) => {
            info!("Closed tab: {:?}", tab_id);

            // Track tab closure (duration tracking would require created_at field)
            if let Err(e) = state.analytics.track_tab_closed(0, workspace_id.as_deref()) {
                warn!("Failed to track tab closure: {}", e);
            }

            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({ "closed": true }))
        }
        Err(e) => IpcResponse::error(format!("Failed to close tab: {}", e)),
    }
}

fn handle_activate_tab(state: &Arc<Mutex<AppState>>, id: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();

    let tab_id = match id.parse::<u64>() {
        Ok(n) => TabId(n),
        Err(_) => return IpcResponse::error("Invalid tab ID"),
    };

    match state.shell.set_active_tab(tab_id) {
        Ok(Some(tab)) => {
            let url = tab.url.to_string();
            info!("Activated tab: {:?}, navigating to {}", tab_id, url);
            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({
                "activated": true,
                "url": url,
                "tab_id": id
            }))
        }
        Ok(None) => IpcResponse::error("Tab not found"),
        Err(e) => IpcResponse::error(format!("Failed to activate tab: {}", e)),
    }
}

fn handle_get_tabs(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();

    // Get active workspace and its active tab
    let (workspace_id, active_tab_id) = state
        .shell
        .get_active_workspace()
        .map(|ws| (Some(ws.id), ws.active_tab))
        .unwrap_or((None, None));

    let tabs: Vec<TabInfo> = state
        .shell
        .list_tabs(workspace_id)
        .iter()
        .map(|tab| {
            let tab_id_str = tab.id.0.to_string();
            TabInfo {
                id: tab_id_str.clone(),
                url: tab.url.to_string(),
                title: tab.title.clone().unwrap_or_else(|| "New Tab".to_string()),
                is_active: active_tab_id == Some(tab.id),
                is_loading: tab.loading,
                favicon: tab.favicon.as_ref().map(|u| u.to_string()),
                locked: tab.locked,
                last_visited: tab.last_visited,
                is_playing_audio: state.is_tab_playing_audio(&tab_id_str),
                is_muted: state.is_tab_muted(&tab_id_str),
            }
        })
        .collect();
    IpcResponse::success(tabs)
}

fn handle_get_active_tab(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();

    // Get active tab from active workspace
    if let Some(ws) = state.shell.get_active_workspace() {
        if let Some(active_tab_id) = ws.active_tab {
            if let Some(tab) = state.shell.get_tab(active_tab_id) {
                let tab_id_str = tab.id.0.to_string();
                return IpcResponse::success(TabInfo {
                    id: tab_id_str.clone(),
                    url: tab.url.to_string(),
                    title: tab.title.clone().unwrap_or_else(|| "New Tab".to_string()),
                    is_active: true,
                    is_loading: tab.loading,
                    favicon: tab.favicon.as_ref().map(|u| u.to_string()),
                    locked: tab.locked,
                    last_visited: tab.last_visited,
                    is_playing_audio: state.is_tab_playing_audio(&tab_id_str),
                    is_muted: state.is_tab_muted(&tab_id_str),
                });
            }
        }
    }

    IpcResponse::success(serde_json::Value::Null)
}

fn handle_update_active_tab_url(state: &Arc<Mutex<AppState>>, url: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();

    // Get active tab and update its URL
    if let Some(active_tab) = state.shell.get_active_tab() {
        let tab_id = active_tab.id;
        // Parse and update the URL
        if let Ok(parsed_url) = Url::parse(url) {
            if let Err(e) = state.shell.update_tab_url(tab_id, parsed_url) {
                return IpcResponse::error(format!("Failed to update tab URL: {}", e));
            }
            info!("Updated active tab URL to: {}", url);
            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            return IpcResponse::success(serde_json::json!({ "updated": true, "url": url }));
        } else {
            return IpcResponse::error("Invalid URL");
        }
    }

    IpcResponse::error("No active tab")
}

// Workspace handlers

fn handle_create_workspace(state: &Arc<Mutex<AppState>>, name: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();
    let workspace_id = state.shell.create_workspace(name.to_string());
    state.ensure_workspace_shelf(name);
    info!("Created workspace: {:?}", workspace_id);
    if let Err(e) = state.save_workspace_state() {
        warn!("Failed to persist workspace state: {}", e);
    }
    IpcResponse::success(serde_json::json!({ "workspace_id": workspace_id.0.to_string() }))
}

fn handle_delete_workspace(state: &Arc<Mutex<AppState>>, id: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();

    let workspace_id = match id.parse::<u64>() {
        Ok(n) => WorkspaceId(n),
        Err(_) => return IpcResponse::error("Invalid workspace ID"),
    };

    let workspace_name = state
        .shell
        .get_workspace(workspace_id)
        .map(|ws| ws.name.clone());

    match state.shell.delete_workspace(workspace_id) {
        Ok(_) => {
            info!("Deleted workspace: {:?}", workspace_id);
            if let Some(name) = workspace_name {
                state.remove_workspace_shelf(&name);
            }
            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({ "deleted": true }))
        }
        Err(e) => IpcResponse::error(format!("Failed to delete workspace: {}", e)),
    }
}

fn handle_rename_workspace(state: &Arc<Mutex<AppState>>, id: &str, new_name: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();

    let workspace_id = match id.parse::<u64>() {
        Ok(n) => WorkspaceId(n),
        Err(_) => return IpcResponse::error("Invalid workspace ID"),
    };

    // Get old name for shelf migration
    let old_name = state
        .shell
        .get_workspace(workspace_id)
        .map(|ws| ws.name.clone());

    match state
        .shell
        .rename_workspace(workspace_id, new_name.to_string())
    {
        Ok(_) => {
            info!("Renamed workspace {:?} to '{}'", workspace_id, new_name);

            // Migrate shelf items to new workspace name
            if let Some(old) = old_name {
                if let Some(items) = state.shelves.remove(&old) {
                    state.shelves.insert(new_name.to_string(), items);
                }
            }

            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({
                "renamed": true,
                "id": id,
                "name": new_name
            }))
        }
        Err(e) => IpcResponse::error(format!("Failed to rename workspace: {}", e)),
    }
}

fn handle_activate_workspace(state: &Arc<Mutex<AppState>>, id: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();

    let workspace_id = match id.parse::<u64>() {
        Ok(n) => WorkspaceId(n),
        Err(_) => return IpcResponse::error("Invalid workspace ID"),
    };

    // Get current workspace before switching
    let from_workspace = state
        .shell
        .get_active_workspace()
        .map(|ws| ws.id.0.to_string())
        .unwrap_or_else(|| "none".to_string());

    match state.shell.set_active_workspace(workspace_id) {
        Ok(_) => {
            info!("Activated workspace: {:?}", workspace_id);

            // Track workspace switch
            let to_workspace = workspace_id.0.to_string();
            if from_workspace != to_workspace {
                if let Err(e) = state
                    .analytics
                    .track_workspace_switch(&from_workspace, &to_workspace)
                {
                    warn!("Failed to track workspace switch: {}", e);
                }
            }

            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({ "activated": true }))
        }
        Err(e) => IpcResponse::error(format!("Failed to activate workspace: {}", e)),
    }
}

fn handle_get_workspaces(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();

    let active_ws_id = state.shell.get_active_workspace().map(|ws| ws.id);

    let workspaces: Vec<WorkspaceInfo> = state
        .shell
        .list_workspaces()
        .iter()
        .map(|ws| {
            let tabs = state
                .shell
                .list_tabs(Some(ws.id))
                .iter()
                .map(|tab| WorkspaceTabSummary {
                    id: tab.id.0.to_string(),
                    title: tab.title.clone().unwrap_or_else(|| "New Tab".to_string()),
                    locked: tab.locked,
                    is_active: ws.active_tab == Some(tab.id),
                    url: tab.url.to_string(),
                })
                .collect();

            WorkspaceInfo {
                id: ws.id.0.to_string(),
                name: ws.name.clone(),
                tab_count: ws.tabs.len(),
                is_active: Some(ws.id) == active_ws_id,
                is_suspended: ws.suspended,
                locked_count: state.workspace_locked_count(ws.id),
                tabs,
            }
        })
        .collect();
    IpcResponse::success(workspaces)
}

fn handle_get_active_workspace(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    if let Some(ws) = state.shell.get_active_workspace() {
        let tabs = state
            .shell
            .list_tabs(Some(ws.id))
            .iter()
            .map(|tab| WorkspaceTabSummary {
                id: tab.id.0.to_string(),
                title: tab.title.clone().unwrap_or_else(|| "New Tab".to_string()),
                locked: tab.locked,
                is_active: ws.active_tab == Some(tab.id),
                url: tab.url.to_string(),
            })
            .collect();
        IpcResponse::success(WorkspaceInfo {
            id: ws.id.0.to_string(),
            name: ws.name.clone(),
            tab_count: ws.tabs.len(),
            is_active: true,
            is_suspended: ws.suspended,
            locked_count: state.workspace_locked_count(ws.id),
            tabs,
        })
    } else {
        IpcResponse::success(serde_json::Value::Null)
    }
}

// Shelf handlers

fn handle_get_shelf(state: &Arc<Mutex<AppState>>, scope: Option<&str>) -> IpcResponse {
    let state = state.lock().unwrap();
    let scope = scope.unwrap_or("workspace");
    let (items, workspace_name) = if scope == "all" {
        (state.shelf_items_all(), None)
    } else {
        let workspace_name = state.shell.get_active_workspace().map(|ws| ws.name.clone());
        (state.shelf_items_for_active_workspace(), workspace_name)
    };

    let payload = serde_json::json!({
        "scope": scope,
        "workspace": workspace_name,
        "items": items,
        "global_count": state.shelf_count_all(),
    });
    IpcResponse::success(payload)
}

fn handle_add_to_shelf(state: &Arc<Mutex<AppState>>, tab_id: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();
    let tab_id = match tab_id.parse::<u64>() {
        Ok(n) => TabId(n),
        Err(_) => return IpcResponse::error("Invalid tab ID"),
    };

    match state.add_to_shelf(tab_id) {
        Ok(item) => {
            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({ "shelved": item }))
        }
        Err(e) => IpcResponse::error(format!("Failed to shelf tab: {}", e)),
    }
}

fn handle_restore_from_shelf(
    state: &Arc<Mutex<AppState>>,
    id: &str,
    workspace_id: Option<&str>,
) -> IpcResponse {
    let mut state = state.lock().unwrap();
    let target_workspace = workspace_id.and_then(|value| match value.parse::<u64>() {
        Ok(n) => Some(WorkspaceId(n)),
        Err(_) => None,
    });
    match state.restore_from_shelf(id, target_workspace) {
        Ok(restored) => {
            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({ "restored": restored }))
        }
        Err(e) => IpcResponse::error(format!("Failed to restore shelf item: {}", e)),
    }
}

fn handle_delete_from_shelf(state: &Arc<Mutex<AppState>>, id: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();
    let removed = state.delete_from_shelf(id);
    if removed {
        if let Err(e) = state.save_workspace_state() {
            warn!("Failed to persist workspace state: {}", e);
        }
    }
    IpcResponse::success(serde_json::json!({ "deleted": removed }))
}

fn handle_move_tab_to_workspace(
    state: &Arc<Mutex<AppState>>,
    tab_id: &str,
    workspace_id: &str,
) -> IpcResponse {
    let mut state = state.lock().unwrap();
    let tab_id = match tab_id.parse::<u64>() {
        Ok(n) => TabId(n),
        Err(_) => return IpcResponse::error("Invalid tab ID"),
    };
    let workspace_id = match workspace_id.parse::<u64>() {
        Ok(n) => WorkspaceId(n),
        Err(_) => return IpcResponse::error("Invalid workspace ID"),
    };

    match state.shell.move_tab_to_workspace(tab_id, workspace_id) {
        Ok(_) => {
            info!("Moved tab {:?} to workspace {:?}", tab_id, workspace_id);
            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({ "moved": true }))
        }
        Err(e) => IpcResponse::error(format!("Failed to move tab: {}", e)),
    }
}

fn handle_lock_page(state: &Arc<Mutex<AppState>>, tab_id: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();
    let tab_id = match tab_id.parse::<u64>() {
        Ok(n) => TabId(n),
        Err(_) => return IpcResponse::error("Invalid tab ID"),
    };

    match state.lock_tab(tab_id) {
        Ok(_) => {
            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({ "locked": true }))
        }
        Err(e) => IpcResponse::error(format!("Failed to lock tab: {}", e)),
    }
}

fn handle_unlock_page(state: &Arc<Mutex<AppState>>, tab_id: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();
    let tab_id = match tab_id.parse::<u64>() {
        Ok(n) => TabId(n),
        Err(_) => return IpcResponse::error("Invalid tab ID"),
    };

    match state.unlock_tab(tab_id) {
        Ok(_) => {
            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({ "locked": false }))
        }
        Err(e) => IpcResponse::error(format!("Failed to unlock tab: {}", e)),
    }
}

fn handle_get_stale_locks(
    state: &Arc<Mutex<AppState>>,
    threshold_days: Option<u64>,
) -> IpcResponse {
    let state = state.lock().unwrap();
    let days = threshold_days.unwrap_or(21);
    let locks = state.get_stale_locks(days);
    let payload: Vec<serde_json::Value> = locks
        .into_iter()
        .map(|tab| {
            serde_json::json!({
                "tab_id": tab.id.0,
                "title": tab.title.clone().unwrap_or_else(|| "New Tab".to_string()),
                "url": tab.url.to_string(),
                "workspace_id": tab.workspace_id.0,
                "last_visited": tab.last_visited
            })
        })
        .collect();
    IpcResponse::success(serde_json::json!({ "locks": payload, "threshold_days": days }))
}

// Command palette handlers

fn handle_search_commands(state: &Arc<Mutex<AppState>>, query: &str) -> IpcResponse {
    let state = state.lock().unwrap();
    let results = state.shell.search_commands(query);
    let commands: Vec<CommandItem> = results
        .iter()
        .enumerate()
        .map(|(i, cmd)| CommandItem {
            id: cmd.id.clone(),
            label: cmd.name.clone(),
            category: format!("{:?}", cmd.category),
            shortcut: cmd.shortcut.clone(),
            score: (100 - i) as i64, // Approximate score based on ranking
        })
        .collect();
    IpcResponse::success(commands)
}

fn handle_execute_command(state: &Arc<Mutex<AppState>>, id: &str) -> IpcResponse {
    info!("Execute command: {}", id);

    // Return action to be performed by main event loop
    let action = match id {
        "new_tab" => Some(serde_json::json!({
            "action": "navigate",
            "url": "about:blank"
        })),
        "close_tab" => {
            // Close the active tab
            let mut state = state.lock().unwrap();
            if let Some(tab) = state.shell.get_active_tab() {
                let tab_id = tab.id;
                let _ = state.shell.close_tab(tab_id);
                if let Err(e) = state.save_workspace_state() {
                    warn!("Failed to persist workspace state: {}", e);
                }
                Some(serde_json::json!({
                    "action": "tab_closed",
                    "tab_id": tab_id.0.to_string()
                }))
            } else {
                None
            }
        }
        "new_workspace" => {
            let mut state = state.lock().unwrap();
            let ws_id = state.shell.create_workspace("New Workspace".to_string());
            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            Some(serde_json::json!({
                "action": "workspace_created",
                "workspace_id": ws_id.0.to_string()
            }))
        }
        "focus_mode" => Some(serde_json::json!({
            "action": "toggle_focus_mode"
        })),
        "reload" => Some(serde_json::json!({
            "action": "reload"
        })),
        "go_back" => Some(serde_json::json!({
            "action": "go_back"
        })),
        "go_forward" => Some(serde_json::json!({
            "action": "go_forward"
        })),
        "toggle_shield" => {
            let mut state = state.lock().unwrap();
            let new_state = !state.shield.is_enabled();
            state.shield.set_enabled(new_state);
            Some(serde_json::json!({
                "action": "shield_toggled",
                "enabled": new_state
            }))
        }
        "open_vault" => Some(serde_json::json!({
            "action": "open_vault"
        })),
        _ => None,
    };

    if let Some(action) = action {
        IpcResponse::success(serde_json::json!({
            "executed": true,
            "command_id": id,
            "result": action
        }))
    } else {
        IpcResponse::error(format!("Unknown command: {}", id))
    }
}

fn handle_open_command_palette(_state: &Arc<Mutex<AppState>>) -> IpcResponse {
    info!("Open command palette");
    IpcResponse::success(serde_json::json!({ "opened": true }))
}

fn handle_close_command_palette(_state: &Arc<Mutex<AppState>>) -> IpcResponse {
    info!("Close command palette");
    IpcResponse::success(serde_json::json!({ "closed": true }))
}

fn handle_set_mode(_state: &Arc<Mutex<AppState>>, mode: &str) -> IpcResponse {
    info!("Mode changed to '{}'", mode);
    IpcResponse::success(serde_json::json!({ "mode": mode }))
}

fn handle_open_settings(_state: &Arc<Mutex<AppState>>) -> IpcResponse {
    info!("Settings requested");
    IpcResponse::success(serde_json::json!({ "opened": true }))
}

// Shield handlers

fn handle_get_shield_stats(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    let stats = state.shield.get_stats();
    IpcResponse::success(ShieldStats {
        enabled: state.shield.is_enabled(),
        requests_blocked: stats.requests_blocked,
        trackers_blocked: stats.trackers_blocked,
    })
}

fn handle_toggle_shield(state: &Arc<Mutex<AppState>>, enabled: bool) -> IpcResponse {
    let mut state = state.lock().unwrap();
    state.shield.set_enabled(enabled);
    info!("Shield toggled: {}", enabled);
    IpcResponse::success(serde_json::json!({ "enabled": enabled }))
}

fn handle_block_and_close(state: &Arc<Mutex<AppState>>, id: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();

    let tab_id = match id.parse::<u64>() {
        Ok(n) => TabId(n),
        Err(_) => return IpcResponse::error("Invalid tab ID"),
    };

    // Get the tab's URL before closing
    let tab_url = match state.shell.get_tab(tab_id) {
        Some(tab) => tab.url.to_string(),
        None => return IpcResponse::error("Tab not found"),
    };

    // Extract domain from URL
    let domain = match crate::state::UserBlocklist::extract_domain(&tab_url) {
        Some(d) => d,
        None => return IpcResponse::error("Could not extract domain from URL"),
    };

    // Add domain to blocklist
    match state.block_domain(domain.clone()) {
        Ok(_) => {}
        Err(e) => {
            warn!("Failed to save blocklist: {}", e);
        }
    }

    // Close the tab
    match state.shell.close_tab(tab_id) {
        Ok(_) => {
            info!("Blocked domain {} and closed tab {:?}", domain, tab_id);
            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            IpcResponse::success(serde_json::json!({
                "blocked": true,
                "domain": domain,
                "closed": true
            }))
        }
        Err(e) => IpcResponse::error(format!("Failed to close tab: {}", e)),
    }
}

fn handle_get_blocklist(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    let domains: Vec<&String> = state.user_blocklist.domains.iter().collect();
    IpcResponse::success(serde_json::json!({
        "domains": domains,
        "count": domains.len()
    }))
}

fn handle_unblock_domain(state: &Arc<Mutex<AppState>>, domain: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();
    match state.unblock_domain(domain) {
        Ok(removed) => {
            if removed {
                info!("Unblocked domain: {}", domain);
                IpcResponse::success(serde_json::json!({ "unblocked": true, "domain": domain }))
            } else {
                IpcResponse::success(
                    serde_json::json!({ "unblocked": false, "domain": domain, "reason": "not in blocklist" }),
                )
            }
        }
        Err(e) => IpcResponse::error(format!("Failed to unblock domain: {}", e)),
    }
}

// Vault handlers

fn handle_get_vault_status(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    let unlocked = state.vault.is_unlocked();
    IpcResponse::success(serde_json::json!({ "unlocked": unlocked }))
}

fn handle_unlock_vault(state: &Arc<Mutex<AppState>>, password: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();
    match state.vault.unlock(password) {
        Ok(_) => {
            info!("Vault unlocked");
            IpcResponse::success(serde_json::json!({ "unlocked": true }))
        }
        Err(e) => {
            warn!("Failed to unlock vault: {}", e);
            IpcResponse::error("Invalid password")
        }
    }
}

fn handle_lock_vault(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let mut state = state.lock().unwrap();
    state.vault.lock();
    info!("Vault locked");
    IpcResponse::success(serde_json::json!({ "locked": true }))
}

fn handle_save_credential(
    state: &Arc<Mutex<AppState>>,
    url_str: &str,
    username: &str,
    password: &str,
) -> IpcResponse {
    let mut state = state.lock().unwrap();

    let url = match Url::parse(url_str) {
        Ok(u) => u,
        Err(_) => match Url::parse(&format!("https://{}", url_str)) {
            Ok(u) => u,
            Err(e) => return IpcResponse::error(format!("Invalid URL: {}", e)),
        },
    };

    match state.vault.save_credential(&url, username, password) {
        Ok(id) => {
            info!("Credential saved for: {}", url);
            IpcResponse::success(serde_json::json!({ "saved": true, "id": id }))
        }
        Err(e) => {
            error!("Failed to save credential: {}", e);
            IpcResponse::error(format!("Failed to save credential: {}", e))
        }
    }
}

fn handle_get_all_credentials(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();

    match state.vault.get_all_credentials() {
        Ok(creds) => {
            let cred_list: Vec<serde_json::Value> = creds
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "url": c.url,
                        "username": c.username,
                        // Don't expose password in list view
                    })
                })
                .collect();
            IpcResponse::success(cred_list)
        }
        Err(e) => IpcResponse::error(format!("{}", e)),
    }
}

fn handle_delete_credential(state: &Arc<Mutex<AppState>>, id: i64) -> IpcResponse {
    let mut state = state.lock().unwrap();

    match state.vault.delete_credential(id) {
        Ok(_) => {
            info!("Credential {} deleted", id);
            IpcResponse::success(serde_json::json!({ "deleted": true }))
        }
        Err(e) => {
            error!("Failed to delete credential: {}", e);
            IpcResponse::error(format!("Failed to delete credential: {}", e))
        }
    }
}

// Focus mode handlers

fn handle_enter_focus_mode(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let mut state = state.lock().unwrap();
    state.focus_mode.enter();
    info!("Entered focus mode");

    // Track focus mode start
    if let Some(url_str) = &state.focus_mode.current_url {
        if let Ok(url) = url::Url::parse(url_str) {
            if let Some(domain) = url.host_str() {
                let workspace_id = state
                    .shell
                    .get_active_workspace()
                    .map(|w| w.id.0.to_string());
                if let Err(e) = state
                    .analytics
                    .track_focus_start(domain, workspace_id.as_deref())
                {
                    warn!("Failed to track focus mode start: {}", e);
                }
            }
        }
    }

    IpcResponse::success(serde_json::json!({ "active": true }))
}

fn handle_exit_focus_mode(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let mut state = state.lock().unwrap();

    // Calculate duration before exiting (which clears entered_at)
    let duration_secs = if let Some(entered_at) = state.focus_mode.entered_at {
        entered_at.elapsed().as_secs() as i64
    } else {
        0
    };

    state.focus_mode.exit();
    info!("Exited focus mode (duration: {}s)", duration_secs);

    // Track focus mode end
    if duration_secs > 0 {
        if let Some(url_str) = &state.focus_mode.current_url {
            if let Ok(url) = url::Url::parse(url_str) {
                if let Some(domain) = url.host_str() {
                    let workspace_id = state
                        .shell
                        .get_active_workspace()
                        .map(|w| w.id.0.to_string());
                    if let Err(e) = state.analytics.track_focus_end(
                        domain,
                        duration_secs,
                        workspace_id.as_deref(),
                    ) {
                        warn!("Failed to track focus mode end: {}", e);
                    }
                }
            }
        }
    }

    IpcResponse::success(serde_json::json!({ "active": false }))
}

fn handle_toggle_focus_mode(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let mut state = state.lock().unwrap();
    state.focus_mode.toggle();
    let active = state.focus_mode.active;
    info!("Toggled focus mode: {}", active);
    IpcResponse::success(serde_json::json!({ "active": active }))
}

fn handle_get_focus_mode_status(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    let duration = state.focus_mode.focus_duration_secs();

    IpcResponse::success(FocusModeStatus {
        active: state.focus_mode.active,
        remaining_seconds: duration.map(|d| d as u32),
        total_seconds: duration.map(|d| d as u32),
    })
}

fn handle_get_focus_mode_config(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    IpcResponse::success(
        serde_json::to_value(&state.focus_mode_config).unwrap_or(serde_json::Value::Null),
    )
}

fn handle_save_focus_mode_config(
    state: &Arc<Mutex<AppState>>,
    config_value: serde_json::Value,
) -> IpcResponse {
    let config: crate::state::FocusModeConfig = match serde_json::from_value(config_value) {
        Ok(c) => c,
        Err(e) => return IpcResponse::error(format!("Invalid focus mode config: {}", e)),
    };

    let mut state = state.lock().unwrap();
    match state.update_focus_config(config) {
        Ok(_) => {
            info!("Focus mode config saved");
            // Reset auto-trigger flag so new settings can take effect
            state.focus_mode.auto_triggered_for_page = false;
            IpcResponse::success(serde_json::json!({ "saved": true }))
        }
        Err(e) => IpcResponse::error(format!("Failed to save focus config: {}", e)),
    }
}

fn handle_focus_scroll_progress(state: &Arc<Mutex<AppState>>, progress: f32) -> IpcResponse {
    let mut state = state.lock().unwrap();
    state.focus_mode.update_scroll_progress(progress);
    state.focus_mode.record_scroll();
    IpcResponse::success(serde_json::json!({ "progress": progress }))
}

fn handle_focus_media_playing(state: &Arc<Mutex<AppState>>, playing: bool) -> IpcResponse {
    let mut state = state.lock().unwrap();
    state.focus_mode.set_media_playing(playing);
    IpcResponse::success(serde_json::json!({ "media_playing": playing }))
}

fn handle_focus_page_loaded(state: &Arc<Mutex<AppState>>, url: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();
    state.focus_mode.record_navigation(url);
    IpcResponse::success(serde_json::json!({ "recorded": true }))
}

fn handle_show_focus_peek(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let mut state = state.lock().unwrap();
    state.focus_mode.show_peek();
    IpcResponse::success(serde_json::json!({ "peek_visible": true }))
}

fn handle_hide_focus_peek(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let mut state = state.lock().unwrap();
    state.focus_mode.hide_peek();
    IpcResponse::success(serde_json::json!({ "peek_visible": false }))
}

fn handle_add_to_focus_blocklist(state: &Arc<Mutex<AppState>>, domain: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();
    let added = state.focus_mode_config.add_to_blocklist(domain.to_string());
    if added {
        if let Err(e) = state.save_focus_config() {
            warn!("Failed to save focus config: {}", e);
        }
    }
    IpcResponse::success(serde_json::json!({ "added": added, "domain": domain }))
}

fn handle_remove_from_focus_blocklist(state: &Arc<Mutex<AppState>>, domain: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();
    let removed = state.focus_mode_config.remove_from_blocklist(domain);
    if removed {
        if let Err(e) = state.save_focus_config() {
            warn!("Failed to save focus config: {}", e);
        }
    }
    IpcResponse::success(serde_json::json!({ "removed": removed, "domain": domain }))
}

// Misc handlers

fn handle_get_config(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    IpcResponse::success(&state.config)
}

fn handle_log(level: &str, message: &str) -> IpcResponse {
    match level.to_lowercase().as_str() {
        "debug" => debug!("[JS] {}", message),
        "info" => info!("[JS] {}", message),
        "warn" => warn!("[JS] {}", message),
        "error" => error!("[JS] {}", message),
        _ => info!("[JS] {}", message),
    }
    IpcResponse::success(serde_json::json!({ "logged": true }))
}

fn handle_open_external(url: &str) -> IpcResponse {
    info!("Opening in external browser: {}", url);

    match platform().open_external(url) {
        Ok(()) => IpcResponse::success(serde_json::json!({ "opened": true, "url": url })),
        Err(e) => IpcResponse::error(format!("Failed to open external browser: {}", e)),
    }
}

fn handle_get_downloads(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    let payload = serde_json::json!({
        "downloads": state.downloads_snapshot(),
        "active_count": state.active_download_count(),
    });
    IpcResponse::success(payload)
}

fn handle_clear_downloads(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let mut state = state.lock().unwrap();
    state.clear_download_history();
    if let Err(err) = state.save_workspace_state() {
        warn!("Failed to persist download history: {}", err);
    }
    IpcResponse::success(serde_json::json!({ "cleared": true }))
}

fn handle_open_download(_state: &Arc<Mutex<AppState>>, path: &str) -> IpcResponse {
    match open_path(Path::new(path)) {
        Ok(_) => IpcResponse::success(serde_json::json!({ "opened": true })),
        Err(e) => IpcResponse::error(e),
    }
}

fn handle_show_download_in_folder(_state: &Arc<Mutex<AppState>>, path: &str) -> IpcResponse {
    match show_path_in_folder(Path::new(path)) {
        Ok(_) => IpcResponse::success(serde_json::json!({ "opened": true })),
        Err(e) => IpcResponse::error(e),
    }
}

fn handle_get_visit_history(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    let history = state.visit_history_snapshot();
    IpcResponse::success(serde_json::json!({ "history": history }))
}

fn handle_clear_visit_history(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let mut state = state.lock().unwrap();
    match state.clear_visit_history() {
        Ok(_) => IpcResponse::success(serde_json::json!({ "cleared": true })),
        Err(err) => IpcResponse::error(format!("Failed to clear history: {}", err)),
    }
}

fn open_path(path: &Path) -> Result<(), String> {
    platform().open_file(path).map_err(|e| e.to_string())
}

fn show_path_in_folder(path: &Path) -> Result<(), String> {
    platform().show_in_folder(path).map_err(|e| e.to_string())
}

// Settings handlers

fn handle_get_settings(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    let settings = state.get_settings();
    IpcResponse::success(serde_json::to_value(&settings).unwrap_or(serde_json::Value::Null))
}

fn handle_save_settings(
    state: &Arc<Mutex<AppState>>,
    settings_value: serde_json::Value,
) -> IpcResponse {
    let settings: crate::state::UserSettings = match serde_json::from_value(settings_value) {
        Ok(s) => s,
        Err(e) => return IpcResponse::error(format!("Invalid settings format: {}", e)),
    };

    let mut state = state.lock().unwrap();
    match state.update_settings(settings) {
        Ok(_) => {
            info!("Settings saved successfully");
            IpcResponse::success(serde_json::json!({ "saved": true }))
        }
        Err(e) => IpcResponse::error(format!("Failed to save settings: {}", e)),
    }
}

// Sidebar width handlers

fn handle_save_sidebar_width(state: &Arc<Mutex<AppState>>, width: u32) -> IpcResponse {
    // Clamp width to valid range
    let width = width.clamp(48, 400);

    let mut state = state.lock().unwrap();
    let mut settings = state.get_settings().clone();
    settings.sidebar_width = width;

    match state.update_settings(settings) {
        Ok(_) => {
            info!("Sidebar width saved: {}px", width);
            IpcResponse::success(serde_json::json!({ "saved": true, "width": width }))
        }
        Err(e) => IpcResponse::error(format!("Failed to save sidebar width: {}", e)),
    }
}

fn handle_get_sidebar_width(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    let settings = state.get_settings();
    IpcResponse::success(serde_json::json!({ "width": settings.sidebar_width }))
}

// Export/Import handlers

fn handle_export_data(
    state: &Arc<Mutex<AppState>>,
    include_settings: bool,
    include_workspaces: bool,
) -> IpcResponse {
    let state = state.lock().unwrap();
    let export = state.export_data(include_settings, include_workspaces);

    match serde_json::to_string_pretty(&export) {
        Ok(json) => {
            info!(
                "Exported data: {} workspaces, settings: {}",
                export.workspaces.len(),
                include_settings
            );
            IpcResponse::success(serde_json::json!({
                "data": json,
                "workspaces_count": export.workspaces.len(),
                "includes_settings": include_settings,
            }))
        }
        Err(e) => IpcResponse::error(format!("Failed to serialize export: {}", e)),
    }
}

fn handle_import_data(state: &Arc<Mutex<AppState>>, data: &str, replace: bool) -> IpcResponse {
    // Parse the import data
    let import_data: crate::state::HiWaveExport = match serde_json::from_str(data) {
        Ok(d) => d,
        Err(e) => return IpcResponse::error(format!("Invalid import format: {}", e)),
    };

    let mut state = state.lock().unwrap();
    match state.import_data(&import_data, replace) {
        Ok(result) => {
            info!(
                "Import complete: {} workspaces, {} tabs, {} errors",
                result.workspaces_created,
                result.tabs_created,
                result.errors.len()
            );
            IpcResponse::success(serde_json::json!({
                "success": result.success,
                "workspaces_created": result.workspaces_created,
                "tabs_created": result.tabs_created,
                "errors": result.errors,
            }))
        }
        Err(e) => IpcResponse::error(format!("Import failed: {}", e)),
    }
}

// Data cleanup handlers

fn handle_clear_browsing_data(
    state: &Arc<Mutex<AppState>>,
    history: bool,
    downloads: bool,
    shelf: bool,
) -> IpcResponse {
    let mut state = state.lock().unwrap();
    let mut cleared = Vec::new();

    if history {
        if let Err(e) = state.clear_visit_history() {
            warn!("Failed to clear visit history: {}", e);
        } else {
            cleared.push("history");
        }
    }

    if downloads {
        state.clear_download_history();
        cleared.push("downloads");
    }

    if shelf {
        state.clear_shelf();
        cleared.push("shelf");
    }

    // Save state after clearing
    if let Err(e) = state.save_workspace_state() {
        warn!("Failed to save state after clearing data: {}", e);
    }

    info!("Cleared browsing data: {:?}", cleared);
    IpcResponse::success(serde_json::json!({
        "cleared": cleared,
    }))
}

// Browser import handlers

fn handle_get_browser_profiles(browser: &str) -> IpcResponse {
    let browser_type = match browser.to_lowercase().as_str() {
        "chrome" => Browser::Chrome,
        "firefox" => Browser::Firefox,
        "brave" => Browser::Brave,
        _ => return IpcResponse::error(format!("Unknown browser: {}", browser)),
    };

    let profiles = import::get_browser_profiles(browser_type);
    let profiles_json: Vec<serde_json::Value> = profiles
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "path": p.path.to_string_lossy(),
                "browser": browser,
            })
        })
        .collect();

    IpcResponse::success(serde_json::json!({
        "browser": browser,
        "profiles": profiles_json,
    }))
}

fn handle_import_bookmarks(
    state: &Arc<Mutex<AppState>>,
    browser: &str,
    profile_path: &str,
) -> IpcResponse {
    let browser_type = match browser.to_lowercase().as_str() {
        "chrome" => Browser::Chrome,
        "firefox" => Browser::Firefox,
        "brave" => Browser::Brave,
        _ => return IpcResponse::error(format!("Unknown browser: {}", browser)),
    };

    let profile = import::BrowserProfile {
        name: "Import".to_string(),
        path: std::path::PathBuf::from(profile_path),
        browser: browser_type,
    };

    // Parse bookmarks from the browser profile
    let bookmarks = match import::import_bookmarks(&profile) {
        Ok(b) => b,
        Err(e) => return IpcResponse::error(format!("Failed to parse bookmarks: {}", e)),
    };

    if bookmarks.is_empty() {
        return IpcResponse::success(serde_json::json!({
            "success": true,
            "workspaces_created": 0,
            "tabs_created": 0,
            "message": "No bookmarks found to import",
        }));
    }

    // Convert bookmarks to workspaces
    let config = ConversionConfig::for_browser(browser_type);
    let result = import::converter::convert_to_workspaces(bookmarks, &config);

    // Add workspaces and tabs to the shell
    let mut state = state.lock().unwrap();
    let mut actual_workspaces_created = 0;
    let mut actual_tabs_created = 0;

    for import_ws in &result.workspaces {
        // Create the workspace in the shell
        let ws_id = state.shell.create_workspace(import_ws.name.clone());
        actual_workspaces_created += 1;

        // Add tabs to the workspace
        for tab in &import_ws.tabs {
            // Parse the URL
            let parsed_url =
                Url::parse(&tab.url).unwrap_or_else(|_| Url::parse("about:blank").unwrap());

            // Create the tab
            let tab_info = CoreTabInfo {
                id: TabId::new(),
                url: parsed_url,
                title: Some(tab.title.clone()),
                favicon: None,
                workspace_id: ws_id,
                suspended: false,
                loading: false,
                locked: false,
                last_visited: None,
            };

            if state.shell.create_tab(tab_info).is_ok() {
                actual_tabs_created += 1;
            }
        }
    }

    // Save state
    if let Err(e) = state.save_workspace_state() {
        warn!("Failed to save state after import: {}", e);
    }

    info!(
        "Import complete: {} workspaces, {} tabs",
        actual_workspaces_created, actual_tabs_created
    );

    IpcResponse::success(serde_json::json!({
        "success": true,
        "workspaces_created": actual_workspaces_created,
        "tabs_created": actual_tabs_created,
    }))
}

// Cellar handlers

fn handle_get_cellar(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    let cellar = state.list_cellar();

    let items: Vec<serde_json::Value> = cellar
        .iter()
        .map(|item| {
            serde_json::json!({
                "id": item.id,
                "url": item.url,
                "title": item.title,
                "workspace": item.workspace,
                "originally_shelved": item.originally_shelved,
                "cellared_at": item.cellared_at,
            })
        })
        .collect();

    IpcResponse::success(serde_json::json!({
        "items": items,
        "count": items.len(),
    }))
}

fn handle_restore_from_cellar(state: &Arc<Mutex<AppState>>, id: &str) -> IpcResponse {
    let mut state = state.lock().unwrap();

    match state.restore_from_cellar(id) {
        Some(shelf_item) => {
            if let Err(e) = state.save_workspace_state() {
                warn!("Failed to persist workspace state: {}", e);
            }
            info!("Restored item from cellar: {}", id);
            IpcResponse::success(serde_json::json!({
                "restored": true,
                "item": {
                    "id": shelf_item.id,
                    "url": shelf_item.url,
                    "title": shelf_item.title,
                    "workspace": shelf_item.workspace,
                }
            }))
        }
        None => IpcResponse::error(format!("Cellar item not found: {}", id)),
    }
}

fn handle_clear_cellar(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let mut state = state.lock().unwrap();
    state.clear_cellar();

    if let Err(e) = state.save_workspace_state() {
        warn!("Failed to persist workspace state: {}", e);
    }

    info!("Cleared cellar");
    IpcResponse::success(serde_json::json!({ "cleared": true }))
}

// Analytics handlers

fn handle_get_today_stats(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    match state.analytics.get_today_stats() {
        Ok(stats) => {
            // Get top domains for today
            let top_domains = state.analytics.get_top_domains(10).unwrap_or_default();

            // Get workspace stats for today
            let workspace_stats = state.analytics.get_workspace_stats().unwrap_or_default();

            IpcResponse::success(serde_json::json!({
                "date": stats.date,
                "trackers_blocked": stats.trackers_blocked,
                "ads_blocked": stats.ads_blocked,
                "popups_blocked": stats.popups_blocked,
                "pages_visited": stats.pages_visited,
                "tabs_opened": stats.tabs_opened,
                "tabs_closed": stats.tabs_closed,
                "browsing_time": stats.browsing_time,
                "focus_time": stats.focus_time,
                "workspace_switches": stats.workspace_switches,
                "time_saved": stats.time_saved,
                "bandwidth_saved": stats.bandwidth_saved,
                "top_domains": top_domains,
                "workspace_breakdown": workspace_stats,
            }))
        }
        Err(e) => IpcResponse::error(format!("Failed to get today's stats: {}", e)),
    }
}

fn handle_get_weekly_report(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    match state.analytics.generate_weekly_report() {
        Ok(report) => IpcResponse::success(serde_json::json!({
            "start_date": report.start_date,
            "end_date": report.end_date,
            "total_stats": {
                "trackers_blocked": report.total_stats.trackers_blocked,
                "ads_blocked": report.total_stats.ads_blocked,
                "popups_blocked": report.total_stats.popups_blocked,
                "pages_visited": report.total_stats.pages_visited,
                "tabs_opened": report.total_stats.tabs_opened,
                "tabs_closed": report.total_stats.tabs_closed,
                "browsing_time": report.total_stats.browsing_time,
                "focus_time": report.total_stats.focus_time,
                "workspace_switches": report.total_stats.workspace_switches,
                "time_saved": report.total_stats.time_saved,
                "bandwidth_saved": report.total_stats.bandwidth_saved,
            },
            "daily_breakdown": report.daily_breakdown,
            "top_domains": report.top_domains,
            "workspace_breakdown": report.workspace_breakdown,
        })),
        Err(e) => IpcResponse::error(format!("Failed to generate weekly report: {}", e)),
    }
}

fn handle_get_monthly_report(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    match state.analytics.generate_monthly_report() {
        Ok(report) => IpcResponse::success(serde_json::json!({
            "start_date": report.start_date,
            "end_date": report.end_date,
            "total_stats": {
                "trackers_blocked": report.total_stats.trackers_blocked,
                "ads_blocked": report.total_stats.ads_blocked,
                "popups_blocked": report.total_stats.popups_blocked,
                "pages_visited": report.total_stats.pages_visited,
                "tabs_opened": report.total_stats.tabs_opened,
                "tabs_closed": report.total_stats.tabs_closed,
                "browsing_time": report.total_stats.browsing_time,
                "focus_time": report.total_stats.focus_time,
                "workspace_switches": report.total_stats.workspace_switches,
                "time_saved": report.total_stats.time_saved,
                "bandwidth_saved": report.total_stats.bandwidth_saved,
            },
            "daily_breakdown": report.daily_breakdown,
            "top_domains": report.top_domains,
            "workspace_breakdown": report.workspace_breakdown,
        })),
        Err(e) => IpcResponse::error(format!("Failed to generate monthly report: {}", e)),
    }
}

fn handle_get_custom_report(
    state: &Arc<Mutex<AppState>>,
    start_date: &str,
    end_date: &str,
) -> IpcResponse {
    let state = state.lock().unwrap();
    match state.analytics.generate_report(start_date, end_date) {
        Ok(report) => IpcResponse::success(serde_json::json!({
            "start_date": report.start_date,
            "end_date": report.end_date,
            "total_stats": {
                "trackers_blocked": report.total_stats.trackers_blocked,
                "ads_blocked": report.total_stats.ads_blocked,
                "popups_blocked": report.total_stats.popups_blocked,
                "pages_visited": report.total_stats.pages_visited,
                "tabs_opened": report.total_stats.tabs_opened,
                "tabs_closed": report.total_stats.tabs_closed,
                "browsing_time": report.total_stats.browsing_time,
                "focus_time": report.total_stats.focus_time,
                "workspace_switches": report.total_stats.workspace_switches,
            },
            "daily_breakdown": report.daily_breakdown,
            "top_domains": report.top_domains,
            "workspace_breakdown": report.workspace_breakdown,
        })),
        Err(e) => IpcResponse::error(format!("Failed to generate report: {}", e)),
    }
}

fn handle_get_top_domains(state: &Arc<Mutex<AppState>>, limit: usize) -> IpcResponse {
    let state = state.lock().unwrap();
    match state.analytics.get_top_domains(limit) {
        Ok(domains) => IpcResponse::success(serde_json::json!({
            "domains": domains,
            "count": domains.len(),
        })),
        Err(e) => IpcResponse::error(format!("Failed to get top domains: {}", e)),
    }
}

fn handle_get_workspace_stats(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    match state.analytics.get_workspace_stats() {
        Ok(stats) => IpcResponse::success(serde_json::json!({
            "workspaces": stats,
            "count": stats.len(),
        })),
        Err(e) => IpcResponse::error(format!("Failed to get workspace stats: {}", e)),
    }
}

fn handle_get_analytics_settings(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    let settings = state.analytics.get_settings();

    IpcResponse::success(serde_json::json!({
        "enabled": settings.enabled,
        "retention_days": settings.retention_days,
        "weekly_report": settings.weekly_report,
        "report_day": settings.report_day,
    }))
}

fn handle_update_analytics_settings(
    state: &Arc<Mutex<AppState>>,
    enabled: bool,
    retention_days: i32,
    weekly_report: bool,
    report_day: &str,
) -> IpcResponse {
    use hiwave_analytics::ReportSettings;

    let new_settings = ReportSettings {
        enabled,
        retention_days,
        weekly_report,
        report_day: report_day.to_string(),
    };

    let state = state.lock().unwrap();
    match state.analytics.update_settings(new_settings) {
        Ok(_) => {
            info!("Analytics settings updated");
            IpcResponse::success(serde_json::json!({ "updated": true }))
        }
        Err(e) => IpcResponse::error(format!("Failed to update analytics settings: {}", e)),
    }
}

fn handle_clear_analytics_data(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();
    match state.analytics.clear_all_data() {
        Ok(_) => {
            info!("Analytics data cleared");
            IpcResponse::success(serde_json::json!({ "cleared": true }))
        }
        Err(e) => IpcResponse::error(format!("Failed to clear analytics data: {}", e)),
    }
}

fn handle_export_analytics_data(state: &Arc<Mutex<AppState>>, format: &str) -> IpcResponse {
    let state = state.lock().unwrap();

    match format.to_lowercase().as_str() {
        "json" => {
            // Generate a comprehensive export
            match state.analytics.generate_monthly_report() {
                Ok(report) => match serde_json::to_string_pretty(&report) {
                    Ok(json) => IpcResponse::success(serde_json::json!({
                        "format": "json",
                        "data": json,
                    })),
                    Err(e) => IpcResponse::error(format!("Failed to serialize JSON: {}", e)),
                },
                Err(e) => IpcResponse::error(format!("Failed to generate report: {}", e)),
            }
        }
        "csv" => {
            // Export daily stats as CSV
            match state.analytics.get_last_n_days_stats(30) {
                Ok(stats) => {
                    let mut csv = String::from("Date,Trackers Blocked,Ads Blocked,Popups Blocked,Pages Visited,Tabs Opened,Tabs Closed,Browsing Time (s),Focus Time (s),Workspace Switches\n");

                    for stat in stats {
                        csv.push_str(&format!(
                            "{},{},{},{},{},{},{},{},{},{}\n",
                            stat.date,
                            stat.trackers_blocked,
                            stat.ads_blocked,
                            stat.popups_blocked,
                            stat.pages_visited,
                            stat.tabs_opened,
                            stat.tabs_closed,
                            stat.browsing_time,
                            stat.focus_time,
                            stat.workspace_switches
                        ));
                    }

                    IpcResponse::success(serde_json::json!({
                        "format": "csv",
                        "data": csv,
                    }))
                }
                Err(e) => IpcResponse::error(format!("Failed to get stats: {}", e)),
            }
        }
        _ => IpcResponse::error(format!("Unsupported export format: {}", format)),
    }
}

fn handle_reset_analytics_data(state: &Arc<Mutex<AppState>>) -> IpcResponse {
    let state = state.lock().unwrap();

    // Archive current data to historical tables, then clear active tables
    match state.analytics.archive_data() {
        Ok(_) => {
            info!("Analytics data archived and reset");
            IpcResponse::success(serde_json::json!({
                "reset": true,
                "message": "Analytics data has been archived and reset. Historical data preserved."
            }))
        }
        Err(e) => IpcResponse::error(format!("Failed to archive analytics data: {}", e)),
    }
}
