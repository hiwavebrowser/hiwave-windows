// chrome.js — board state for chrome.html (CH3.A1). Runs AFTER the page's own
// script, through the same entry points the Rust host calls at startup
// (hiwave-app/src/main.rs: setSidebarState, renderWorkspaces, renderTabs,
// renderShelf, updateShieldStats, updateDownloads, updateHistory).
(function () {
  var board = window.__hiwaveBoard;
  var chrome = window.hiwaveChrome;
  var focusOn = false;
  var focusConfig = { show_progress_bar: true, hide_cursor: true, cursor_hide_delay_secs: 3 };

  // The host's answers to the commands these states post.
  board.on('toggle_focus_mode', function () {
    focusOn = !focusOn;
    chrome.setFocusMode(focusOn, focusConfig);
  });
  board.on('get_shelf', function () { chrome.renderShelf(shelfPayload()); });
  board.on('get_sidebar_width', function () { /* answered below, once, as the host does */ });

  // renderWorkspaces writes the active name into #workspaceName, an element
  // chrome.html does not have (it throws there in any engine). A hidden
  // sink keeps the call on the same path in both engines.
  if (!document.getElementById('workspaceName')) {
    var sink = document.createElement('span');
    sink.id = 'workspaceName';
    sink.style.display = 'none';
    document.body.appendChild(sink);
  }

  function shelfPayload() {
    return {
      scope: 'workspace',
      global_count: 2,
      items: [
        { id: 'shelf-1', title: 'Flexbox cheat sheet', domain: 'css-tricks.com', age: '2d', daysRemaining: 5, decayLevel: 0, workspace: 'Research' },
        { id: 'shelf-2', title: 'Rust book — ownership', domain: 'doc.rust-lang.org', age: '6d', daysRemaining: 1, decayLevel: 2, workspace: 'Research' }
      ]
    };
  }

  var tabs = [
    { id: 1, title: 'HiWave — a quieter browser', url: 'https://hiwave.dev/', active: false, is_active: false, locked: false, decay_level: 0 },
    { id: 2, title: 'Rust book — ownership', url: 'https://doc.rust-lang.org/book/ch04-00.html', active: true, is_active: true, locked: false, decay_level: 0 },
    { id: 3, title: 'New Tab', url: 'hiwave://newtab', active: false, is_active: false, locked: true, decay_level: 1 }
  ];
  var workspaces = [
    { id: 'ws-1', name: 'Research', is_active: true, is_suspended: false, locked_count: 1, tab_count: 3,
      tabs: tabs.map(function (t) { return { id: t.id, title: t.title, is_active: t.is_active, locked: t.locked, decay_level: t.decay_level }; }) },
    { id: 'ws-2', name: 'Work', is_active: false, is_suspended: false, locked_count: 0, tab_count: 2,
      tabs: [{ id: 4, title: 'Calendar', is_active: false, locked: false, decay_level: 0 }, { id: 5, title: 'Mail', is_active: false, locked: false, decay_level: 0 }] }
  ];

  chrome.setSidebarState(220, true);
  chrome.renderWorkspaces(workspaces);
  chrome.renderTabs(tabs);
  chrome.renderShelf(shelfPayload());
  chrome.updateShieldStats({ enabled: true, requests_blocked: 128, trackers_blocked: 17 });
  chrome.updateBlocklist(['ads.example', 'tracker.example']);
  chrome.updateDownloads({
    active_count: 1,
    downloads: [
      { file_name: 'rustkit-notes.pdf', path: '/Users/me/Downloads/rustkit-notes.pdf', status: 'completed', url: 'https://hiwave.dev/notes.pdf' },
      { file_name: 'wave.png', path: '/Users/me/Downloads/wave.png', status: 'in_progress', url: 'https://hiwave.dev/wave.png' }
    ]
  });
  chrome.updateHistory([
    { title: 'HiWave — a quieter browser', url: 'https://hiwave.dev/', count: 4, workspace: 'Research' },
    { title: 'Rust book — ownership', url: 'https://doc.rust-lang.org/book/ch04-00.html', count: 2, workspace: 'Research' }
  ]);
  chrome.updateUrl('https://doc.rust-lang.org/book/ch04-00.html');
  chrome.updateNavState(true, false);
})();
