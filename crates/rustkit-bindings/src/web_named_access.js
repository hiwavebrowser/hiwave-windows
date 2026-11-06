// Named access on the Window object (HTML §7.2.2.3, WebIDL §3.7.4 named
// properties object). `<div id="target">` makes bare `target` and
// `window.target` the element; several matches give an HTMLCollection.
//
// Why: WPT tests and real pages read elements by id as globals. Without this
// `target.textContent` throws ReferenceError.
//
// How: as in browsers, a WindowProperties object sits on window's prototype
// chain (window -> Window.prototype -> WindowProperties -> EventTarget.prototype).
// It is a Proxy that answers a name only when nothing further up the chain
// has it, by asking the document (dom.rs `HOOKS.windowNamed`) each time, so
// ids added or removed after load are seen. Being on the prototype, it is
// shadowed by window's own properties (var/function/let declarations,
// assignments) and adds nothing to Object.keys(window) or for-in.
//
// Honest limits: an iframe/frame's browsing-context name is not a named
// property (no child navigables), an HTMLCollection result is a snapshot,
// and an object element is exposed whatever its fallback content.
(function (g) {
    var HOOKS = g.__rkDomHooks, Window = g.Window;
    if (!HOOKS || typeof HOOKS.windowNamed !== 'function' || typeof Window !== 'function') return;
    var named = HOOKS.windowNamed, FRAME = named.FRAME;
    var W = Window.prototype;
    var target = Object.create(Object.getPrototypeOf(W));
    Object.defineProperty(target, Symbol.toStringTag, { value: 'WindowProperties', configurable: true });

    // A name only an iframe/frame carries would be that frame's WindowProxy;
    // there are no child navigables, so it stays unresolved and is reported.
    function resolve(key) {
        var v = named(key);
        if (v !== FRAME) return v;
        __rk.unsupported('Window', 'named access (iframe name)');
        return undefined;
    }
    function lookup(t, key) {
        if (typeof key !== 'string' || Reflect.has(t, key)) return undefined;
        return resolve(key);
    }
    var WindowProperties = new Proxy(target, {
        has: function (t, key) {
            return Reflect.has(t, key) || lookup(t, key) !== undefined;
        },
        get: function (t, key, receiver) {
            var v = lookup(t, key);
            return v !== undefined ? v : Reflect.get(t, key, receiver);
        },
        getOwnPropertyDescriptor: function (t, key) {
            var own = Reflect.getOwnPropertyDescriptor(t, key);
            if (own || typeof key !== 'string') return own;
            var v = resolve(key);
            return v === undefined ? undefined : { value: v, writable: true, enumerable: false, configurable: true };
        }
    });
    Object.setPrototypeOf(W, WindowProperties);
})(globalThis);
