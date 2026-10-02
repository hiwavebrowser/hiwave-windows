// baselines/common/parity-freeze.js
//
// Injected into Chromium for parity captures to reduce nondeterminism.
// Intentionally light-weight and compatible with file:// pages.

(() => {
  // Freeze time.
  const FROZEN_TIME = 1704067200000; // 2024-01-01T00:00:00Z
  const _Date = Date;

  // Preserve constructor behavior but freeze now()/getTime().
  // eslint-disable-next-line no-global-assign
  Date = function (...args) {
    // @ts-ignore
    return args.length ? new _Date(...args) : new _Date(FROZEN_TIME);
  };
  // @ts-ignore
  Date.UTC = _Date.UTC;
  // @ts-ignore
  Date.parse = _Date.parse;
  // @ts-ignore
  Date.prototype = _Date.prototype;
  // @ts-ignore
  Date.now = () => FROZEN_TIME;
  // @ts-ignore
  Date.prototype.getTime = function () {
    return FROZEN_TIME;
  };

  // Freeze RAF to a constant timestamp (best-effort).
  const frozenRaf = (cb) => {
    try {
      cb(FROZEN_TIME);
    } catch (_) {
      // ignore
    }
    return 1;
  };

  // eslint-disable-next-line no-global-assign
  requestAnimationFrame = frozenRaf;

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


