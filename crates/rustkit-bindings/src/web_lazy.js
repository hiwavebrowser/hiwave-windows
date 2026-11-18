// Lazy installation for a nested page's realm (`DomBindings::new_child`).
//
// A child realm installs every page script again, and that is most of what
// a frame costs to set up (2026-11-12: ~97 ms of a child page's ~97 ms in
// the test build). Four interfaces most frames never touch are installed
// here on first use instead: Intl (and the toLocale*String methods over
// it), Streams, Web Animations, and the canvas 2D context. Until then each
// entry point is a stand-in that installs the real script in this realm
// and answers through it, so script sees the same values either way.
//
// The sources come in `__rustkit_lazy_sources` (taken and deleted here);
// each runs as the eager install would have run it. What this file needs
// to do that is taken now, at realm creation, before any page script: the
// built-ins it calls. The natives the scripts work through are on the host
// object, never on the window. A lazily installed script is not given the
// host object itself: it gets an object with the natives it needs and
// nothing else.
(function (g) {
    var SOURCES = g.__rustkit_lazy_sources;
    delete g.__rustkit_lazy_sources;
    if (!SOURCES) return;
    var Fn = Function, gOPD = Object.getOwnPropertyDescriptor, dP = Object.defineProperty,
        apply = Reflect.apply, create = Object.create, setProto = Object.setPrototypeOf;
    function bare(o) { return o ? setProto(o, null) : o; }
    var reporting = create(null);
    // On this record the reporter finds the realm's `error` event too.
    reporting.reportError = __rk.reportError;
    reporting.fireError = function (value, text, path, line, column) {
        return typeof __rk.fireError === 'function' ? __rk.fireError(value, text, path, line, column) : undefined;
    };
    reporting.unsupported = __rk.unsupported;
    // What each script is handed in place of the host object, made when
    // it installs from what was taken now: the natives it works through,
    // which live on the host object and are never on the window, and for
    // the canvas script the built-ins it keeps its records with.
    var host = __rk, wmProto = g.WeakMap.prototype, builtins = create(null);
    builtins.WeakMap = g.WeakMap; builtins.wmGet = wmProto.get; builtins.wmSet = wmProto.set;
    builtins.wmHas = wmProto.has; builtins.wmDelete = wmProto.delete;
    builtins.defineProperty = dP; builtins.create = create;
    builtins.trunc = g.Math.trunc; builtins.abs = g.Math.abs; builtins.isFinite = g.isFinite;
    builtins.String = g.String; builtins.TypeError = g.TypeError; builtins.RangeError = g.RangeError;
    builtins.apply = apply;
    builtins.bufferOf = gOPD(g.Object.getPrototypeOf(g.Uint8ClampedArray.prototype), 'buffer').get;
    var resizable = gOPD(g.ArrayBuffer.prototype, 'resizable');
    builtins.resizableOf = resizable && resizable.get;
    function hostFor(key) {
        var h = create(null);
        if (key === 'animations' || key === 'streams') {
            h.reportError = reporting.reportError;
            h.unsupported = reporting.unsupported;
        }
        if (key === 'animations') {
            h.animatedStyle = host.animatedStyle;
            // The realm's page clock, as an eagerly installed script reads it: the timers' clock,
            // the document timeline (TIMELINE1) and this document's origin on the tree's clock
            // (TIMELINE2). Without them a frame's timeline was its last frame's stamp in every
            // task, and its CSS animations' start times were off by its origin.
            h.now = host.now;
            h.timelineNow = host.timelineNow;
            h.timeOrigin = host.timeOrigin;
        }
        if (key === 'intl') h.likelySubtags = host.likelySubtags;
        if (key === 'canvas') {
            h.pixelsOut = host.pixelsOut;
            h.pixelsIn = host.pixelsIn;
            h.canvasCall = host.canvasCall;
            h.canvasSlot = host.canvasSlot;
            h.builtins = builtins;
        }
        return h;
    }

    function lazy(key, globals, methods, accessors) {
        var src = SOURCES[key], done = false, undo = [];
        if (typeof src !== 'string') return;
        function install() {
            if (done) return;
            done = true;
            for (var i = undo.length - 1; i >= 0; i--) undo[i]();
            // The script's names for the global object are parameters too:
            // what a page made of `globalThis`, `window` or `self` is not asked.
            var run = Fn('__rk', 'globalThis', 'window', 'self', src);
            apply(run, g, bare([hostFor(key), g, g, g]));
        }
        function keep(obj, name) {
            var had = bare(gOPD(obj, name));
            undo[undo.length] = (function () {
                if (had) dP(obj, name, had);
                else delete obj[name];
            });
        }
        globals.forEach(function (name) {
            if (gOPD(g, name)) return; // someone defined it already: leave it
            keep(g, name);
            dP(g, name, {
                get: function () { install(); return g[name]; },
                set: function (v) { install(); g[name] = v; },
                enumerable: false, configurable: true
            });
        });
        methods.forEach(function (m) {
            var obj = m[0], name = m[1];
            if (!obj) return;
            keep(obj, name);
            dP(obj, name, {
                value: function () { install(); return apply(obj[name], this, arguments); },
                writable: true, enumerable: false, configurable: true
            });
        });
        (accessors || []).forEach(function (a) {
            var obj = a[0], name = a[1];
            if (!obj) return;
            keep(obj, name);
            dP(obj, name, {
                get: function () { install(); return this[name]; },
                set: function (v) { install(); this[name] = v; },
                enumerable: true, configurable: true
            });
        });
    }

    var D = g.Date && g.Date.prototype, N = g.Number && g.Number.prototype,
        B = g.BigInt && g.BigInt.prototype, S = g.String && g.String.prototype,
        A = g.Array && g.Array.prototype;
    lazy('intl', ['Intl'], [
        [N, 'toLocaleString'], [B, 'toLocaleString'], [D, 'toLocaleString'],
        [D, 'toLocaleDateString'], [D, 'toLocaleTimeString'], [S, 'localeCompare'], [A, 'toLocaleString']
    ]);
    lazy('streams', ['ReadableStream', 'ReadableStreamDefaultReader', 'ReadableStreamDefaultController',
        'WritableStream', 'WritableStreamDefaultWriter', 'WritableStreamDefaultController',
        'TransformStream', 'TransformStreamDefaultController', 'CountQueuingStrategy',
        'ByteLengthQueuingStrategy', 'TextEncoderStream', 'TextDecoderStream'], []);
    var E = g.Element && g.Element.prototype, Doc = g.Document && g.Document.prototype;
    lazy('animations', ['Animation', 'KeyframeEffect', 'AnimationEffect', 'AnimationTimeline',
        'DocumentTimeline', 'AnimationPlaybackEvent', 'CSSAnimation', 'CSSTransition'],
        [[E, 'animate'], [E, 'getAnimations'], [Doc, 'getAnimations'], [g.document, 'getAnimations']],
        [[g.document, 'timeline']]);
    var C = g.HTMLCanvasElement && g.HTMLCanvasElement.prototype;
    lazy('canvas', ['CanvasRenderingContext2D', 'CanvasGradient', 'CanvasPattern', 'TextMetrics', 'ImageData'],
        [[C, 'getContext'], [C, 'toDataURL'], [C, 'toBlob']],
        [[C, 'width'], [C, 'height']]);
})(globalThis);
