// report.js — board state for report.html (CH3.A1): the host's answers to the
// analytics requests. report.html's sendIpc resolves on a `hiwave-response`
// event dispatched after the request (its listener is attached right after
// the post), so the stub answers from a microtask: after the page's current
// script, before anything else, in both engines. Not a timer: parity-capture's
// --html-file load runs no timer callbacks after the load, so a host that
// answered on setTimeout would leave RustKit's report at "Loading analytics
// data..." forever (chrome board, 2026-11-05).
(function () {
  var board = window.__hiwaveBoard;
  function respond(data) {
    Promise.resolve().then(function () {
      window.dispatchEvent(new CustomEvent('hiwave-response', { detail: { type: 'success', data: data } }));
    });
  }
  var today = {
    pages_visited: 42, trackers_blocked: 311, ads_blocked: 97, popups_blocked: 3,
    bandwidth_saved: 18.4, time_saved: 27, focus_time: 95
  };
  var days = ['2026-10-29', '2026-10-30', '2026-10-31', '2026-11-01', '2026-11-02', '2026-11-03', '2026-11-04'];
  var daily = days.map(function (d, i) {
    return { date: d, pages_visited: 30 + i * 5, trackers_blocked: 200 + i * 23, ads_blocked: 60 + i * 7, popups_blocked: i % 3, tabs_opened: 12 + i };
  });
  var week = {
    total_stats: { pages_visited: 322, trackers_blocked: 2093, ads_blocked: 567, popups_blocked: 9, bandwidth_saved: 121.7, time_saved: 184, focus_time: 540 },
    daily_breakdown: daily,
    top_domains: [
      { domain: 'doc.rust-lang.org', visit_count: 58, trackers_blocked: 0 },
      { domain: 'news.example', visit_count: 31, trackers_blocked: 412 },
      { domain: 'hiwave.dev', visit_count: 27, trackers_blocked: 0 }
    ],
    workspace_breakdown: [
      { workspace_id: 'ws-1', tab_count: 3, visit_count: 190 },
      { workspace_id: 'ws-2', tab_count: 2, visit_count: 132 }
    ]
  };
  board.on('get_today_stats', function () { respond(today); });
  board.on('get_weekly_report', function () { respond(week); });
  board.on('get_monthly_report', function () { respond(week); });
})();
