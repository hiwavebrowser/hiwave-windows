//! Data the chrome's panels show (history, downloads), as scripts for the chrome page.
//!
//! The WebView2 shell builds these inline in `main.rs` (`sync_history_to_chrome`,
//! `sync_downloads_to_chrome`); they are `cfg(not(native-win32))`, so the native
//! shell has its own. Kept as plain functions so what reaches the chrome can be
//! tested without a window.

use crate::state::AppState;

/// A call into the chrome page: `hiwaveChrome.<method>(<payload>)`, guarded for a
/// page that has not set `hiwaveChrome` up yet. The payload is serialised JSON, so
/// no text in it can become script.
pub fn chrome_call(method: &str, payload: &serde_json::Value) -> String {
    format!("if(window.hiwaveChrome) {{ hiwaveChrome.{method}({payload}); }}")
}

/// The history panel's data: every visit, newest first.
pub fn history_script(state: &AppState) -> String {
    let history = serde_json::to_value(state.visit_history_snapshot()).unwrap_or(serde_json::json!([]));
    chrome_call("updateHistory", &history)
}

/// The sidebar's workspaces: each with its tabs (ids as strings, the shape the chrome's
/// `renderWorkspaces` reads), the active one, and how many tabs are locked. The same
/// shape the WebView2 shell builds in `main.rs`.
pub fn workspaces_json(state: &AppState) -> serde_json::Value {
    let active_ws = state.shell.get_active_workspace().map(|w| w.id);
    let active_tab = state.shell.get_active_tab().map(|t| t.id);
    let decay_days = state.user_settings.tab_decay_days;
    // `list_workspaces` walks a HashMap, so its order changes from run to run; the sidebar
    // lists workspaces in the order they were made.
    let mut ordered = state.shell.list_workspaces();
    ordered.sort_by_key(|ws| ws.id.0);
    let workspaces: Vec<serde_json::Value> = ordered
        .iter()
        .map(|ws| {
            let tabs: Vec<serde_json::Value> = ws
                .tabs
                .iter()
                .filter_map(|id| state.shell.get_tab(*id))
                .map(|tab| {
                    serde_json::json!({
                        "id": tab.id.0.to_string(),
                        "title": tab.title.clone().unwrap_or_else(|| "New Tab".to_string()),
                        "url": tab.url.to_string(),
                        "is_active": active_tab == Some(tab.id),
                        "locked": tab.locked,
                        "decay_level": hiwave_shell::BrowserShell::calculate_decay_level(tab.last_visited, decay_days),
                    })
                })
                .collect();
            serde_json::json!({
                "id": ws.id.0.to_string(),
                "name": ws.name,
                "tab_count": ws.tabs.len(),
                "tabs": tabs,
                "is_active": Some(ws.id) == active_ws,
                "is_suspended": ws.suspended,
                "locked_count": state.workspace_locked_count(ws.id),
            })
        })
        .collect();
    serde_json::Value::Array(workspaces)
}

/// The shelf's data for `scope` (`"all"` or `"workspace"`, the active workspace's), in the
/// shape `renderShelf` reads (the same one `main.rs` builds for the WebView2 shell).
pub fn shelf_payload(state: &AppState, scope: &str) -> serde_json::Value {
    let all = scope == "all";
    let (items, workspace) = if all {
        (state.shelf_items_all(), None)
    } else {
        (
            state.shelf_items_for_active_workspace(),
            state.shell.get_active_workspace().map(|w| w.name.clone()),
        )
    };
    serde_json::json!({
        "scope": if all { "all" } else { "workspace" },
        "workspace": workspace,
        "items": items,
        "global_count": state.shelf_count_all(),
    })
}

/// The script that hands the sidebar its shelf.
pub fn shelf_script(state: &AppState, scope: &str) -> String {
    chrome_call("renderShelf", &shelf_payload(state, scope))
}

/// The script that hands the sidebar its workspaces.
pub fn workspaces_script(state: &AppState) -> String {
    chrome_call("renderWorkspaces", &workspaces_json(state))
}

/// The blocklist panel's data: the user's blocked domains, sorted so the list does not
/// reorder itself between calls (the set has no order).
pub fn blocklist_script(state: &AppState) -> String {
    let mut domains: Vec<&String> = state.user_blocklist.domains.iter().collect();
    domains.sort();
    chrome_call("updateBlocklist", &serde_json::json!(domains))
}

/// The downloads panel's data and the count of active downloads.
pub fn downloads_script(state: &AppState) -> String {
    let payload = serde_json::json!({
        "downloads": state.downloads_snapshot(),
        "active_count": state.active_download_count(),
    });
    chrome_call("updateDownloads", &payload)
}

/// Whether a page belongs in the history. The start page, blank tabs and the
/// browser's own addresses do not; neither do `javascript:` and `data:` URLs.
pub fn should_record_history_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    !(lower.starts_with("about:")
        || lower.starts_with("data:")
        || lower.starts_with("javascript:")
        || lower.starts_with("hiwave:"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_chrome_call_is_guarded_and_carries_json() {
        let call = chrome_call("updateHistory", &json!([{"url": "https://a.test/"}]));
        assert_eq!(
            call,
            r#"if(window.hiwaveChrome) { hiwaveChrome.updateHistory([{"url":"https://a.test/"}]); }"#
        );
    }

    #[test]
    fn text_in_the_payload_cannot_become_script() {
        // A page title is page-controlled. It goes through JSON, so a quote or a
        // closing paren is escaped inside a string, not a way out of the call.
        let title = r#"x"); window.ipc.postMessage("evil"); //"#;
        let call = chrome_call("updateHistory", &json!([{ "title": title }]));
        // The whole argument still parses as ONE JSON value, so the hostile text is
        // data inside a string and not a second statement.
        let open = call.find("updateHistory(").unwrap() + "updateHistory(".len();
        let close = call.rfind("); }").unwrap();
        let back: serde_json::Value = serde_json::from_str(&call[open..close]).expect("one JSON value");
        assert_eq!(back[0]["title"], json!(title));
    }

    fn temp_state(name: &str) -> AppState {
        let dir = std::env::temp_dir().join(format!("hiwave_chrome_data_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut config = crate::state::AppConfig::default();
        config.data_dir = dir.clone();
        config.download_dir = dir.join("downloads");
        AppState::new(config).expect("state")
    }

    #[test]
    fn the_workspace_list_has_the_shape_the_chrome_reads() {
        let mut s = temp_state("ws");
        let work = s.shell.create_workspace("Work".to_string());
        let default = s.shell.get_active_workspace().map(|w| w.id).unwrap();
        let tab = hiwave_core::types::TabInfo {
            id: hiwave_core::types::TabId::new(),
            url: url::Url::parse("https://a.test/").unwrap(),
            title: Some("A \"quoted\" title".to_string()),
            favicon: None,
            workspace_id: default,
            suspended: false,
            loading: false,
            locked: true,
            last_visited: None,
        };
        let tab_id = tab.id;
        s.shell.create_tab(tab).unwrap();
        s.shell.set_active_tab(tab_id).unwrap();

        let json = workspaces_json(&s);
        let list = json.as_array().unwrap();
        assert_eq!(list.len(), 2);
        let first = &list[0];
        assert_eq!(first["is_active"], true);
        assert_eq!(first["tab_count"], 1);
        assert_eq!(first["locked_count"], 1);
        assert_eq!(first["tabs"][0]["id"], tab_id.0.to_string());
        assert_eq!(first["tabs"][0]["title"], "A \"quoted\" title");
        assert_eq!(first["tabs"][0]["is_active"], true);
        let second = list.iter().find(|w| w["id"] == work.0.to_string()).unwrap();
        assert_eq!(second["name"], "Work");
        assert_eq!(second["is_active"], false);
        assert!(workspaces_script(&s).starts_with("if(window.hiwaveChrome) { hiwaveChrome.renderWorkspaces(["));
    }

    #[test]
    fn a_tab_the_native_shell_just_opened_is_not_shown_as_expired() {
        let mut s = temp_state("decay");
        let mut mirrored = std::collections::HashSet::new();
        let id = super::super::shell_tabs::alloc_tab_id();
        let tabs = [super::super::shell_tabs::NativeTab {
            id,
            url: "https://a.test/".into(),
            title: "A".into(),
            active: true,
        }];
        super::super::shell_tabs::reconcile(&mut s, &mut mirrored, &tabs);
        let json = workspaces_json(&s);
        let level = json[0]["tabs"][0]["decay_level"].as_u64().unwrap();
        assert!(level < 5, "a fresh tab has decay level {level}, which the sidebar draws struck through");
    }

    #[test]
    fn workspaces_are_listed_in_the_order_they_were_made() {
        let mut s = temp_state("ws_order");
        for name in ["Alpha", "Beta", "Gamma", "Delta", "Epsilon"] {
            s.shell.create_workspace(name.to_string());
        }
        let json = workspaces_json(&s);
        let names: Vec<&str> = json.as_array().unwrap().iter().map(|w| w["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Default", "Alpha", "Beta", "Gamma", "Delta", "Epsilon"]);
    }

    #[test]
    fn the_shelf_payload_follows_the_scope() {
        let mut s = temp_state("shelf");
        let work = s.shell.create_workspace("Work".to_string());
        s.ensure_workspace_shelf("Work");
        let mut mirrored = std::collections::HashSet::new();
        let id = super::super::shell_tabs::alloc_tab_id();
        super::super::shell_tabs::reconcile(
            &mut s,
            &mut mirrored,
            &[super::super::shell_tabs::NativeTab { id, url: "https://a.test/".into(), title: "A".into(), active: true }],
        );
        s.add_to_shelf(hiwave_core::types::TabId(id)).unwrap().expect("shelved");

        let here = shelf_payload(&s, "workspace");
        assert_eq!(here["scope"], "workspace");
        assert_eq!(here["workspace"], "Default");
        assert_eq!(here["items"].as_array().unwrap().len(), 1);
        assert_eq!(here["global_count"], 1);

        s.shell.set_active_workspace(work).unwrap();
        let there = shelf_payload(&s, "workspace");
        assert_eq!(there["items"].as_array().unwrap().len(), 0, "the other workspace's shelf is empty");
        assert_eq!(shelf_payload(&s, "all")["items"].as_array().unwrap().len(), 1);
        // Anything but "all" is the workspace scope.
        assert_eq!(shelf_payload(&s, "bogus")["scope"], "workspace");
        assert!(shelf_script(&s, "all").starts_with("if(window.hiwaveChrome) { hiwaveChrome.renderShelf({"));
    }

    #[test]
    fn pages_and_internal_addresses_are_recorded_or_not() {
        assert!(should_record_history_url("https://example.com/a?b=c"));
        assert!(should_record_history_url("http://localhost:8765/x.html"));
        assert!(should_record_history_url("file:///C:/a.html"));
        assert!(!should_record_history_url(""));
        assert!(!should_record_history_url("about:blank"));
        assert!(!should_record_history_url("hiwave://about"));
        assert!(!should_record_history_url("data:text/html,hi"));
        assert!(!should_record_history_url("JavaScript:alert(1)"));
    }
}
