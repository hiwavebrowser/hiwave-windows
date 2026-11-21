// baselines/common/parity-freeze.js
//
// Injected into Chromium for parity captures to reduce nondeterminism.
// Intentionally light-weight and compatible with file:// pages.

(() => {
  // Freeze time. The clock is virtual: it reads FROZEN_TIME until an
  // animation frame runs, then advances FRAME_MS per frame. Pages that never
  // ask for a frame see exactly the old constant clock.
  //
  // What stays real (known leaks): Date.now() between frames does not move
  // with wall time, but how many frames have run by the time a task reads it
  // does; performance.now(), document.timeline and timers are not virtual, so
  // a library timed on performance.now() (GSAP, d3-timer) still runs on wall
  // time. settle() covers its end state if it finishes inside the budget.
  const FROZEN_TIME = 1704067200000; // 2024-01-01T00:00:00Z
  const FRAME_MS = 1000 / 60;
  // A page that keeps asking for frames forever (a spinner, a polling loop)
  // stops getting them here, so its capture is always this frame: 300 frames
  // is 5 s of virtual time, five times Chart.js's default 1 s animation. The
  // budget, not settle()'s wall-clock timeout, must end such a page: 300
  // frames takes about 3 s headless and 5 s at 60 Hz, and settle() waits 30 s.
  const MAX_FRAMES = 300;
  let frames = 0;
  let dropped = 0;
  const frameMs = () => frames * FRAME_MS;
  const nowMs = () => Math.floor(FROZEN_TIME + frameMs());
  const _Date = Date;

  // Preserve constructor behavior but freeze now()/getTime().
  // eslint-disable-next-line no-global-assign
  Date = function (...args) {
    // @ts-ignore
    return args.length ? new _Date(...args) : new _Date(nowMs());
  };
  // @ts-ignore
  Date.UTC = _Date.UTC;
  // @ts-ignore
  Date.parse = _Date.parse;
  // @ts-ignore
  Date.prototype = _Date.prototype;
  // @ts-ignore
  Date.now = nowMs;
  // @ts-ignore
  Date.prototype.getTime = function () {
    return nowMs();
  };

  // Animation frames on the virtual clock. The old freeze called the
  // callback synchronously with a constant timestamp, so a rAF loop either
  // recursed until the stack overflowed (the error was swallowed) or, like
  // Chart.js's animator, never saw time pass: rAF-driven canvases were never
  // drawn (found by #1361 on the report page).
  //
  // Now each frame is one of Chrome's own rendering updates (native rAF), so
  // the HTML order holds: this frame's callbacks, then ResizeObserver
  // deliveries, then paint. Chart.js depends on that order: run its frames
  // ahead of the resize observation and the resized line chart is never
  // drawn. What is virtual is time: every frame advances the clock by exactly
  // FRAME_MS, so a 1 s animation is 60 frames whatever the wall clock does.
  // The callbacks' timestamp is that virtual time on performance.now()'s
  // scale (ms since the time origin, frame n at n * FRAME_MS), as in Chrome;
  // Date.now() is the same time on the epoch scale. Callbacks asked for
  // during a frame run in the next one; a cancelled one never runs, even
  // when an earlier callback of the same frame cancels it.
  //
  // Frames take real time (about 100 a second headless), so a capture must
  // wait for them to finish: __parityFreeze.settle() (deterministic.mjs
  // settleAnimationFrames) resolves once no callback has been pending for two
  // native frames. A page still asking at the budget is reported capped.
  const nativeRaf = window.requestAnimationFrame.bind(window);
  let pending = new Map();
  let running = new Map();
  let nextId = 1;
  let scheduled = false;
  const runFrame = () => {
    scheduled = false;
    if (pending.size === 0) return;
    frames += 1;
    const ts = frameMs();
    running = pending;
    pending = new Map();
    // A Map iterator skips entries deleted before their turn, so a callback
    // cancelled by an earlier one of this frame is not called.
    for (const [id, cb] of running) {
      running.delete(id);
      try {
        cb(ts);
      } catch (err) {
        // As in a real frame: report the error, keep running the others.
        try {
          reportError(err);
        } catch (_) {
          // ignore
        }
      }
    }
    running = new Map();
  };

  // eslint-disable-next-line no-global-assign
  requestAnimationFrame = (cb) => {
    if (typeof cb !== 'function') {
      throw new TypeError("Failed to execute 'requestAnimationFrame' on 'Window': parameter 1 is not of type 'Function'.");
    }
    const id = nextId++;
    if (frames < MAX_FRAMES) {
      pending.set(id, cb);
      if (!scheduled) {
        scheduled = true;
        nativeRaf(runFrame);
      }
    } else {
      dropped += 1;
    }
    return id;
  };
  // eslint-disable-next-line no-global-assign
  cancelAnimationFrame = (id) => {
    pending.delete(id);
    running.delete(id);
  };

  // For the capture harness, not the page.
  Object.defineProperty(window, '__parityFreeze', {
    value: Object.freeze({
      frames: () => frames,
      // {frames, settled, capped}: settled is false only if callbacks were
      // still pending at the timeout; capped is true if the page asked for a
      // frame past the budget (it never stopped on its own).
      settle: (timeoutMs = 30000) =>
        new Promise((resolve) => {
          const deadline = performance.now() + timeoutMs;
          let quiet = 0;
          const check = () => {
            quiet = pending.size === 0 ? quiet + 1 : 0;
            if (quiet >= 2) resolve({ frames, settled: true, capped: dropped > 0 });
            else if (performance.now() > deadline) resolve({ frames, settled: false, capped: dropped > 0 });
            else nativeRaf(check);
          };
          nativeRaf(check);
        }),
    }),
    enumerable: false,
  });

  // Disable transitions/animations at runtime too (in case author CSS re-enables).
  // Init scripts run before the document has a root element on file://
  // navigations (session 7: the old unconditional appendChild threw, which
  // also killed the matchMedia shim below — dead code in every capture).
  const injectFreezeStyle = () => {
    if (!document.documentElement || document.querySelector('style[data-parity-freeze]')) return;
    const style = document.createElement('style');
    style.setAttribute('data-parity-freeze', '1');
    style.textContent = `
      *, *::before, *::after {
        transition: none !important;
        animation: none !important;
        animation-play-state: paused !important;
        caret-color: transparent !important;
        scroll-behavior: auto !important;
      }
    `;
    document.documentElement.appendChild(style);
  };
  if (document.documentElement) {
    injectFreezeStyle();
  } else {
    const obs = new MutationObserver(() => {
      if (document.documentElement) {
        injectFreezeStyle();
        obs.disconnect();
      }
    });
    obs.observe(document, { childList: true });
    document.addEventListener('DOMContentLoaded', injectFreezeStyle);
  }

  // Hint reduced motion.
  try {
    const originalMatchMedia = window.matchMedia.bind(window);
    window.matchMedia = (query) => {
      if (typeof query === 'string' && query.includes('prefers-reduced-motion')) {
        return {
          matches: true,
          media: query,
          onchange: null,
          addListener: () => {},
          removeListener: () => {},
          addEventListener: () => {},
          removeEventListener: () => {},
          dispatchEvent: () => false,
        };
      }
      return originalMatchMedia(query);
    };
  } catch (_) {
    // ignore
  }
})();


