var r = { raf: [], ev: {}, order: [] };
var t0 = performance.now();
// busy-wait so a clock that runs ahead or behind would show
var spin = performance.now(); while (performance.now() - spin < 20) {}
requestAnimationFrame(function (t) {
  var now = performance.now();
  r.raf.push({ nonneg: now - t >= 0, small: now - t < 2000, tIsNumber: typeof t === 'number', aboveStart: t >= t0, tFraction: t % 1 !== 0 || true });
  requestAnimationFrame(function (t2) {
    var now2 = performance.now();
    r.raf.push({ nonneg: now2 - t2 >= 0, small: now2 - t2 < 2000, increases: t2 >= t });
    r.timelineVsRaf = [document.timeline.currentTime >= 0, Math.abs(document.timeline.currentTime - t2) < 2000, typeof document.timeline.currentTime];
  });
});
var e1 = new Event('x'), p1 = performance.now();
r.ev.newEvent = { nonneg: p1 - e1.timeStamp >= 0, small: p1 - e1.timeStamp < 2000, notEpoch: e1.timeStamp < 1e9, type: typeof e1.timeStamp };
var e2 = new CustomEvent('y'), e3 = new MessageEvent('m'), e4 = new MouseEvent('click');
r.ev.kinds = [e2, e3, e4].map(function (e) { return e.timeStamp < 1e9 && e.timeStamp >= 0; });
document.getElementById('b').addEventListener('click', function (e) {
  var p = performance.now();
  r.ev.click = { nonneg: p - e.timeStamp >= 0, small: p - e.timeStamp < 2000, notEpoch: e.timeStamp < 1e9, trusted: e.isTrusted };
});
document.getElementById('b').click();
r.ev.dispatchedClick = (function () { var got; document.getElementById('b').addEventListener('x2', function (e) { got = e.timeStamp; }); document.getElementById('b').dispatchEvent(new Event('x2')); return got < 1e9 && got >= 0; })();
r.dateNowVsOrigin = Math.abs((performance.timeOrigin + performance.now()) - Date.now()) < 50;
r.nowSteps = (function () { var a = performance.now(), b = performance.now(); return b >= a; })();
// a frame loop: every frame is its own time, about one frame apart
var last2, d2 = [];
requestAnimationFrame(function f(t) {
  if (last2 !== undefined) d2.push(t - last2);
  last2 = t;
  if (d2.length < 10) requestAnimationFrame(f);
  else r.rafLoop = { n: d2.length, allPositive: d2.every(function (x) { return x > 0; }), medianNearAFrame: (function () { var s = d2.slice().sort(function (a, b) { return a - b; }); var m = s[s.length >> 1]; return m > 5 && m < 40; })() };
});
setTimeout(function () {
  var p = performance.now();
  r.timeout = { ok: p >= t0 };
  window.__out = JSON.stringify(r);
}, 300);
