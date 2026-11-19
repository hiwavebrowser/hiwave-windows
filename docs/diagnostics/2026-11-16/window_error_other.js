// Served from another origin (another port): what it throws is reported to the page without detail.
function otherOriginThrows() { throw new Error("from another origin's script"); }
function otherOriginEvals() { eval("throw new Error('from another origin, in eval')"); }
function otherOriginThrowsAString() { throw "another origin's string"; }
function otherOriginThrowsAnObject() { throw { theirs: 1 }; }
function otherOriginCalls(f) { f(); }
var otherOriginMade = new Function("throw new Error('in a function another origin made')");
var otherOriginEvalMade = eval("(function () { throw new Error('in a function another origin made with eval'); })");
function otherOriginRejects() { Promise.reject(new Error("another origin's later rejection")); }
// The browser's own errors, raised for this script's calls.
function otherOriginBadSelector() { document.querySelector("#" + "x" + "!!"); }
function otherOriginAppendsAncestor() { document.body.appendChild(document.documentElement); }
function otherOriginTimerDom() { setTimeout(otherOriginBadSelector, 0); }
function otherOriginListens(el) { el.addEventListener("pong", otherOriginBadSelector); }
async function otherOriginAsyncDom() { await null; document.querySelector("#!!"); }
function otherOriginExecutorDom() { new Promise(function () { document.querySelector("#!!"); }); }
function otherOriginReactionDom() { Promise.resolve().then(function () { document.querySelector("#!!"); }); }
function otherOriginDispatches(el) { el.dispatchEvent(new Event("ping")); }
// Code the standard gives to the document, made by this script.
function otherOriginStringTimer() { setTimeout("throw new Error('in a string timer another origin set')", 0); }
function otherOriginAttribute() { var d = document.createElement("div"); d.setAttribute("onclick", "throw new Error('in a handler attribute another origin set')"); document.body.appendChild(d); d.click(); }
function otherOriginInline() { var s = document.createElement("script"); s.textContent = "throw new Error('in an inline script another origin inserted')"; document.body.appendChild(s); }
function otherOriginCombinators() { Promise.all([Promise.reject(new Error("their all"))]); Promise.reject(new Error("their finally")).finally(function () {}); }
Promise.reject(new Error("another origin's rejection"));
throw new Error("from another origin's top level");
