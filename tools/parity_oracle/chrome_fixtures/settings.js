// settings.js — board state for settings.html (CH3.A1): the host's answers to
// get_settings and get_focus_mode_config, delivered the way the host delivers
// them (a `message` event the page's own handler applies), plus the same
// values applied directly so the state does not depend on event timing.
(function () {
  var board = window.__hiwaveBoard;
  var settings = {
    theme: 'dark',
    default_mode: 'zen',
    sidebar_default_open: true,
    tab_decay_days: 7,
    auto_shelf_days: 3,
    new_tab_url: 'hiwave://newtab',
    shield_enabled: true
  };
  var focus = {
    auto_enabled: true,
    activation_delay_secs: 90,
    show_progress_bar: true,
    hide_cursor: true,
    blocklist: ['news.example', 'social.example']
  };
  function deliver(type, data) {
    window.dispatchEvent(new MessageEvent('message', { data: { type: type, data: data } }));
  }
  board.on('get_settings', function () { deliver('settings_result', settings); });
  board.on('get_focus_mode_config', function () { deliver('focus_config_result', focus); });
  if (typeof applySettingsToUI === 'function') applySettingsToUI(settings);
  if (typeof applyFocusConfigToUI === 'function') applyFocusConfigToUI(focus);
})();
