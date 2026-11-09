//! The native shell's tabs, kept in the shared `BrowserShell`.
//!
//! The WebView2 shell (and macOS) keep every tab in `AppState.shell` (a `TabId`, a
//! workspace, a lock flag, persistence through `workspace_state.json`). Workspaces and
//! the shelf are views over that model, so the native shell's tabs have to live in it
//! too. Rather than hook every create / close / activate path, the shell calls
//! `reconcile` once a turn with the tabs it has; this module makes the shared model
//! match and reports what it changed. Plain functions over `&mut AppState` so it can
//! be tested without a window.
//!
//! Tabs the native shell did not create (restored from a previous session) are never
//! touched: only ids in `mirrored` are ours.

use std::collections::HashSet;

use hiwave_core::types::{TabId, TabInfo};

use crate::state::AppState;

/// A tab the native shell has, as the shared model should see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTab {
    pub id: u64,
    pub url: String,
    pub title: String,
    pub active: bool,
}

/// A new tab id from the shared counter, never 0 (the chrome treats a 0 id as "none").
/// Using the same counter as `BrowserShell` means a native id can never collide with a
/// tab restored from disk.
pub fn alloc_tab_id() -> u64 {
    loop {
        let id = TabId::new().0;
        if id != 0 {
            return id;
        }
    }
}

/// What `reconcile` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// Something in the shared model changed (so it should be saved).
    pub changed: bool,
    /// Of the tabs given, the ones the shared model has locked.
    pub locked: HashSet<u64>,
}

/// Make the shared model match `tabs`: add the ones it lacks (in the active workspace),
/// drop mirrored ones that are gone, and update url, title and the active tab.
pub fn reconcile(state: &mut AppState, mirrored: &mut HashSet<u64>, tabs: &[NativeTab]) -> Reconciled {
    let mut out = Reconciled::default();
    let workspace = state.shell.get_active_workspace().map(|w| w.id);

    for tab in tabs {
        let id = TabId(tab.id);
        let url = url::Url::parse(&tab.url).ok();
        if state.shell.get_tab(id).is_none() {
            let (Some(workspace_id), Some(url)) = (workspace, url.clone().or_else(|| url::Url::parse("about:blank").ok())) else {
                continue;
            };
            let info = TabInfo {
                id,
                url,
                title: Some(tab.title.clone()),
                favicon: None,
                workspace_id,
                suspended: false,
                loading: false,
                locked: false,
                // A tab with no last visit reads as fully decayed in the sidebar.
                last_visited: Some(AppState::current_timestamp_secs()),
            };
            if state.shell.create_tab(info).is_ok() {
                mirrored.insert(tab.id);
                out.changed = true;
            }
        }
        if let Some(existing) = state.shell.get_tab_mut(id) {
            if let Some(url) = url {
                if existing.url != url {
                    existing.url = url;
                    out.changed = true;
                }
            }
            let title = Some(tab.title.clone());
            if existing.title != title {
                existing.title = title;
                out.changed = true;
            }
            if existing.locked {
                out.locked.insert(tab.id);
            }
        }
        if tab.active {
            let is_active = state.shell.get_active_tab().map(|t| t.id) == Some(id);
            if !is_active && state.shell.set_active_tab(id).is_ok() {
                out.changed = true;
            }
        }
    }

    // Ours that the native shell no longer has.
    let present: HashSet<u64> = tabs.iter().map(|t| t.id).collect();
    let gone: Vec<u64> = mirrored.iter().copied().filter(|id| !present.contains(id)).collect();
    for id in gone {
        let _ = state.shell.close_tab(TabId(id));
        mirrored.remove(&id);
        out.changed = true;
    }
    out
}

/// A tab this shell saved: the workspace's name and the tab's url.
pub type SavedTab = (String, String);

/// The tabs this shell owns, as they sit in the shared model.
pub fn native_session(state: &AppState, mirrored: &HashSet<u64>) -> Vec<SavedTab> {
    let mut out = Vec::new();
    for ws in state.shell.list_workspaces() {
        for id in &ws.tabs {
            if !mirrored.contains(&id.0) {
                continue;
            }
            if let Some(tab) = state.shell.get_tab(*id) {
                out.push((ws.name.clone(), tab.url.to_string()));
            }
        }
    }
    out
}

/// Where the native shell records which saved tabs are its own. `workspace_state.json` is
/// shared with the default (WebView2) build, whose tabs must never be touched from here.
pub fn native_session_path(state: &AppState) -> std::path::PathBuf {
    state.config.data_dir.join("native_session.json")
}

/// The tabs the last native session saved (empty when there is no record).
pub fn read_native_session(state: &AppState) -> Vec<SavedTab> {
    std::fs::read_to_string(native_session_path(state))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn write_native_session(state: &AppState, tabs: &[SavedTab]) -> std::io::Result<()> {
    let path = native_session_path(state);
    if tabs.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let raw = serde_json::to_string(tabs).map_err(std::io::Error::other)?;
    std::fs::write(path, raw)
}

/// Drop the tabs an earlier native session saved. The native shell starts with one fresh
/// tab and cannot reopen them; left in the model they were saved and reloaded forever.
/// Only tabs in `saved` go (each entry matches one tab, same workspace and url): tabs the
/// default build saved in the same file are left alone. Returns how many were dropped.
pub fn drop_native_saved_tabs(state: &mut AppState, saved: &[SavedTab]) -> usize {
    let mut dropped = 0;
    for (workspace, url) in saved {
        let found = state
            .shell
            .list_workspaces()
            .iter()
            .filter(|ws| &ws.name == workspace)
            .flat_map(|ws| ws.tabs.iter().copied())
            .find(|id| state.shell.get_tab(*id).is_some_and(|t| t.url.as_str() == url));
        if let Some(id) = found {
            let _ = state.shell.close_tab(id);
            dropped += 1;
        }
    }
    dropped
}

/// The native tabs that belong to the active workspace, and which of them it last had
/// active. The tab strip shows only these.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct WorkspaceView {
    /// Native tab ids in the active workspace, in the workspace's order.
    pub visible: Vec<u64>,
    /// The workspace's own active tab, when it is one of `visible`.
    pub active: Option<u64>,
}

/// What the native shell must do so its active tab is one the active workspace shows.
#[derive(Debug, PartialEq, Eq)]
pub enum AfterSwitch {
    /// The active tab is already in the workspace.
    Keep,
    /// Make this native tab active.
    Activate(u64),
    /// The workspace has no native tab: open one (it lands in the active workspace).
    NewTab,
}

/// The active workspace's native tabs. `native` is the set of ids this shell owns (tabs
/// restored from disk have no view and are not shown).
pub fn active_workspace_view(state: &AppState, native: &HashSet<u64>) -> WorkspaceView {
    let Some(ws) = state.shell.get_active_workspace() else {
        return WorkspaceView::default();
    };
    let visible: Vec<u64> = ws.tabs.iter().map(|t| t.0).filter(|id| native.contains(id)).collect();
    let active = ws.active_tab.map(|t| t.0).filter(|id| visible.contains(id));
    WorkspaceView { visible, active }
}

/// After the active workspace changed (or a tab moved out of it), pick the tab to show.
pub fn after_switch(view: &WorkspaceView, current_active: Option<u64>) -> AfterSwitch {
    if let Some(current) = current_active {
        if view.visible.contains(&current) {
            return AfterSwitch::Keep;
        }
    }
    match view.active.or_else(|| view.visible.first().copied()) {
        Some(id) => AfterSwitch::Activate(id),
        None => AfterSwitch::NewTab,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppConfig;

    fn state(name: &str) -> AppState {
        let dir = std::env::temp_dir().join(format!("hiwave_shell_tabs_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = AppConfig::default();
        config.data_dir = dir.clone();
        config.download_dir = dir.join("downloads");
        AppState::new(config).expect("state")
    }

    fn tab(id: u64, url: &str, title: &str, active: bool) -> NativeTab {
        NativeTab { id, url: url.into(), title: title.into(), active }
    }

    #[test]
    fn the_view_is_the_active_workspaces_native_tabs_only() {
        let mut s = state("view");
        let mut mirrored = HashSet::new();
        let (a, b) = (alloc_tab_id(), alloc_tab_id());
        reconcile(&mut s, &mut mirrored, &[tab(a, "https://a.test/", "A", false), tab(b, "https://b.test/", "B", true)]);
        let work = s.shell.create_workspace("Work".to_string());
        s.shell.move_tab_to_workspace(TabId(a), work).unwrap();

        let view = active_workspace_view(&s, &mirrored);
        assert_eq!(view.visible, vec![b], "a moved-away tab is not in the strip");
        assert_eq!(view.active, Some(b));

        s.shell.set_active_workspace(work).unwrap();
        let view = active_workspace_view(&s, &mirrored);
        assert_eq!(view.visible, vec![a]);
    }

    fn saved_tab(s: &mut AppState, url: &str) -> TabId {
        let ws = s.shell.get_active_workspace().map(|w| w.id).unwrap();
        let id = TabId::new();
        s.shell
            .create_tab(TabInfo {
                id,
                url: url::Url::parse(url).unwrap(),
                title: None,
                favicon: None,
                workspace_id: ws,
                suspended: true,
                loading: false,
                locked: false,
                last_visited: None,
            })
            .unwrap();
        id
    }

    #[test]
    fn a_native_session_drops_its_own_saved_tabs_and_not_the_webview2_builds() {
        let mut s = state("drop_native_saved");
        // The default (WebView2) build saved this one in the shared file.
        let theirs = saved_tab(&mut s, "https://webview2.test/");
        // The native shell owns this one, and saves a record of it.
        let mut mirrored = HashSet::new();
        let ours = alloc_tab_id();
        reconcile(&mut s, &mut mirrored, &[tab(ours, "https://native.test/", "N", true)]);
        let record = native_session(&s, &mirrored);
        assert_eq!(record, vec![("Default".to_string(), "https://native.test/".to_string())]);
        write_native_session(&s, &record).unwrap();

        // Next native start: only the recorded tab goes.
        let read = read_native_session(&s);
        assert_eq!(read, record);
        assert_eq!(drop_native_saved_tabs(&mut s, &read), 1);
        assert!(s.shell.get_tab(theirs).is_some(), "the WebView2 build's tab survives a native start");
        assert!(s.shell.get_tab(TabId(ours)).is_none());
        assert_eq!(drop_native_saved_tabs(&mut s, &read), 0, "nothing left to drop");

        write_native_session(&s, &[]).unwrap();
        assert!(read_native_session(&s).is_empty());
    }

    #[test]
    fn a_saved_entry_matches_one_tab_in_its_own_workspace_only() {
        let mut s = state("drop_one");
        saved_tab(&mut s, "https://same.test/");
        saved_tab(&mut s, "https://same.test/");
        let other = s.shell.create_workspace("Other".to_string());
        let kept = TabId::new();
        s.shell
            .create_tab(TabInfo {
                id: kept,
                url: url::Url::parse("https://same.test/").unwrap(),
                title: None,
                favicon: None,
                workspace_id: other,
                suspended: true,
                loading: false,
                locked: false,
                last_visited: None,
            })
            .unwrap();
        let saved = vec![("Default".to_string(), "https://same.test/".to_string())];
        assert_eq!(drop_native_saved_tabs(&mut s, &saved), 1, "one entry, one tab");
        assert!(s.shell.get_tab(kept).is_some(), "another workspace's tab is not touched");
        assert_eq!(s.shell.list_tabs(None).len(), 2);
    }

    #[test]
    fn a_tab_restored_from_disk_is_not_in_the_view() {
        let mut s = state("restored");
        let ws = s.shell.get_active_workspace().map(|w| w.id).unwrap();
        s.shell
            .create_tab(TabInfo {
                id: TabId::new(),
                url: url::Url::parse("https://old.test/").unwrap(),
                title: None,
                favicon: None,
                workspace_id: ws,
                suspended: true,
                loading: false,
                locked: false,
                last_visited: None,
            })
            .unwrap();
        let view = active_workspace_view(&s, &HashSet::new());
        assert!(view.visible.is_empty());
    }

    #[test]
    fn after_a_switch_the_shell_keeps_picks_or_opens() {
        let view = WorkspaceView { visible: vec![4, 5], active: Some(5) };
        assert_eq!(after_switch(&view, Some(4)), AfterSwitch::Keep);
        // The active tab is in another workspace: go to the workspace's own active tab.
        assert_eq!(after_switch(&view, Some(9)), AfterSwitch::Activate(5));
        let no_active = WorkspaceView { visible: vec![4, 5], active: None };
        assert_eq!(after_switch(&no_active, None), AfterSwitch::Activate(4));
        assert_eq!(after_switch(&WorkspaceView::default(), Some(9)), AfterSwitch::NewTab);
    }

    #[test]
    fn allocated_ids_are_never_zero_and_never_repeat() {
        let a = alloc_tab_id();
        let b = alloc_tab_id();
        assert!(a != 0 && b != 0 && a != b);
    }

    #[test]
    fn a_native_tab_appears_in_the_active_workspace() {
        let mut s = state("add");
        let mut mirrored = HashSet::new();
        let id = alloc_tab_id();
        let r = reconcile(&mut s, &mut mirrored, &[tab(id, "https://a.test/", "A", true)]);
        assert!(r.changed);
        let ws = s.shell.get_active_workspace().map(|w| w.id).unwrap();
        let t = s.shell.get_tab(TabId(id)).expect("mirrored");
        assert_eq!(t.workspace_id, ws);
        assert_eq!(t.url.as_str(), "https://a.test/");
        assert_eq!(t.title.as_deref(), Some("A"));
        assert_eq!(s.shell.get_active_tab().map(|t| t.id), Some(TabId(id)));
    }

    #[test]
    fn a_new_native_tab_is_fresh_not_decayed() {
        let mut s = state("fresh");
        let mut mirrored = HashSet::new();
        let id = alloc_tab_id();
        reconcile(&mut s, &mut mirrored, &[tab(id, "https://a.test/", "A", true)]);
        let visited = s.shell.get_tab(TabId(id)).unwrap().last_visited;
        assert!(visited.is_some(), "no last visit would show the tab as expired");
    }

    #[test]
    fn nothing_changed_means_nothing_to_save() {
        let mut s = state("idle");
        let mut mirrored = HashSet::new();
        let id = alloc_tab_id();
        let tabs = [tab(id, "https://a.test/", "A", true)];
        reconcile(&mut s, &mut mirrored, &tabs);
        let again = reconcile(&mut s, &mut mirrored, &tabs);
        assert!(!again.changed, "a second identical turn must not report a change");
    }

    #[test]
    fn navigation_and_title_updates_reach_the_model() {
        let mut s = state("nav");
        let mut mirrored = HashSet::new();
        let id = alloc_tab_id();
        reconcile(&mut s, &mut mirrored, &[tab(id, "https://a.test/", "A", true)]);
        let r = reconcile(&mut s, &mut mirrored, &[tab(id, "https://b.test/x", "B", true)]);
        assert!(r.changed);
        let t = s.shell.get_tab(TabId(id)).unwrap();
        assert_eq!((t.url.as_str(), t.title.as_deref()), ("https://b.test/x", Some("B")));
    }

    #[test]
    fn a_closed_native_tab_leaves_the_model_but_a_restored_one_stays() {
        let mut s = state("close");
        // A tab restored from a previous session: not the native shell's.
        let ws = s.shell.get_active_workspace().map(|w| w.id).unwrap();
        let restored = TabId::new();
        s.shell
            .create_tab(TabInfo {
                id: restored,
                url: url::Url::parse("https://old.test/").unwrap(),
                title: Some("Old".into()),
                favicon: None,
                workspace_id: ws,
                suspended: true,
                loading: false,
                locked: false,
                last_visited: None,
            })
            .unwrap();
        let mut mirrored = HashSet::new();
        let (a, b) = (alloc_tab_id(), alloc_tab_id());
        reconcile(&mut s, &mut mirrored, &[tab(a, "https://a.test/", "A", false), tab(b, "https://b.test/", "B", true)]);
        reconcile(&mut s, &mut mirrored, &[tab(b, "https://b.test/", "B", true)]);
        assert!(s.shell.get_tab(TabId(a)).is_none(), "the closed native tab is gone");
        assert!(s.shell.get_tab(TabId(b)).is_some());
        assert!(s.shell.get_tab(restored).is_some(), "a tab the native shell did not make is left alone");
    }

    #[test]
    fn locks_live_in_the_model_and_come_back() {
        let mut s = state("lock");
        let mut mirrored = HashSet::new();
        let id = alloc_tab_id();
        let tabs = [tab(id, "https://a.test/", "A", true)];
        assert!(reconcile(&mut s, &mut mirrored, &tabs).locked.is_empty());
        s.lock_tab(TabId(id)).unwrap();
        assert!(reconcile(&mut s, &mut mirrored, &tabs).locked.contains(&id));
        s.unlock_tab(TabId(id)).unwrap();
        assert!(reconcile(&mut s, &mut mirrored, &tabs).locked.is_empty());
    }

    #[test]
    fn an_address_that_does_not_parse_is_kept_as_about_blank() {
        let mut s = state("badurl");
        let mut mirrored = HashSet::new();
        let id = alloc_tab_id();
        reconcile(&mut s, &mut mirrored, &[tab(id, "", "New Tab", true)]);
        assert_eq!(s.shell.get_tab(TabId(id)).unwrap().url.as_str(), "about:blank");
    }
}
