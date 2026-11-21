// The window's `error` event for uncaught exceptions, by where they are thrown, and its rules:
// window.onerror's five arguments and its return-true-cancels, preventDefault, a handler that itself
// throws, reportError(), thrown non-errors, a frame's own window, another origin's script.
var out = { events: [], onerror: [], rejections: [], frame: { parent: [], child: [] }, inHandler: { calls: 0 }, order: [] };
function shape(ev) {
  return { message: String(ev.message), type: Object.prototype.toString.call(ev), isTrusted: ev.isTrusted, cancelable: ev.cancelable,
           bubbles: ev.bubbles, target: ev.target === window, file: String(ev.filename).replace(/^.*\//, "").replace(/\?.*$/, ""),
           lineno: ev.lineno > 0, colno: ev.colno > 0, error: ev.error === null ? "null" : ev.error instanceof Error ? ev.error.name : ev.error instanceof DOMException ? "DOMException " + ev.error.name : typeof ev.error,
           prevented: ev.defaultPrevented };
}
var tag = "";
window.addEventListener("error", function (ev) {
  if (ev.target !== window) return; // a resource's error event (capture is off; kept for clarity)
  var s = shape(ev); s.where = tag; out.events.push(s);
  if (String(ev.message).indexOf("from a frame") >= 0) out.frame.parent.push(String(ev.message));
});
window.onerror = function (message, source, lineno, colno, error) {
  out.onerror.push({ n: arguments.length, message: String(message), source: String(source).replace(/^.*\//, "").replace(/\?.*$/, ""),
                     lineno: typeof lineno, colno: typeof colno, error: error === null ? "null" : error instanceof Error ? error.name : typeof error,
                     thisIsWindow: this === window });
  out.order.push("onerror");
  if (String(message).indexOf("cancel me") >= 0) return true;
};
window.addEventListener("error", function (ev) {
  out.order.push("listener");
  if (String(ev.message).indexOf("cancel me") >= 0) out.cancelSeen = { prevented: ev.defaultPrevented, returnValue: ev.returnValue };
  if (String(ev.message).indexOf("prevent me") >= 0) { ev.preventDefault(); out.preventSeen = ev.defaultPrevented; }
  if (String(ev.message).indexOf("throw in the handler") >= 0 || String(ev.message).indexOf("from the handler") >= 0) {
    out.inHandler.calls++;
    out.inHandler.messages = (out.inHandler.messages || []).concat(String(ev.message));
    if (out.inHandler.calls < 5) throw new Error("from the handler");
  }
});
window.addEventListener("unhandledrejection", function (ev) { var r = ev.reason; out.rejections.push(r instanceof Error ? (/missing/.test(r.message) ? r.name + ": a failed import" : r.message) : typeof r === "object" && r !== null ? JSON.stringify(r) : String(r)); });

function combinators(tag) {
  Promise.all([Promise.reject(new Error(tag + " all"))]);
  Promise.race([Promise.reject(new Error(tag + " race"))]);
  Promise.any([Promise.reject(new Error(tag + " any"))]);
  Promise.allSettled([Promise.reject(new Error(tag + " allSettled, handled"))]);
  Promise.reject(new Error(tag + " finally")).finally(function () {});
  Promise.resolve({ then: function (resolve, reject) { reject(new Error(tag + " thenable")); } });
  (async function () { await Promise.reject(new Error(tag + " awaited")); })();
  import("./window_error_missing.js");
}
var steps = [
  ["combinators, one origin", function () { combinators("one origin"); }],
  ["listener", function () { var b = document.createElement("button"); document.body.appendChild(b);
    b.addEventListener("click", function () { throw new Error("from a listener"); }); b.dispatchEvent(new Event("click")); out.syncAfterDispatch = out.events.length; }],
  ["timer", function () { setTimeout(function () { throw new Error("from a timer"); }, 0); }],
  ["frame callback", function () { requestAnimationFrame(function () { throw new Error("from an animation frame"); }); }],
  ["promise job", function () { Promise.resolve().then(function () { throw new Error("from a promise job"); }); }],
  ["onerror returns true", function () { setTimeout(function () { throw new Error("cancel me"); }, 0); }],
  ["preventDefault", function () { setTimeout(function () { throw new Error("prevent me"); }, 0); }],
  ["handler throws", function () { setTimeout(function () { throw new Error("throw in the handler"); }, 0); }],
  ["reportError", function () { out.reportError = typeof self.reportError; if (typeof self.reportError === "function") { var e = new RangeError("reported");
    var before = out.events.length; self.reportError(e); out.reportErrorSync = out.events.length - before; out.reportErrorSame = out.lastError === e; } }],
  ["reportError no argument", function () { try { self.reportError(); out.reportErrorNoArg = "returned"; } catch (e) { out.reportErrorNoArg = e.name; } }],
  ["thrown string", function () { setTimeout(function () { throw "a string"; }, 0); }],
  ["thrown object", function () { setTimeout(function () { throw { message: "an object's message" }; }, 0); }],
  ["thrown null", function () { setTimeout(function () { throw null; }, 0); }],
  ["inline handler attribute", function () { var d = document.createElement("div"); d.setAttribute("onclick", "throw new Error('from an attribute handler')");
    document.body.appendChild(d); d.dispatchEvent(new Event("click")); }],
  ["syntax error", function () { var s = document.createElement("script"); s.textContent = "var = ;"; document.body.appendChild(s); }],
  ["inserted script", function () { var s = document.createElement("script"); s.textContent = "throw new TypeError('from an inserted script')"; document.body.appendChild(s); }],
  ["module", function () { var s = document.createElement("script"); s.type = "module"; s.src = "window_error_module.js"; document.body.appendChild(s); }],
  ["frame", function () { var f = document.createElement("iframe"); f.src = "window_error_child.html"; document.body.appendChild(f); }],
  ["other origin top level", function () { var s = document.createElement("script"); s.src = window.OTHER + "/window_error_other.js"; document.body.appendChild(s); }],
  ["other origin function", function () { setTimeout(function () { if (typeof otherOriginThrows === "function") otherOriginThrows(); else out.otherMissing = true; }, 0); }],
  ["other origin eval", function () { setTimeout(function () { otherOriginEvals(); }, 0); }],
  ["other origin string", function () { setTimeout(function () { otherOriginThrowsAString(); }, 0); }],
  ["other origin object", function () { setTimeout(function () { otherOriginThrowsAnObject(); }, 0); }],
  ["other origin made function", function () { setTimeout(function () { otherOriginMade(); }, 0); }],
  ["other origin eval-made function", function () { setTimeout(function () { otherOriginEvalMade(); }, 0); }],
  ["other origin timer", function () { setTimeout(otherOriginThrows, 0); }],
  ["other origin rejection in a callback", function () { setTimeout(otherOriginRejects, 0); }],
  ["other origin resource that is no script", function () { var s = document.createElement("script"); s.src = window.OTHER + "/window_error_child.html"; document.body.appendChild(s); }],
  ["other origin dom top level", function () { var s = document.createElement("script"); s.src = window.OTHER + "/window_error_dom.js"; document.body.appendChild(s); }],
  ["other origin dom function", function () { setTimeout(function () { otherOriginBadSelector(); }, 0); }],
  ["other origin dom hierarchy", function () { setTimeout(function () { otherOriginAppendsAncestor(); }, 0); }],
  ["other origin dom timer", function () { otherOriginTimerDom(); }],
  ["other origin dom listener", function () { otherOriginListens(document.body); document.body.dispatchEvent(new Event("pong")); }],
  ["other origin dom rejections", function () { otherOriginAsyncDom(); otherOriginExecutorDom(); otherOriginReactionDom(); otherOriginCombinators(); }],
  ["other origin string timer", function () { otherOriginStringTimer(); }],
  ["other origin attribute", function () { otherOriginAttribute(); }],
  ["other origin inline script", function () { otherOriginInline(); }],
  // The page's own code, with another origin's script on the page.
  ["page function called by the other origin", function () { setTimeout(function () { otherOriginCalls(function () { throw new Error("the page's, called by the other origin"); }); }, 0); }],
  ["page-made function called by the other origin", function () { var made = new Function("throw new Error('in a function the page made')"); setTimeout(function () { otherOriginCalls(made); }, 0); }],
  ["page listener run for the other origin", function () { document.body.addEventListener("ping", function () { throw new Error("the page's listener, dispatched by the other origin"); });
    setTimeout(function () { otherOriginDispatches(document.body); }, 0); }],
  ["page dom after", function () { setTimeout(function () { document.querySelector("#" + "page" + "!!"); }, 0); }],
  ["page dom hierarchy after", function () { setTimeout(function () { document.body.appendChild(document.documentElement); }, 0); }],
  ["combinators, mixed", function () { combinators("mixed"); }],
  ["page string after", function () { setTimeout(function () { throw "the page's string"; }, 0); }],
  ["page object after", function () { setTimeout(function () { throw { ours: 1 }; }, 0); }],
  ["page rejections after", function () { setTimeout(function () { Promise.reject({ status: 404 }); Promise.reject("the page's string reason"); Promise.reject(undefined);
    Promise.reject(new Error("the page's error reason")).then(function () {}); }, 0); }],
  ["dispatched by the page", function () { var before = out.onerror.length; window.dispatchEvent(new ErrorEvent("error", { message: "made by the page", filename: "f.js", lineno: 3, colno: 4, error: null, cancelable: true }));
    out.pageDispatchCallsOnerror = out.onerror.length - before; }]
];
window.addEventListener("error", function (ev) { out.lastError = ev.error; });
var i = 0;
function next() {
  if (i >= steps.length) { tag = "done"; return setTimeout(function () { window.__out = JSON.stringify(out); console.log("PROBE " + window.__out); }, 300); }
  var step = steps[i++]; tag = step[0];
  try { step[1](); } catch (e) { out.events.push({ where: tag, stepThrew: String(e) }); }
  setTimeout(next, 250);
}
setTimeout(next, 50);
tag = "top level";
throw new Error("from a script's top level");
