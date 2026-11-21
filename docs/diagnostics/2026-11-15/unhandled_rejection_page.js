// unhandledrejection / rejectionhandled: what the pinned Chrome does
// (unhandled_rejection_cft148.json). Scenarios run one after another, each in
// its own timer task; each records the order of what it saw as one string.
// `window.__out` is the JSON of all of them once the last has settled.
(function () {
  var out = {}, log = [], seen = [];
  function label(r) {
    if (r === undefined) return 'undefined';
    if (r && typeof r === 'object' && 'message' in r) return r.message;
    return typeof r + ':' + String(r);
  }
  // The probe's own listener, there for every scenario.
  window.addEventListener('unhandledrejection', function (e) { log.push('unhandled(' + label(e.reason) + ')'); seen.push(e); });
  window.addEventListener('rejectionhandled', function (e) { log.push('handled(' + label(e.reason) + ')'); seen.push(e); });

  var scenarios = [];
  // `run` starts the scenario; `settle` ms later `read` (or the log) is recorded.
  function scenario(name, run, read, settle) { scenarios.push({ name: name, run: run, read: read, settle: settle || 60 }); }
  function next() {
    var s = scenarios.shift();
    if (!s) { window.__out = JSON.stringify(out); return; }
    log = []; seen = [];
    var state;
    try { state = s.run(); } catch (e) { log.push('run throws ' + e.name); }
    setTimeout(function () {
      var r;
      try { r = s.read ? s.read(state) : log.join(' '); } catch (e) { r = 'read throws ' + e.name + ' after ' + log.join(' '); }
      out[s.name] = r;
      setTimeout(next, 0);
    }, s.settle);
  }

  scenario('shape', function () {}, function () {
    return ['onunhandledrejection' in window, String(window.onunhandledrejection), 'onrejectionhandled' in window,
            String(window.onrejectionhandled), typeof PromiseRejectionEvent].join(' ');
  }, 1);

  // ---- when: after the task's microtasks, before a timer set in the task
  scenario('order.script', function () {
    log.push('sync');
    Promise.reject(new Error('A'));
    Promise.resolve().then(function () { log.push('micro'); return null; }).then(function () { log.push('micro2'); });
    setTimeout(function () { log.push('timer0'); }, 0);
    log.push('sync-end');
  });
  scenario('order.timer', function () {
    setTimeout(function () {
      log.push('timer');
      Promise.reject(new Error('T'));
      Promise.resolve().then(function () { log.push('micro'); });
      setTimeout(function () { log.push('timer0'); }, 0);
    }, 0);
  });
  scenario('order.raf', function () {
    requestAnimationFrame(function () {
      log.push('raf');
      Promise.reject(new Error('R'));
      Promise.resolve().then(function () { log.push('micro'); });
      setTimeout(function () { log.push('timer0'); }, 0);
      requestAnimationFrame(function () { log.push('raf2'); });
    });
  }, null, 200);
  scenario('order.two', function () {
    Promise.reject(new Error('one'));
    Promise.reject(new Error('two'));
    Promise.reject(new Error('three'));
  });
  scenario('order.betweenEvents', function () {
    function l(e) {
      var r = e.reason.message;
      Promise.resolve().then(function () { log.push('micro-after-' + r); });
      setTimeout(function () { log.push('timer-after-' + r); }, 0);
    }
    window.addEventListener('unhandledrejection', l);
    Promise.reject(new Error('b1'));
    Promise.reject(new Error('b2'));
    setTimeout(function () { window.removeEventListener('unhandledrejection', l); }, 30);
  });
  scenario('order.rejectedInMicrotask', function () {
    Promise.resolve().then(function () { log.push('micro'); throw new Error('M'); });
    setTimeout(function () { log.push('timer0'); }, 0);
  });

  // ---- the event
  scenario('event', function () {
    var err = new Error('E'), p = Promise.reject(err);
    return { err: err, p: p };
  }, function (s) {
    var e = seen[0];
    if (!e) return 'no event';
    return [e.type, e.promise === s.p, e.reason === s.err, 'cancelable:' + e.cancelable, 'bubbles:' + e.bubbles,
            'composed:' + e.composed, 'trusted:' + e.isTrusted, e instanceof PromiseRejectionEvent,
            Object.prototype.toString.call(e), 'target:' + (e.target === window), 'count:' + seen.length].join(' ');
  });
  scenario('reason.none', function () { Promise.reject(); });
  scenario('reason.string', function () { Promise.reject('why'); });
  scenario('reason.asyncThrow', function () { (async function () { throw new Error('async'); })(); });
  scenario('reason.awaitRejected', function () { (async function () { await Promise.reject(new Error('awaited')); })(); });
  scenario('reason.executorThrow', function () { new Promise(function () { throw new Error('executor'); }); });
  scenario('reason.thenThrow', function () { Promise.resolve().then(function () { throw new Error('then'); }); });

  // ---- handled before the event: none
  scenario('handled.sameTask', function () { var p = Promise.reject(new Error('H1')); p.catch(function () { log.push('caught'); }); });
  scenario('handled.thenOnly', function () { var p = Promise.reject(new Error('H2')); p.then(function () {}); });
  scenario('handled.inMicrotask', function () {
    var p = Promise.reject(new Error('H3'));
    Promise.resolve().then(function () { p.catch(function () { log.push('caught'); }); });
  });
  scenario('handled.inLateMicrotask', function () {
    var p = Promise.reject(new Error('H4'));
    Promise.resolve().then(function () {}).then(function () {}).then(function () { p.catch(function () { log.push('caught'); }); });
  });

  // ---- handled after the event: rejectionhandled
  scenario('later.catch', function () {
    var err = new Error('L1'), p = Promise.reject(err);
    setTimeout(function () {
      log.push('attach');
      p.catch(function () { log.push('caught'); });
      Promise.resolve().then(function () { log.push('micro'); });
      setTimeout(function () { log.push('timer0'); }, 0);
      log.push('attached');
    }, 10);
    return { err: err, p: p };
  }, function (s) {
    var e = seen[1];
    return log.join(' ') + ' / ' + (e ? [e.type, e.promise === s.p, e.reason === s.err, 'cancelable:' + e.cancelable,
      'trusted:' + e.isTrusted, e instanceof PromiseRejectionEvent].join(' ') : 'no second event');
  });
  scenario('later.afterPrevented', function () {
    var p = Promise.reject(new Error('L2'));
    function once(e) { e.preventDefault(); window.removeEventListener('unhandledrejection', once); }
    window.addEventListener('unhandledrejection', once);
    setTimeout(function () { p.catch(function () {}); }, 10);
  });
  scenario('later.twoHandlers', function () {
    var p = Promise.reject(new Error('L3'));
    setTimeout(function () { p.catch(function () {}); p.catch(function () {}); }, 10);
  });
  scenario('later.inTheListener', function () {
    Promise.reject(new Error('L4'));
    function once(e) { window.removeEventListener('unhandledrejection', once); e.promise.catch(function () { log.push('caught'); }); }
    window.addEventListener('unhandledrejection', once);
  });

  // ---- preventDefault: the console line is the launcher's to record (pageErrors)
  scenario('prevent', function () {
    function once(e) {
      window.removeEventListener('unhandledrejection', once);
      log.push('before:' + e.defaultPrevented); e.preventDefault(); log.push('after:' + e.defaultPrevented);
    }
    window.addEventListener('unhandledrejection', once);
    Promise.reject(new Error('prevented-P'));
  });
  scenario('prevent.returnFalse', function () {
    window.onunhandledrejection = function (e) { log.push('on:' + e.defaultPrevented); return false; };
    Promise.reject(new Error('prevented-F'));
    setTimeout(function () { log.push('prevented:' + (seen[0] && seen[0].defaultPrevented)); window.onunhandledrejection = null; }, 30);
  });
  scenario('notPrevented', function () { Promise.reject(new Error('reported-N')); });

  // ---- listeners
  scenario('listeners.order', function () {
    function a() { log.push('listener1'); }
    function b() { log.push('listener2'); }
    window.addEventListener('unhandledrejection', a);
    window.onunhandledrejection = function () { log.push('on'); };
    window.addEventListener('unhandledrejection', b);
    Promise.reject(new Error('O'));
    setTimeout(function () {
      window.removeEventListener('unhandledrejection', a); window.removeEventListener('unhandledrejection', b);
      window.onunhandledrejection = null;
    }, 30);
  });
  scenario('listeners.onThis', function () {
    window.onunhandledrejection = function (e) { log.push('this:' + (this === window) + ' arg:' + (e === seen[0]) + ' args:' + arguments.length); };
    Promise.reject(new Error('this'));
    setTimeout(function () { window.onunhandledrejection = null; }, 30);
  });
  scenario('listeners.addedAfterTheRejection', function () {
    Promise.reject(new Error('late-listener'));
    function l() { log.push('late'); window.removeEventListener('unhandledrejection', l); }
    window.addEventListener('unhandledrejection', l);
  });
  scenario('listeners.elsewhere', function () {
    function d() { log.push('document'); }
    function c() { log.push('capture'); }
    document.addEventListener('unhandledrejection', d);
    window.addEventListener('unhandledrejection', c, true);
    document.body.setAttribute('onunhandledrejection', 'window.__bodyAttr = (window.__bodyAttr || 0) + 1');
    Promise.reject(new Error('where'));
    setTimeout(function () {
      log.push('bodyAttr:' + (window.__bodyAttr || 0) + ' windowOn:' + typeof window.onunhandledrejection);
      document.removeEventListener('unhandledrejection', d); window.removeEventListener('unhandledrejection', c, true);
      document.body.removeAttribute('onunhandledrejection');
    }, 30);
  });
  scenario('listeners.throwing', function () {
    function t() { window.removeEventListener('unhandledrejection', t); log.push('throwing'); throw new Error('listener-threw'); }
    function u() { window.removeEventListener('unhandledrejection', u); log.push('next'); }
    window.addEventListener('unhandledrejection', t);
    window.addEventListener('unhandledrejection', u);
    Promise.reject(new Error('X'));
  });
  scenario('listeners.rejecting', function () {
    var n = 0;
    function r() { if (++n > 1) { window.removeEventListener('unhandledrejection', r); return; } log.push('rejects'); Promise.reject(new Error('second')); }
    window.addEventListener('unhandledrejection', r);
    Promise.reject(new Error('first'));
  });
  scenario('dispatchedByScript', function () {
    var e = new PromiseRejectionEvent('unhandledrejection', { promise: Promise.resolve(), reason: 'made', cancelable: true });
    log.push('returned:' + window.dispatchEvent(e) + ' trusted:' + e.isTrusted);
  });

  // ---- many at once: every one is announced, in the task after the one that rejected them
  function counted(run, settle) {
    function quiet(e) { e.preventDefault(); }
    return [function () { window.addEventListener('unhandledrejection', quiet); return run(); }, function () {
      window.removeEventListener('unhandledrejection', quiet);
      var last = seen[seen.length - 1];
      return seen.length + ' first:' + label(seen[0] && seen[0].reason) + ' last:' + label(last && last.reason);
    }, settle || 150];
  }
  function many(name, run, settle) { var c = counted(run, settle); scenario(name, c[0], c[1], c[2]); }
  many('many.100', function () { for (var i = 0; i < 100; i++) Promise.reject(new Error('m' + i)); });
  many('many.300', function () { for (var i = 0; i < 300; i++) Promise.reject(new Error('m' + i)); });
  many('many.inAListener', function () {
    function l(e) {
      if (e.reason.message !== 'seed') return;
      window.removeEventListener('unhandledrejection', l);
      for (var i = 0; i < 100; i++) Promise.reject(new Error('c' + i));
    }
    window.addEventListener('unhandledrejection', l);
    Promise.reject(new Error('seed'));
  });
  many('many.chain', function () {
    var n = 0;
    function l() { if (++n > 40) { window.removeEventListener('unhandledrejection', l); return; } Promise.reject(new Error('link' + n)); }
    window.addEventListener('unhandledrejection', l);
    Promise.reject(new Error('link0'));
  }, 400);

  // ---- a handler added, inside a later listener, to a promise announced before
  scenario('later.handledInAListener', function () {
    var p = Promise.reject(new Error('P'));
    setTimeout(function () {
      function l(e) {
        if (e.reason.message !== 'q1') return;
        window.removeEventListener('unhandledrejection', l);
        p.catch(function () { log.push('caught'); });
      }
      window.addEventListener('unhandledrejection', l);
      Promise.reject(new Error('q1'));
      Promise.reject(new Error('q2'));
    }, 10);
  });

  // ---- a dedicated worker's own rejection (skipped where `window.__noWorkers` is set)
  scenario('worker', function () {
    if (window.__noWorkers) { log.push('not run'); return; }
    var src = "self.addEventListener('unhandledrejection', function (e) {" +
      " postMessage('unhandled(' + e.reason + ') target:' + (e.target === self) + ' cancelable:' + e.cancelable); e.preventDefault(); });" +
      " Promise.reject('w'); setTimeout(function () { postMessage('end'); }, 50);";
    var w = new Worker(URL.createObjectURL(new Blob([src], { type: 'text/javascript' })));
    w.onmessage = function (e) { log.push('worker:' + e.data); if (e.data === 'end') w.terminate(); };
  }, null, 500);

  setTimeout(next, 0);
})();
