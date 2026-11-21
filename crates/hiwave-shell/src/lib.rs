//! Browser UI shell - tabs, workspaces, command palette

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use hiwave_core::{
    types::{TabId, TabInfo, WorkspaceId, WorkspaceInfo},
    HiWaveResult,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct BrowserShell {
    tabs: HashMap<TabId, TabInfo>,
    /// Every open tab, in tab-strip order (M1c): the order they were
    /// opened, a new tab at the end, a tab moved to another workspace at
    /// the end of it. Filtered to one workspace it equals that
    /// workspace's `tabs`. Every reader of tab order reads this, never the
    /// map's order.
    tab_order: Vec<TabId>,
    workspaces: HashMap<WorkspaceId, WorkspaceInfo>,
    active_workspace: Option<WorkspaceId>,
    command_palette: CommandPalette,
}

impl BrowserShell {
    pub fn new() -> Self {
        log::info!("Initializing browser shell");

        let mut shell = Self {
            tabs: HashMap::new(),
            tab_order: Vec::new(),
            workspaces: HashMap::new(),
            active_workspace: None,
            command_palette: CommandPalette::new(),
        };

        // Create default workspace
        let default_ws = shell.create_workspace("Default".to_string());
        shell.set_active_workspace(default_ws).ok();

        shell
    }

    /// The open tabs in tab-strip order.
    fn ordered_tabs(&self) -> impl Iterator<Item = &TabInfo> {
        self.tab_order.iter().filter_map(|id| self.tabs.get(id))
    }

    fn current_timestamp_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    }

    // Workspace management
    pub fn create_workspace(&mut self, name: String) -> WorkspaceId {
        let id = WorkspaceId::new();

        let workspace = WorkspaceInfo {
            id,
            name,
            tabs: Vec::new(),
            active_tab: None,
            suspended: false,
        };

        log::info!("Created workspace {:?}: {}", id, workspace.name);
        self.workspaces.insert(id, workspace);
        id
    }

    pub fn delete_workspace(&mut self, id: WorkspaceId) -> HiWaveResult<()> {
        if self.workspaces.len() <= 1 {
            return Err(hiwave_core::HiWaveError::Config(
                "Cannot delete the last workspace".to_string(),
            ));
        }

        if let Some(workspace) = self.workspaces.remove(&id) {
            log::info!("Deleted workspace {:?}: {}", id, workspace.name);

            // Close all tabs in this workspace
            for tab_id in &workspace.tabs {
                self.tabs.remove(tab_id);
            }
            self.tab_order.retain(|tab_id| !workspace.tabs.contains(tab_id));

            // Switch to another workspace if this was active
            if self.active_workspace == Some(id) {
                if let Some(new_active) = self.workspaces.keys().min_by_key(|ws| ws.0).copied() {
                    self.set_active_workspace(new_active)?;
                }
            }
        }

        Ok(())
    }

    pub fn rename_workspace(&mut self, id: WorkspaceId, new_name: String) -> HiWaveResult<()> {
        if let Some(workspace) = self.workspaces.get_mut(&id) {
            log::info!(
                "Renaming workspace {:?} from '{}' to '{}'",
                id,
                workspace.name,
                new_name
            );
            workspace.name = new_name;
            Ok(())
        } else {
            Err(hiwave_core::HiWaveError::Config(
                "Workspace not found".to_string(),
            ))
        }
    }

    pub fn set_active_workspace(&mut self, id: WorkspaceId) -> HiWaveResult<()> {
        if !self.workspaces.contains_key(&id) {
            return Err(hiwave_core::HiWaveError::Config(
                "Workspace not found".to_string(),
            ));
        }

        log::info!("Switching to workspace {:?}", id);

        // Suspend previous workspace
        if let Some(prev_id) = self.active_workspace {
            if let Some(prev_ws) = self.workspaces.get_mut(&prev_id) {
                prev_ws.suspended = true;
            }
        }

        // Activate new workspace
        if let Some(workspace) = self.workspaces.get_mut(&id) {
            workspace.suspended = false;
        }

        self.active_workspace = Some(id);
        Ok(())
    }

    pub fn get_active_workspace(&self) -> Option<&WorkspaceInfo> {
        self.active_workspace
            .and_then(|id| self.workspaces.get(&id))
    }

    pub fn get_workspace(&self, id: WorkspaceId) -> Option<&WorkspaceInfo> {
        self.workspaces.get(&id)
    }

    /// Workspaces in creation order (ids are handed out in sequence).
    pub fn list_workspaces(&self) -> Vec<&WorkspaceInfo> {
        let mut workspaces: Vec<&WorkspaceInfo> = self.workspaces.values().collect();
        workspaces.sort_by_key(|ws| ws.id.0);
        workspaces
    }

    // Tab management
    pub fn create_tab(&mut self, tab: TabInfo) -> HiWaveResult<TabId> {
        let tab_id = tab.id;
        let workspace_id = tab.workspace_id;

        log::info!("Creating tab {:?} in workspace {:?}", tab_id, workspace_id);

        // A re-used id (no caller does this) replaces the old tab, which
        // leaves both lists before the new one joins at the end.
        if let Some(old) = self.tabs.insert(tab_id, tab) {
            if let Some(workspace) = self.workspaces.get_mut(&old.workspace_id) {
                workspace.tabs.retain(|&id| id != tab_id);
            }
            self.tab_order.retain(|&id| id != tab_id);
        }
        // A new tab goes at the end of the strip.
        self.tab_order.push(tab_id);

        // Add to workspace
        if let Some(workspace) = self.workspaces.get_mut(&workspace_id) {
            workspace.tabs.push(tab_id);
            if workspace.active_tab.is_none() {
                workspace.active_tab = Some(tab_id);
            }
        }

        Ok(tab_id)
    }

    pub fn close_tab(&mut self, tab_id: TabId) -> HiWaveResult<()> {
        if let Some(tab) = self.tabs.remove(&tab_id) {
            log::info!("Closing tab {:?}", tab_id);
            self.tab_order.retain(|&id| id != tab_id);

            // Remove from workspace
            if let Some(workspace) = self.workspaces.get_mut(&tab.workspace_id) {
                workspace.tabs.retain(|&id| id != tab_id);

                // Update active tab if necessary
                if workspace.active_tab == Some(tab_id) {
                    workspace.active_tab = workspace.tabs.first().copied();
                }
            }
        }

        Ok(())
    }

    pub fn set_active_tab(&mut self, tab_id: TabId) -> HiWaveResult<Option<&TabInfo>> {
        // Find which workspace this tab belongs to and update last_visited
        let tab = self
            .tabs
            .get_mut(&tab_id)
            .ok_or_else(|| hiwave_core::HiWaveError::Config("Tab not found".to_string()))?;

        let workspace_id = tab.workspace_id;

        // Touch last_visited when activating a tab
        tab.last_visited = Some(Self::current_timestamp_secs());

        // Update the workspace's active tab
        if let Some(workspace) = self.workspaces.get_mut(&workspace_id) {
            log::info!(
                "Setting active tab {:?} in workspace {:?}",
                tab_id,
                workspace_id
            );
            workspace.active_tab = Some(tab_id);
        }

        Ok(self.tabs.get(&tab_id))
    }

    pub fn lock_tab(&mut self, tab_id: TabId) -> HiWaveResult<()> {
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.locked = true;
            tab.last_visited = Some(Self::current_timestamp_secs());
            Ok(())
        } else {
            Err(hiwave_core::HiWaveError::Config(
                "Tab not found".to_string(),
            ))
        }
    }

    pub fn unlock_tab(&mut self, tab_id: TabId) -> HiWaveResult<()> {
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.locked = false;
            tab.last_visited = Some(Self::current_timestamp_secs());
            Ok(())
        } else {
            Err(hiwave_core::HiWaveError::Config(
                "Tab not found".to_string(),
            ))
        }
    }

    pub fn touch_tab(&mut self, tab_id: TabId) -> HiWaveResult<()> {
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.last_visited = Some(Self::current_timestamp_secs());
            Ok(())
        } else {
            Err(hiwave_core::HiWaveError::Config(
                "Tab not found".to_string(),
            ))
        }
    }

    pub fn stale_locks(&self, threshold_days: u64) -> Vec<TabInfo> {
        let cutoff_secs = threshold_days.saturating_mul(86400);
        let now = Self::current_timestamp_secs();
        self.ordered_tabs()
            .filter(|tab| tab.locked)
            .filter(|tab| {
                tab.last_visited
                    .map(|last| now.saturating_sub(last) >= cutoff_secs)
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
    }

    /// Calculate decay level for a tab (0 = fresh, 1-4 = progressively decayed, 5 = expired)
    /// Based on percentage of decay_days elapsed since last visit
    pub fn calculate_decay_level(last_visited: Option<u64>, decay_days: u32) -> u8 {
        let now = Self::current_timestamp_secs();
        let decay_secs = (decay_days as u64).saturating_mul(86400);

        if decay_secs == 0 {
            return 0;
        }

        let elapsed = match last_visited {
            Some(last) => now.saturating_sub(last),
            None => decay_secs, // Treat tabs without last_visited as fully decayed
        };

        // Calculate decay level (0-5) based on percentage of decay threshold
        let percentage = (elapsed as f64 / decay_secs as f64).min(1.0);

        match percentage {
            p if p <= 0.0 => 0, // Fresh
            p if p < 0.2 => 1,  // Slightly decayed
            p if p < 0.4 => 2,  // Moderately decayed
            p if p < 0.6 => 3,  // Significantly decayed
            p if p < 0.8 => 4,  // Heavily decayed
            _ => 5,             // Expired (>= 80% of threshold)
        }
    }

    /// Get all tabs with their decay levels for a workspace
    pub fn tabs_with_decay(
        &self,
        workspace_id: Option<WorkspaceId>,
        decay_days: u32,
    ) -> Vec<(TabInfo, u8)> {
        self.ordered_tabs()
            .filter(|tab| workspace_id.is_none() || tab.workspace_id == workspace_id.unwrap())
            .map(|tab| {
                let decay_level = Self::calculate_decay_level(tab.last_visited, decay_days);
                (tab.clone(), decay_level)
            })
            .collect()
    }

    /// Get tabs that have reached maximum decay (level 5) and are candidates for auto-shelving
    pub fn expired_tabs(&self, workspace_id: Option<WorkspaceId>, decay_days: u32) -> Vec<TabInfo> {
        self.ordered_tabs()
            .filter(|tab| workspace_id.is_none() || tab.workspace_id == workspace_id.unwrap())
            .filter(|tab| !tab.locked) // Don't expire locked tabs
            .filter(|tab| Self::calculate_decay_level(tab.last_visited, decay_days) >= 5)
            .cloned()
            .collect()
    }

    pub fn workspace_locked_count(&self, workspace_id: WorkspaceId) -> usize {
        self.tabs
            .values()
            .filter(|tab| tab.workspace_id == workspace_id && tab.locked)
            .count()
    }

    pub fn move_tab_to_workspace(
        &mut self,
        tab_id: TabId,
        target_workspace: WorkspaceId,
    ) -> HiWaveResult<()> {
        let tab = self
            .tabs
            .get_mut(&tab_id)
            .ok_or_else(|| hiwave_core::HiWaveError::Config("Tab not found".to_string()))?;

        if tab.workspace_id == target_workspace {
            return Ok(());
        }

        if !self.workspaces.contains_key(&target_workspace) {
            return Err(hiwave_core::HiWaveError::Config(
                "Target workspace not found".to_string(),
            ));
        }

        let previous_workspace = tab.workspace_id;
        tab.workspace_id = target_workspace;

        if let Some(workspace) = self.workspaces.get_mut(&previous_workspace) {
            workspace.tabs.retain(|&id| id != tab_id);
            if workspace.active_tab == Some(tab_id) {
                workspace.active_tab = workspace.tabs.first().copied();
            }
        }

        if let Some(workspace) = self.workspaces.get_mut(&target_workspace) {
            workspace.tabs.push(tab_id);
            workspace.active_tab = Some(tab_id);
        }
        // At the end of its new workspace, as in `workspace.tabs`.
        self.tab_order.retain(|&id| id != tab_id);
        self.tab_order.push(tab_id);

        Ok(())
    }

    pub fn get_active_tab(&self) -> Option<&TabInfo> {
        self.active_workspace
            .and_then(|ws_id| self.workspaces.get(&ws_id))
            .and_then(|ws| ws.active_tab)
            .and_then(|tab_id| self.tabs.get(&tab_id))
    }

    pub fn get_tab(&self, id: TabId) -> Option<&TabInfo> {
        self.tabs.get(&id)
    }

    pub fn get_tab_mut(&mut self, id: TabId) -> Option<&mut TabInfo> {
        self.tabs.get_mut(&id)
    }

    /// Update a tab's URL
    pub fn update_tab_url(&mut self, id: TabId, url: url::Url) -> HiWaveResult<()> {
        if let Some(tab) = self.tabs.get_mut(&id) {
            log::info!("Updating tab {:?} URL to: {}", id, url);
            tab.url = url;
            Ok(())
        } else {
            Err(hiwave_core::HiWaveError::Config(
                "Tab not found".to_string(),
            ))
        }
    }

    /// The open tabs (of one workspace, or all), in tab-strip order.
    pub fn list_tabs(&self, workspace_id: Option<WorkspaceId>) -> Vec<&TabInfo> {
        self.ordered_tabs()
            .filter(|tab| workspace_id.map_or(true, |ws_id| tab.workspace_id == ws_id))
            .collect()
    }

    pub fn snapshot(&self) -> ShellSnapshot {
        let workspaces = self.list_workspaces();

        let active_workspace_index = self
            .active_workspace
            .and_then(|active_id| workspaces.iter().position(|ws| ws.id == active_id));

        let workspace_snapshots = workspaces
            .iter()
            .map(|ws| {
                let mut tabs = Vec::new();
                for tab_id in &ws.tabs {
                    if let Some(tab) = self.tabs.get(tab_id) {
                        tabs.push(TabSnapshot {
                            url: tab.url.to_string(),
                            title: tab.title.clone(),
                            locked: tab.locked,
                            last_visited: tab.last_visited,
                        });
                    }
                }

                let active_tab_index = ws
                    .active_tab
                    .and_then(|tab_id| ws.tabs.iter().position(|id| *id == tab_id));

                WorkspaceSnapshot {
                    name: ws.name.clone(),
                    tabs,
                    active_tab_index,
                }
            })
            .collect();

        ShellSnapshot {
            workspaces: workspace_snapshots,
            active_workspace_index,
        }
    }

    pub fn load_snapshot(&mut self, snapshot: ShellSnapshot) -> HiWaveResult<()> {
        self.tabs.clear();
        self.tab_order.clear();
        self.workspaces.clear();
        self.active_workspace = None;

        let mut workspace_ids = Vec::new();

        for workspace_snapshot in snapshot.workspaces {
            let workspace_id = WorkspaceId::new();
            workspace_ids.push(workspace_id);

            let mut workspace = WorkspaceInfo {
                id: workspace_id,
                name: workspace_snapshot.name,
                tabs: Vec::new(),
                active_tab: None,
                suspended: false,
            };

            for tab_snapshot in workspace_snapshot.tabs {
                let tab_id = TabId::new();
                let url = url::Url::parse(&tab_snapshot.url)
                    .unwrap_or_else(|_| url::Url::parse("about:blank").unwrap());
                let tab_info = TabInfo {
                    id: tab_id,
                    url,
                    title: tab_snapshot.title,
                    favicon: None,
                    workspace_id,
                    suspended: false,
                    loading: false,
                    locked: tab_snapshot.locked,
                    last_visited: tab_snapshot.last_visited,
                };
                self.tabs.insert(tab_id, tab_info);
                self.tab_order.push(tab_id);
                workspace.tabs.push(tab_id);
            }

            if let Some(index) = workspace_snapshot.active_tab_index {
                if index < workspace.tabs.len() {
                    workspace.active_tab = Some(workspace.tabs[index]);
                }
            }

            if workspace.active_tab.is_none() {
                workspace.active_tab = workspace.tabs.first().copied();
            }

            self.workspaces.insert(workspace_id, workspace);
        }

        if let Some(index) = snapshot.active_workspace_index {
            if index < workspace_ids.len() {
                self.active_workspace = Some(workspace_ids[index]);
            }
        }

        if self.active_workspace.is_none() {
            self.active_workspace = workspace_ids.first().copied();
        }

        if self.workspaces.is_empty() {
            let default_ws = self.create_workspace("Default".to_string());
            self.set_active_workspace(default_ws).ok();
        }

        Ok(())
    }

    // Command palette
    pub fn search_commands(&self, query: &str) -> Vec<Command> {
        self.command_palette.search(query)
    }

    pub fn register_command(&mut self, command: Command) {
        self.command_palette.register(command);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabSnapshot {
    pub url: String,
    pub title: Option<String>,
    pub locked: bool,
    pub last_visited: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub name: String,
    pub tabs: Vec<TabSnapshot>,
    pub active_tab_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellSnapshot {
    pub workspaces: Vec<WorkspaceSnapshot>,
    pub active_workspace_index: Option<usize>,
}

impl Default for BrowserShell {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub id: String,
    pub name: String,
    pub description: String,
    pub shortcut: Option<String>,
    pub category: CommandCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandCategory {
    Navigation,
    Workspace,
    Tab,
    Focus,
    Settings,
}

pub struct CommandPalette {
    commands: Vec<Command>,
    matcher: SkimMatcherV2,
}

impl CommandPalette {
    fn new() -> Self {
        let mut palette = Self {
            commands: Vec::new(),
            matcher: SkimMatcherV2::default(),
        };

        // Register default commands
        palette.register_defaults();
        palette
    }

    fn register_defaults(&mut self) {
        let defaults = vec![
            Command {
                id: "new_tab".to_string(),
                name: "New Tab".to_string(),
                description: "Open a new tab".to_string(),
                shortcut: Some("Ctrl+T".to_string()),
                category: CommandCategory::Tab,
            },
            Command {
                id: "close_tab".to_string(),
                name: "Close Tab".to_string(),
                description: "Close the current tab".to_string(),
                shortcut: Some("Ctrl+W".to_string()),
                category: CommandCategory::Tab,
            },
            Command {
                id: "new_workspace".to_string(),
                name: "New Workspace".to_string(),
                description: "Create a new workspace".to_string(),
                shortcut: Some("Ctrl+Shift+N".to_string()),
                category: CommandCategory::Workspace,
            },
            Command {
                id: "focus_mode".to_string(),
                name: "Toggle Focus Mode".to_string(),
                description: "Enable or disable focus mode".to_string(),
                shortcut: Some("Ctrl+Shift+F".to_string()),
                category: CommandCategory::Focus,
            },
        ];

        for command in defaults {
            self.register(command);
        }
    }

    fn register(&mut self, command: Command) {
        self.commands.push(command);
    }

    fn search(&self, query: &str) -> Vec<Command> {
        let mut results: Vec<(i64, Command)> = self
            .commands
            .iter()
            .filter_map(|cmd| {
                self.matcher
                    .fuzzy_match(&cmd.name, query)
                    .map(|score| (score, cmd.clone()))
            })
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| b.0.cmp(&a.0));

        results.into_iter().map(|(_, cmd)| cmd).take(10).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_workspace() {
        let mut shell = BrowserShell::new();
        let ws = shell.create_workspace("Test".to_string());

        let workspace = shell.get_workspace(ws).unwrap();
        assert_eq!(workspace.name, "Test");
    }

    #[test]
    fn test_command_search() {
        let shell = BrowserShell::new();
        let results = shell.search_commands("new");

        assert!(!results.is_empty());
        assert!(results.iter().any(|c| c.id == "new_tab"));
    }
}

/// M1c: the shell keeps its tabs in one explicit order (the order they
/// were opened; a new tab goes at the end, a moved tab goes at the end of
/// its new workspace, a closed tab leaves the rest in place), and every
/// reader of tab order reads it.
#[cfg(test)]
mod tab_order_tests {
    use super::*;

    fn tab(ws: WorkspaceId, n: usize) -> TabInfo {
        TabInfo {
            id: TabId::new(),
            url: url::Url::parse(&format!("http://127.0.0.1/p{n}")).unwrap(),
            title: Some(format!("p{n}")),
            favicon: None,
            workspace_id: ws,
            suspended: false,
            loading: false,
            locked: false,
            last_visited: None,
        }
    }

    fn ids(tabs: Vec<&TabInfo>) -> Vec<TabId> {
        tabs.iter().map(|t| t.id).collect()
    }

    /// Every reader of tab order agrees with `expected` for workspace `ws`,
    /// and the workspace's own list does too.
    fn assert_order(shell: &BrowserShell, ws: WorkspaceId, expected: &[TabId], at: &str) {
        assert_eq!(ids(shell.list_tabs(Some(ws))), expected, "{at}: list_tabs(ws)");
        assert_eq!(shell.get_workspace(ws).unwrap().tabs, expected, "{at}: workspace.tabs");
        let decay: Vec<TabId> = shell.tabs_with_decay(Some(ws), 7).iter().map(|(t, _)| t.id).collect();
        assert_eq!(decay, expected, "{at}: tabs_with_decay");
        let expired: Vec<TabId> = shell.expired_tabs(Some(ws), 7).iter().map(|t| t.id).collect();
        assert_eq!(expired, expected, "{at}: expired_tabs (no tab visited: all expired)");
    }

    #[test]
    fn list_tabs_is_in_the_order_the_tabs_were_opened() {
        let mut shell = BrowserShell::new();
        let ws = shell.get_active_workspace().unwrap().id;
        let opened: Vec<TabId> = (0..12).map(|n| shell.create_tab(tab(ws, n)).unwrap()).collect();
        assert_order(&shell, ws, &opened, "12 opened");
        assert_eq!(ids(shell.list_tabs(None)), opened, "list_tabs(None)");
    }

    #[test]
    fn deleting_a_workspace_activates_the_lowest_id_and_clears_its_tabs_from_the_order() {
        // `keys().next()` on a HashMap picks an arbitrary survivor, and each shell has its own
        // random hasher. With six survivors one shell would pass by luck about one time in six,
        // so many fresh shells are tried: a regression to an arbitrary pick fails some of them.
        for round in 0..64 {
            let mut shell = BrowserShell::new();
            let mut workspaces = vec![shell.get_active_workspace().unwrap().id];
            for n in 1..8 {
                workspaces.push(shell.create_workspace(format!("W{n}")));
            }
            // Two tabs in every workspace, opened workspace by workspace.
            let mut tabs: Vec<(WorkspaceId, TabId)> = Vec::new();
            for (n, ws) in workspaces.iter().enumerate() {
                for k in 0..2 {
                    tabs.push((*ws, shell.create_tab(tab(*ws, n * 2 + k)).unwrap()));
                }
            }
            let victim = workspaces[3];
            shell.set_active_workspace(victim).unwrap();
            shell.delete_workspace(victim).unwrap();

            // The lowest id survives as the active workspace, not an arbitrary one.
            let lowest = workspaces.iter().filter(|ws| **ws != victim).min_by_key(|ws| ws.0).copied().unwrap();
            assert_eq!(shell.get_active_workspace().unwrap().id, lowest, "round {round}");
            // The deleted workspace's tabs are gone from the map and from the order; the
            // other tabs keep their places.
            let survivors: Vec<TabId> = tabs.iter().filter(|(ws, _)| *ws != victim).map(|(_, id)| *id).collect();
            for (ws, id) in &tabs {
                assert_eq!(shell.get_tab(*id).is_none(), *ws == victim, "round {round}");
            }
            assert_eq!(ids(shell.list_tabs(None)), survivors, "round {round}");
            assert_eq!(shell.tab_order, survivors, "round {round}: no stale entry for a deleted tab");

            // Delete the active (lowest) workspace as well: with six survivors left, the next
            // lowest id is chosen, again and not by luck.
            shell.delete_workspace(lowest).unwrap();
            let next = workspaces
                .iter()
                .filter(|ws| **ws != victim && **ws != lowest)
                .min_by_key(|ws| ws.0)
                .copied()
                .unwrap();
            assert_eq!(shell.get_active_workspace().unwrap().id, next, "round {round}, second delete");
            assert_eq!(shell.list_workspaces().len(), 6);
            let left: Vec<TabId> = tabs.iter().filter(|(ws, _)| *ws != victim && *ws != lowest).map(|(_, id)| *id).collect();
            assert_eq!(shell.tab_order, left, "round {round}, second delete");
        }
    }

    #[test]
    fn closing_a_tab_keeps_the_others_in_order() {
        let mut shell = BrowserShell::new();
        let ws = shell.get_active_workspace().unwrap().id;
        let mut opened: Vec<TabId> = (0..6).map(|n| shell.create_tab(tab(ws, n)).unwrap()).collect();
        shell.close_tab(opened[1]).unwrap();
        opened.remove(1);
        shell.close_tab(opened[3]).unwrap();
        opened.remove(3);
        assert_order(&shell, ws, &opened, "after closing the 2nd and 5th");
    }

    /// A tiny deterministic generator, so a failure replays.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, below: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % below
        }
    }

    #[test]
    fn the_order_survives_a_hundred_random_opens_and_closes() {
        let mut shell = BrowserShell::new();
        let ws = shell.get_active_workspace().unwrap().id;
        let mut rng = Lcg(0x5eed_1c);
        let mut model: Vec<TabId> = Vec::new();
        for step in 0..100 {
            if model.is_empty() || rng.next(3) != 0 {
                model.push(shell.create_tab(tab(ws, step)).unwrap());
            } else {
                let victim = model.remove(rng.next(model.len()));
                shell.close_tab(victim).unwrap();
            }
            assert_order(&shell, ws, &model, &format!("step {step}"));
        }
        assert!(model.len() > 20, "the walk should leave many tabs open: {}", model.len());
    }

    #[test]
    fn a_moved_tab_goes_to_the_end_of_its_new_workspace() {
        let mut shell = BrowserShell::new();
        let a = shell.get_active_workspace().unwrap().id;
        let b = shell.create_workspace("B".to_string());
        let in_a: Vec<TabId> = (0..4).map(|n| shell.create_tab(tab(a, n)).unwrap()).collect();
        let in_b: Vec<TabId> = (0..3).map(|n| shell.create_tab(tab(b, n)).unwrap()).collect();
        shell.move_tab_to_workspace(in_a[1], b).unwrap();
        assert_order(&shell, a, &[in_a[0], in_a[2], in_a[3]], "source after move");
        assert_order(&shell, b, &[in_b[0], in_b[1], in_b[2], in_a[1]], "target after move");
    }

    #[test]
    fn a_snapshot_round_trip_keeps_the_order() {
        let mut shell = BrowserShell::new();
        let ws = shell.get_active_workspace().unwrap().id;
        for n in 0..9 {
            shell.create_tab(tab(ws, n)).unwrap();
        }
        let mut restored = BrowserShell::new();
        restored.load_snapshot(shell.snapshot()).unwrap();
        let ws = restored.get_active_workspace().unwrap().id;
        let titles: Vec<String> = restored
            .list_tabs(Some(ws))
            .iter()
            .map(|t| t.title.clone().unwrap())
            .collect();
        let expected: Vec<String> = (0..9).map(|n| format!("p{n}")).collect();
        assert_eq!(titles, expected, "restored list_tabs order");
        let restored_ids = restored.get_workspace(ws).unwrap().tabs.clone();
        assert_order(&restored, ws, &restored_ids, "restored");
    }

    #[test]
    fn list_workspaces_is_in_creation_order() {
        let mut shell = BrowserShell::new();
        let first = shell.get_active_workspace().unwrap().id;
        let made: Vec<WorkspaceId> = (0..8).map(|n| shell.create_workspace(format!("w{n}"))).collect();
        let listed: Vec<WorkspaceId> = shell.list_workspaces().iter().map(|w| w.id).collect();
        let mut expected = vec![first];
        expected.extend(made);
        assert_eq!(listed, expected);
    }
}
