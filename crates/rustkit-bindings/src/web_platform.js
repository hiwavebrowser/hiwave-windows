// Web platform baseline: the window, screen, performance and navigator
// surface that real pages read without feature-testing. Each name is only
// defined when nothing else has defined it, so a real implementation
// (now or later) always wins over this one.
//
// Why it exists: the Windows seat's live-site script logs showed pages
// dying on the first `screen.width`, `performance.now()`, `window.scrollY`
// or `navigator.hardwareConcurrency` (ReferenceError / TypeError), taking
// the rest of their script with them. A page that gets a plausible value
// carries on; a page that gets an exception does not.
//
// Values are the honest ones for a headless, non-scrolling, single-window
// RustKit view: no scroll offset, one top-level window, no touch, a 24-bit
// screen the size of the viewport.
(function (g) {
    function def(obj, name, value) {
        if (obj[name] === undefined) {
            Object.defineProperty(obj, name, { value: value, writable: true, configurable: true, enumerable: true });
        }
    }
    function getter(obj, name, fn) {
        if (obj[name] === undefined) {
            Object.defineProperty(obj, name, { get: fn, configurable: true, enumerable: true });
        }
    }
    function noop() {}

    // ---- screen (CSSOM View §4.3). The "screen" is the viewport.
    if (typeof g.screen === 'undefined') {
        var screen = {};
        getter(screen, 'width', function () { return g.innerWidth; });
        getter(screen, 'height', function () { return g.innerHeight; });
        getter(screen, 'availWidth', function () { return g.innerWidth; });
        getter(screen, 'availHeight', function () { return g.innerHeight; });
        def(screen, 'availLeft', 0);
        def(screen, 'availTop', 0);
        def(screen, 'colorDepth', 24);
        def(screen, 'pixelDepth', 24);
        def(screen, 'orientation', { type: 'landscape-primary', angle: 0,
            addEventListener: noop, removeEventListener: noop, lock: function () { return Promise.resolve(); },
            unlock: noop });
        g.screen = screen;
    }

    // ---- performance (High Resolution Time, Performance Timeline, Navigation Timing).
    if (typeof g.performance === 'undefined') {
        var origin = Date.now();
        var entries = [];
        var performance = {
            timeOrigin: origin,
            now: function () { return Date.now() - origin; },
            mark: function (name) {
                var e = { name: String(name), entryType: 'mark', startTime: performance.now(), duration: 0 };
                entries.push(e);
                return e;
            },
            measure: function (name, start, end) {
                var t0 = 0, t1 = performance.now();
                entries.forEach(function (e) {
                    if (e.entryType === 'mark' && e.name === start) t0 = e.startTime;
                    if (e.entryType === 'mark' && e.name === end) t1 = e.startTime;
                });
                var m = { name: String(name), entryType: 'measure', startTime: t0, duration: t1 - t0 };
                entries.push(m);
                return m;
            },
            clearMarks: function (name) {
                entries = entries.filter(function (e) { return !(e.entryType === 'mark' && (name === undefined || e.name === name)); });
            },
            clearMeasures: function (name) {
                entries = entries.filter(function (e) { return !(e.entryType === 'measure' && (name === undefined || e.name === name)); });
            },
            clearResourceTimings: noop,
            setResourceTimingBufferSize: noop,
            getEntries: function () { return entries.slice(); },
            getEntriesByType: function (type) { return entries.filter(function (e) { return e.entryType === type; }); },
            getEntriesByName: function (name, type) {
                return entries.filter(function (e) { return e.name === name && (type === undefined || e.entryType === type); });
            },
            toJSON: function () { return { timeOrigin: origin }; },
            navigation: { type: 0, redirectCount: 0 },
            timing: {
                navigationStart: origin, fetchStart: origin, domainLookupStart: origin, domainLookupEnd: origin,
                connectStart: origin, connectEnd: origin, requestStart: origin, responseStart: origin,
                responseEnd: origin, domLoading: origin, domInteractive: origin,
                domContentLoadedEventStart: origin, domContentLoadedEventEnd: origin, domComplete: origin,
                loadEventStart: origin, loadEventEnd: origin
            },
            memory: { jsHeapSizeLimit: 2147483648, totalJSHeapSize: 33554432, usedJSHeapSize: 16777216 }
        };
        g.performance = performance;
    }

    // ---- window: scroll position, screen position, frame tree, small methods.
    ['scrollX', 'scrollY', 'pageXOffset', 'pageYOffset', 'screenX', 'screenY', 'screenLeft', 'screenTop'].forEach(function (k) {
        def(g, k, 0);
    });
    def(g, 'name', '');
    def(g, 'top', g);
    def(g, 'parent', g);
    def(g, 'frames', g);
    def(g, 'opener', null);
    def(g, 'closed', false);
    def(g, 'length', 0);
    def(g, 'status', '');
    def(g, 'crossOriginIsolated', false);
    getter(g, 'origin', function () { return g.location && g.location.origin; });
    getter(g, 'isSecureContext', function () {
        var p = g.location && g.location.protocol;
        return p === 'https:' || p === 'file:' || (g.location && g.location.hostname === 'localhost');
    });
    ['scroll', 'scrollTo', 'scrollBy', 'focus', 'blur', 'print', 'stop', 'close', 'moveTo', 'moveBy',
     'resizeTo', 'resizeBy'].forEach(function (k) { def(g, k, noop); });
    def(g, 'open', function () { return null; });
    def(g, 'find', function () { return false; });
    def(g, 'getSelection', function () {
        return { rangeCount: 0, isCollapsed: true, type: 'None', anchorNode: null, focusNode: null,
                 toString: function () { return ''; }, removeAllRanges: noop, addRange: noop,
                 getRangeAt: function () { return null; }, collapse: noop, empty: noop };
    });

    // ---- navigator: the read-only facts pages branch on.
    var nav = g.navigator;
    if (nav) {
        def(nav, 'hardwareConcurrency', 4);
        def(nav, 'maxTouchPoints', 0);
        def(nav, 'cookieEnabled', true);
        def(nav, 'doNotTrack', null);
        def(nav, 'webdriver', false);
        def(nav, 'vendor', '');
        def(nav, 'vendorSub', '');
        def(nav, 'product', 'Gecko');
        def(nav, 'productSub', '20030107');
        def(nav, 'appName', 'Netscape');
        def(nav, 'appCodeName', 'Mozilla');
        def(nav, 'appVersion', '5.0 (RustKit)');
        def(nav, 'plugins', []);
        def(nav, 'mimeTypes', []);
        def(nav, 'pdfViewerEnabled', false);
        def(nav, 'sendBeacon', function () { return false; });
        def(nav, 'javaEnabled', function () { return false; });
        if (nav.userAgent === undefined) def(nav, 'userAgent', 'RustKit/1.0');
    }
})(globalThis);
