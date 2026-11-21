// Nested browsing contexts, Stage 2 (docs/diagnostics/2026-11-11/
// nested-browsing-contexts-plan.md): the page half of `<iframe>`.
//
// Installed by `DomBindings::enable_frames`, which the engine calls only with
// `EngineConfig::nested_contexts` on. Without it nothing here exists and an
// iframe's `contentWindow`/`contentDocument` stay `null` (web_reflect.js).
//
// What it does: tells the engine which frame to load what (an iframe going
// into the document, by a node insertion or as parsed markup (`innerHTML`),
// its `src` set while it is in, it leaving), and answers
// `contentWindow` / `contentDocument` with the window the engine attached.
// The engine does the fetching, the origin check, the child realm, the
// child's scripts and the `load` event (`__rk.frames.loaded`).
//
// Stated limits of this stage: `contentWindow` is `null` until the engine
// has drained the request (after the script that inserted the frame
// returns; HTML makes it `about:blank` at once). The blank document's
// `load` comes with it, a turn late, and goes only to what could hear it
// at the insertion, where Chrome fires it: a listener added or a handler
// set after the insertion never hears the blank `load` (`navigate`).
// A cross-origin frame is
// left blank: `contentWindow` and `contentDocument` stay `null`, and `load`
// fires. A frame with a `sandbox` attribute is left blank the same way: the
// engine reads the attribute itself when the request below is made
// (`FrameRequest::sandbox`), nothing here does.
// `srcdoc`, `name` targeting and `window.frames[i]` are not here.
(function (g) {
    var HOOKS = __rk.domHooks;
    var request = g.__rustkit_frame_request;
    // Taken off the window first, whatever follows.
    delete g.__rustkit_frame_request;
    if (!HOOKS || !HOOKS.nodeId || typeof request !== 'function' || __rk.frames) return;
    // Taken now, before page script: the engine's side below (the scan,
    // a frame's size) calls only these and plain loops.
    var URL_ = g.URL, doc = g.document;
    var uncurry = Function.prototype.bind.bind(Function.prototype.call);
    var getProto = Object.getPrototypeOf, gOPD = Object.getOwnPropertyDescriptor, create = Object.create;
    // The getter of an accessor found on `o` or its prototypes, uncurried.
    function getterOf(o, name) {
        for (; o; o = getProto(o)) {
            var d = gOPD(o, name);
            if (d) return typeof d.get === 'function' ? uncurry(d.get) : null;
        }
        return null;
    }
    function methodOf(o, name) { return o && typeof o[name] === 'function' ? uncurry(o[name]) : null; }
    var E = g.Element && g.Element.prototype;
    var docAll = methodOf(doc, 'querySelectorAll'), elAll = methodOf(E, 'querySelectorAll'),
        getAttr = methodOf(E, 'getAttribute'), connected = getterOf(g.Node && g.Node.prototype, 'isConnected'),
        baseURI = getterOf(doc, 'baseURI'), trim = uncurry(String.prototype.trim),
        exec = uncurry(RegExp.prototype.exec), round = Math.round, toFloat = parseFloat, toInt = parseInt;
    var frames = create(null); // node id -> { el, win }
    function isConnected(n) { return connected ? connected(n) : n.isConnected; }
    function record(el) { var r = create(null); r.el = el; r.win = null; return r; }

    function isFrame(n) {
        return n != null && n.nodeType === 1 && n.localName === 'iframe';
    }
    function each(n, f) {
        if (isFrame(n)) return f(n);
        if (n == null) return;
        var list = n === doc && docAll ? docAll(n, 'iframe')
            : n.nodeType === 1 && elAll ? elAll(n, 'iframe')
            : typeof n.querySelectorAll === 'function' ? n.querySelectorAll('iframe') : null;
        for (var i = 0; list && i < list.length; i++) f(list[i]);
    }
    // The URL the frame is to show: its `src` resolved, or about:blank.
    function target(el) {
        var src = getAttr(el, 'src');
        if (src === null || trim(src) === '') return 'about:blank';
        try { return new URL_(src, (baseURI ? baseURI(doc) : doc.baseURI) || g.location.href).href; } catch (e) { return 'about:blank'; }
    }
    // `inserted`: a script just put the frame into the document; `parsed`:
    // the parser did, and the scan found it. Chrome fires the initial
    // about:blank's `load` inside the insertion, so only what was on the
    // frame by then hears it. The engine's comes after the inserting script
    // returns: what would hear it is recorded now (`HOOKS.loadSnapshot`)
    // and the `load` is replayed to that record (`loaded`), or not asked
    // for at all when the record is empty.
    var NORMAL = 0, QUIET = 1, INITIAL = 2;
    var heardAt = create(null); // node id -> what could hear the blank load
    function navigate(el, inserted, parsed) {
        var id = HOOKS.nodeId(el);
        if (id < 0) return;
        var url = target(el), mode = NORMAL;
        frames[id] = record(el);
        // A later `src` leaves the record: the blank request made before
        // it is still to be answered.
        if ((inserted || parsed) && url === 'about:blank' && HOOKS.loadSnapshot) {
            var snap = HOOKS.loadSnapshot(el, !!parsed);
            if (snap) { heardAt[id] = snap; mode = INITIAL; } else { delete heardAt[id]; mode = QUIET; }
        }
        request(id, url, mode);
    }
    function gone(el) {
        var id = HOOKS.nodeId(el);
        if (id < 0 || !frames[id]) return;
        delete frames[id];
        delete heardAt[id];
        request(id, null);
    }

    // A frame that is in the document and has not been asked for yet.
    // `inserted` as for `navigate`.
    function fresh(f, inserted) {
        var id = HOOKS.nodeId(f);
        if (id >= 0 && !frames[id] && isConnected(f)) navigate(f, inserted);
    }
    // The same, for a frame a script's insertion just put in (a fragment's
    // children, parsed markup).
    function arrived(f) { fresh(f, true); }
    // The frames whose element is no longer in the document have left.
    function sweep() {
        for (var id in frames) {
            if (!isConnected(frames[id].el)) { delete frames[id]; delete heardAt[id]; request(+id, null); }
        }
    }
    HOOKS.frames = Object.setPrototypeOf({
        // `parent`: where `node` went. A fragment's children are under it
        // by now, the fragment empty.
        tree: function (op, node, parent) {
            if (op === 'insert' && node != null && node.nodeType === 11) each(parent, arrived);
            else if (op === 'insert') each(node, function (f) { if (isConnected(f)) navigate(f, true); });
            else if (op === 'remove') each(node, gone);
        },
        changed: function (node, op, name) {
            // An element's children replaced by parsed markup or by text:
            // the frames that left, then the frames that came.
            if (op === 'setHTML' || op === 'setText' || op === 'setInnerText') {
                sweep();
                if (op === 'setHTML') each(node, arrived);
                return;
            }
            if (op !== 'setAttr' || String(name).length !== 3) return;
            if (isFrame(node) && isConnected(node) && String(name).toLowerCase() === 'src') navigate(node);
        }
    }, null);

    // The engine's side, on the host object.
    __rk.frames = Object.setPrototypeOf({
        // The parser's iframes, once the document is in.
        scan: function () {
            var list = HOOKS.byTag('iframe');
            for (var i = 0; i < list.length; i++) {
                var f = list[i], id = HOOKS.nodeId(f);
                if (id >= 0 && !frames[id] && isConnected(f)) navigate(f, false, true);
            }
        },
        // The frame takes the window (or, with null, gives it up). What
        // the window is to be told when it took one: this window and the
        // frame's element (the caller adds the tree's top); or null.
        attach: function (id, win) {
            var r = frames[id];
            if (!r) return null;
            r.win = win;
            var h = create(null);
            h.up = g; h.el = r.el; h.top = null;
            return h;
        },
        // `initial`: the document is the blank one of the frame's
        // insertion, whose `load` goes to what was recorded then.
        loaded: function (id, initial) {
            var r = frames[id];
            if (!r) return false;
            // The engine's own event, through its own dispatch: whatever
            // the page has put on the element or the prototypes since.
            var ev = __rk.trust(__rk.make('Event', 'load'));
            if (initial) {
                var snap = heardAt[id];
                delete heardAt[id];
                if (snap && HOOKS.loadReplay) HOOKS.loadReplay(snap, ev);
            } else {
                // A record still held is for a blank document that was
                // never made (a frame left blank by the engine): let it go.
                delete heardAt[id];
                __rk.dispatchTo(r.el, ev);
            }
            return true;
        },
        // The child's viewport until layout sizes frames (Stage 3): the
        // element's own CSS px width/height, its attributes, or 300x150.
        size: function (id) {
            var r = frames[id], w = 300, h = 150;
            if (!r) return '300x150';
            // The inline style is read from the `style` attribute's text.
            var el = r.el, st = getAttr(el, 'style') || '';
            function px(re) { var m = exec(re, st); return m ? round(toFloat(m[1])) : 0; }
            function attr(n) { var v = getAttr(el, n); return v && exec(/^\s*[0-9]+\s*$/, v) ? toInt(v, 10) : 0; }
            w = px(/(?:^|;)\s*width\s*:\s*([0-9.]+)px\s*(?:;|$)/i) || attr('width') || w;
            h = px(/(?:^|;)\s*height\s*:\s*([0-9.]+)px\s*(?:;|$)/i) || attr('height') || h;
            return w + 'x' + h;
        }
    }, null);

    var F = g.HTMLIFrameElement && g.HTMLIFrameElement.prototype;
    if (F) {
        Object.defineProperty(F, 'contentWindow', {
            get: function () { var r = frames[HOOKS.nodeId(this)]; return r && r.win || null; },
            enumerable: true, configurable: true
        });
        Object.defineProperty(F, 'contentDocument', {
            get: function () { var r = frames[HOOKS.nodeId(this)]; return r && r.win ? r.win.document : null; },
            enumerable: true, configurable: true
        });
    }
})(globalThis);
