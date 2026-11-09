//! The native shell's tabs, kept in the shared `BrowserShell`.
//!
//! The WebView2 shell (and macOS) keep every tab in `AppState.shell` (a `TabId`, a
//! workspace, a lock flag, persistence through the native shell's own `workspace_state.native.json`). Workspaces and
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

/// What reopening a shelved page will do.
#[derive(Debug, PartialEq, Eq)]
pub enum RestoreKind {
    /// Load this address in the new tab.
    Page,
    /// The app's own start page: the new tab already is it.
    StartPage,
    /// The domain is on the user's blocklist: nothing opens.
    Blocked,
    /// The address is not something the shell can load.
    Unopenable,
}

/// Decide, before touching the shelf, whether `url` can be reopened.
pub fn restore_kind(state: &AppState, url: &str) -> RestoreKind {
    if url.starts_with("hiwave://about") || url == "about:blank" {
        return RestoreKind::StartPage;
    }
    if super::chrome_data::url_is_blocked(state, url) {
        return RestoreKind::Blocked;
    }
    if super::address::resolve(url).is_none() {
        return RestoreKind::Unopenable;
    }
    RestoreKind::Page
}

/// A shelved item, without taking it off the shelf: its address and the workspace a restore
/// opens it in (the target, else the workspace it was shelved from, else the active one).
pub fn peek_shelf_item(
    state: &AppState,
    id: &str,
    target: Option<hiwave_core::types::WorkspaceId>,
) -> Option<(String, hiwave_core::types::WorkspaceId)> {
    let (source, item) = state
        .shelves
        .iter()
        .find_map(|(workspace, items)| items.iter().find(|item| item.id == id).map(|item| (workspace.clone(), item)))?;
    let workspace = target.or_else(|| {
        let name = if source.is_empty() { item.workspace.clone() } else { source };
        state
            .shell
            .list_workspaces()
            .iter()
            .find(|ws| ws.name == name)
            .map(|ws| ws.id)
            .or_else(|| state.shell.get_active_workspace().map(|ws| ws.id))
    })?;
    Some((item.url.clone(), workspace))
}

/// Take an item off the shelf for the native shell to reopen. Unlike
/// `AppState::restore_from_shelf` this does not add a tab to the shared model: the native
/// shell opens the page itself and `reconcile` adds it. It does make the target the active
/// workspace, as the shared restore does. Returns the address and the workspace. Call it only
/// once the page is going to open (see `restore_kind`): a refused restore must keep the item.
pub fn take_from_shelf(
    state: &mut AppState,
    id: &str,
    target: Option<hiwave_core::types::WorkspaceId>,
) -> Option<(String, hiwave_core::types::WorkspaceId)> {
    let (url, workspace) = peek_shelf_item(state, id, target)?;
    for items in state.shelves.values_mut() {
        if let Some(index) = items.iter().position(|item| item.id == id) {
            items.remove(index);
            break;
        }
    }
    let _ = state.shell.set_active_workspace(workspace);
    Some((url, workspace))
}

/// The file the native shell keeps its workspaces and tabs in. Not `workspace_state.json`:
/// that is the default (WebView2) build's session, which the native shell can neither
/// restore nor has any business deleting.
pub const NATIVE_STATE_FILE: &str = "workspace_state.native.json";

/// Forget every tab the model has. The native shell starts with one fresh tab and cannot
/// reopen tabs saved by an earlier native session; left in the model they were saved and
/// reloaded forever. Returns how many were dropped.
pub fn drop_unreopenable_tabs(state: &mut AppState) -> usize {
    let ids: Vec<TabId> = state.shell.list_tabs(None).iter().map(|t| t.id).collect();
    for id in &ids {
        let _ = state.shell.close_tab(*id);
    }
    ids.len()
}

/// Make the workspace that owns tab `id` the active one (the sidebar lists every workspace's
/// tabs, so a click can name a tab that is not in the strip). True when it switched.
pub fn switch_to_tab_workspace(state: &mut AppState, id: u64) -> bool {
    let Some(owner) = state.shell.get_tab(TabId(id)).map(|t| t.workspace_id) else {
        return false;
    };
    if state.shell.get_active_workspace().map(|w| w.id) == Some(owner) {
        return false;
    }
    state.shell.set_active_workspace(owner).is_ok()
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
    fn startup_drops_every_tab_of_the_native_file_and_leaves_no_workspace_entry() {
        let mut s = state("drop_native_file");
        for n in 0..3 {
            saved_tab(&mut s, &format!("https://old{n}.test/"));
        }
        assert_eq!(drop_unreopenable_tabs(&mut s), 3);
        assert!(s.shell.list_tabs(None).is_empty());
        assert!(s.shell.get_active_workspace().unwrap().tabs.is_empty());
        assert_eq!(drop_unreopenable_tabs(&mut s), 0);
    }

    #[test]
    fn the_native_state_file_is_not_the_default_builds() {
        let mut s = state("own_file");
        s.workspace_state_file = NATIVE_STATE_FILE.to_string();
        saved_tab(&mut s, "https://native.test/");
        s.save_workspace_state().unwrap();
        assert!(s.config.data_dir.join(NATIVE_STATE_FILE).exists());
        assert!(
            !s.config.data_dir.join("workspace_state.json").exists(),
            "a native save must not create or touch the default build's file"
        );
    }

    #[test]
    fn a_restore_that_would_be_refused_leaves_the_item_on_the_shelf() {
        let mut s = state("refused_restore");
        let mut mirrored = HashSet::new();
        let (blocked, ok, start) = (alloc_tab_id(), alloc_tab_id(), alloc_tab_id());
        reconcile(
            &mut s,
            &mut mirrored,
            &[
                tab(blocked, "https://blocked.test/", "B", false),
                tab(ok, "https://fine.test/", "F", false),
                tab(start, "hiwave://about", "About", true),
            ],
        );
        let ids: Vec<String> = [blocked, ok, start]
            .iter()
            .map(|id| s.add_to_shelf(TabId(*id)).unwrap().expect("shelved").id)
            .collect();
        s.block_domain("blocked.test".to_string()).unwrap();

        // Blocked: classified before anything leaves the shelf.
        let (url, _) = peek_shelf_item(&s, &ids[0], None).expect("still there");
        assert_eq!(restore_kind(&s, &url), RestoreKind::Blocked);
        assert!(peek_shelf_item(&s, &ids[0], None).is_some(), "a peek takes nothing");
        // The start page and an ordinary page are openable.
        let (url, _) = peek_shelf_item(&s, &ids[2], None).unwrap();
        assert_eq!(restore_kind(&s, &url), RestoreKind::StartPage);
        let (url, _) = peek_shelf_item(&s, &ids[1], None).unwrap();
        assert_eq!(restore_kind(&s, &url), RestoreKind::Page);

        // Only the take removes it, and only the one asked for.
        assert!(take_from_shelf(&mut s, &ids[1], None).is_some());
        assert!(peek_shelf_item(&s, &ids[1], None).is_none());
        assert!(peek_shelf_item(&s, &ids[0], None).is_some(), "the blocked item is still shelved");
        assert!(peek_shelf_item(&s, &ids[2], None).is_some());
    }

    #[test]
    fn clicking_a_tab_of_another_workspace_switches_to_that_workspace() {
        let mut s = state("click_other_ws");
        let mut mirrored = HashSet::new();
        let (a, b) = (alloc_tab_id(), alloc_tab_id());
        reconcile(&mut s, &mut mirrored, &[tab(a, "https://a.test/", "A", false), tab(b, "https://b.test/", "B", true)]);
        let work = s.shell.create_workspace("Work".to_string());
        s.shell.move_tab_to_workspace(TabId(a), work).unwrap();

        assert!(!switch_to_tab_workspace(&mut s, b), "b is already in the active workspace");
        assert!(switch_to_tab_workspace(&mut s, a));
        assert_eq!(s.shell.get_active_workspace().map(|w| w.id), Some(work));
        assert_eq!(active_workspace_view(&s, &mirrored).visible, vec![a]);
        assert!(!switch_to_tab_workspace(&mut s, 999_999), "an unknown tab changes nothing");
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
    fn taking_an_item_off_the_shelf_returns_its_page_and_activates_its_workspace() {
        let mut s = state("shelf_take");
        let mut mirrored = HashSet::new();
        let id = alloc_tab_id();
        reconcile(&mut s, &mut mirrored, &[tab(id, "https://a.test/page", "A", true)]);
        let item = s.add_to_shelf(TabId(id)).unwrap().expect("shelved");
        let work = s.shell.create_workspace("Work".to_string());
        s.shell.set_active_workspace(work).unwrap();

        let (url, workspace) = take_from_shelf(&mut s, &item.id, None).expect("on the shelf");
        assert_eq!(url, "https://a.test/page");
        // It goes back to the workspace it was shelved from, and that becomes active.
        assert_eq!(s.shell.get_workspace(workspace).map(|w| w.name.as_str()), Some("Default"));
        assert_eq!(s.shell.get_active_workspace().map(|w| w.id), Some(workspace));
        assert_eq!(s.shelf_count_all(), 0, "it is off the shelf");
        // And the shared model got no tab out of it: the native shell opens the page.
        assert!(s.shell.list_tabs(Some(workspace)).is_empty());
        assert!(take_from_shelf(&mut s, &item.id, None).is_none(), "a second take finds nothing");
    }

    #[test]
    fn an_explicit_target_workspace_wins() {
        let mut s = state("shelf_target");
        let mut mirrored = HashSet::new();
        let id = alloc_tab_id();
        reconcile(&mut s, &mut mirrored, &[tab(id, "https://a.test/", "A", true)]);
        let item = s.add_to_shelf(TabId(id)).unwrap().unwrap();
        let work = s.shell.create_workspace("Work".to_string());
        let (_, workspace) = take_from_shelf(&mut s, &item.id, Some(work)).unwrap();
        assert_eq!(workspace, work);
        assert_eq!(s.shell.get_active_workspace().map(|w| w.id), Some(work));
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
