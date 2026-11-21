// ipc_stub.js — the host, stubbed, for the chrome board (CH3.A1).
//
// HiWave's chrome pages talk to the Rust host through `window.ipc.postMessage`
// and the host talks back by calling `window.hiwaveChrome.*`,
// `window.hiwaveShelf.*`, `window.hiwaveInspector.*`, or by dispatching a
// `message` / `hiwave-response` event. A headless render has no host, and
// chrome.html posts its first command at script top level, so this file is
// inserted BEFORE the page's own script, identically for RustKit and Chromium.
//
// It records every command (`window.__hiwaveBoard.calls()`), lets a page
// fixture answer some of them (`window.__hiwaveBoard.on(cmd, handler)`), and
// otherwise does nothing — which is what the real host does with most
// commands as far as the page can see.
(function () {
  var calls = [];
  var handlers = {};
  window.__hiwaveBoard = {
    calls: function () { return calls.slice(); },
    on: function (cmd, fn) { handlers[cmd] = fn; },
    errors: []
  };
  window.ipc = {
    postMessage: function (raw) {
      var msg;
      try { msg = JSON.parse(raw); } catch (e) { msg = { cmd: '?' }; }
      calls.push(msg.cmd);
      var fn = handlers[msg.cmd];
      if (fn) {
        try { fn(msg); } catch (e) { window.__hiwaveBoard.errors.push(msg.cmd + ': ' + e); }
      }
    }
  };
  // report.html waits on a `hiwave-response` CustomEvent; a host polyfill, used
  // only where the engine has no CustomEvent constructor.
  if (typeof window.CustomEvent !== 'function') {
    window.CustomEvent = function (type, init) {
      var ev = document.createEvent('Event');
      ev.initEvent(type, !!(init && init.bubbles), !!(init && init.cancelable));
      ev.detail = init ? init.detail : undefined;
      return ev;
    };
  }
})();
