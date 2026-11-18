// Document members pages read without feature-testing (HTML §3.1, CSSOM View,
// CSS Font Loading, DOM §4.5): document.location, document.fonts and FontFace,
// the document's collections (forms, images, links, scripts, anchors, embeds,
// getElementsByName), visibilityState/hidden, characterSet, compatMode,
// contentType, doctype, hasFocus, fullscreen*, adoptNode, document.implementation.
//
// Why: the Windows interactive board's script logs showed pages dying in
// bootstrap on `const { pathname } = document.location` and
// `document.fonts.ready.then(...)`: the member was undefined, destructuring or
// calling through it threw, and the rest of the page's script went with it.
// Each name is only defined when nothing has defined it.
//
// Honest limits: document.fonts never loads a font (FontFace.load() resolves,
// the face is not installed in the engine), the collections are snapshots
// taken when read (not live; their named properties are of that snapshot),
// and the document is always visible and focused.
(function (g) {
    var document = g.document;
    if (!document || !g.Document) return;
    var D = g.Document.prototype;

    function define(obj, name, descriptor) {
        if (Object.prototype.hasOwnProperty.call(obj, name) || obj[name] !== undefined) return;
        descriptor.configurable = true;
        if (descriptor.enumerable === undefined) descriptor.enumerable = true;
        Object.defineProperty(obj, name, descriptor);
    }
    function getter(name, fn, set) { define(D, name, { get: fn, set: set }); }
    function value(name, fn) { define(D, name, { value: fn, writable: true }); }

    // ---- document.location is window.location; assigning navigates.
    getter('location', function () { return g.location; }, function (v) { g.location.href = String(v); });

    // ---- document.defaultView is window (HTML §3.1.2)
    getter('defaultView', function () { return g; });

    // ---- state
    getter('visibilityState', function () { return 'visible'; });
    getter('hidden', function () { return false; });
    getter('webkitVisibilityState', function () { return 'visible'; });
    getter('webkitHidden', function () { return false; });
    getter('characterSet', function () { return 'UTF-8'; });
    getter('charset', function () { return 'UTF-8'; });
    getter('inputEncoding', function () { return 'UTF-8'; });
    getter('contentType', function () { return 'text/html'; });
    // DocumentType: the doctype node wrapper gets this prototype when read.
    if (typeof g.DocumentType !== 'function') {
        var DocumentType = function DocumentType() { throw new TypeError('Illegal constructor'); };
        Object.setPrototypeOf(DocumentType.prototype, g.Node.prototype);
        Object.defineProperty(DocumentType.prototype, Symbol.toStringTag, { value: 'DocumentType', configurable: true });
        Object.defineProperty(DocumentType.prototype, 'name', { get: function () { return this.nodeName; }, configurable: true, enumerable: true });
        Object.defineProperty(DocumentType.prototype, 'publicId', { get: function () { return ''; }, configurable: true, enumerable: true });
        Object.defineProperty(DocumentType.prototype, 'systemId', { get: function () { return ''; }, configurable: true, enumerable: true });
        Object.defineProperty(g, 'DocumentType', { value: DocumentType, writable: true, configurable: true, enumerable: false });
    }
    getter('doctype', function () {
        var kids = document.childNodes;
        for (var i = 0; i < kids.length; i++) {
            if (kids[i].nodeType === 10) {
                if (Object.getPrototypeOf(kids[i]) === g.Node.prototype) Object.setPrototypeOf(kids[i], g.DocumentType.prototype);
                return kids[i];
            }
        }
        return null;
    });
    getter('compatMode', function () { return document.doctype ? 'CSS1Compat' : 'BackCompat'; });
    getter('fullscreenElement', function () { return null; });
    getter('fullscreenEnabled', function () { return false; });
    getter('webkitFullscreenElement', function () { return null; });
    getter('pictureInPictureElement', function () { return null; });
    getter('pointerLockElement', function () { return null; });
    // HTML §6.6.3: whether the document's window has the system focus. The
    // engine says when its view lost or regained it (`set_view_focused`); a
    // view that was never told is taken as focused, as a headless Chrome is.
    value('hasFocus', function hasFocus() { return __rk.viewFocused !== false; });
    value('exitFullscreen', function exitFullscreen() { return Promise.resolve(); });
    value('adoptNode', function adoptNode(node) {
        if (node === null || typeof node !== 'object' || typeof node.nodeType !== 'number') {
            throw new TypeError("Failed to execute 'adoptNode' on 'Document': parameter 1 is not of type 'Node'.");
        }
        if (node.nodeType === 9) {
            throw new g.DOMException("Failed to execute 'adoptNode' on 'Document': The node provided is a document, which may not be adopted.", 'NotSupportedError');
        }
        if (node.parentNode) node.parentNode.removeChild(node);
        // The node is this document's now (its scripting state with it).
        if (typeof __rk.adoptTo === 'function') __rk.adoptTo(node, this);
        return node;
    });

    // ---- collections: HTMLCollection-shaped snapshots, with an
    // HTMLCollection's named properties (DOM §4.2.10.2): `document.forms.login`,
    // `document.forms["benchmark-options"]`.
    //
    // As the pinned Chrome answers (CfT 148,
    // docs/diagnostics/2026-11-15/named_collections_cft148.json):
    //   - a name is an element's id, or the `name` attribute of an HTML
    //     element; a match by id anywhere in the collection comes before a
    //     match by name, and the first in tree order wins among each;
    //   - a name that is an array index is never a named property, an empty
    //     name is none, and a name the interface or Object.prototype already
    //     has stays theirs (`length`, `item`, `toString`);
    //   - a named property is an own property that is read-only and not
    //     enumerable, and cannot be deleted or redefined.
    //
    // Cost: the names are read when the collection is made, an id and a
    // name per member (two attribute reads, no tree walk); reading an index
    // or `length` of a collection in hand costs what it did. Each read of
    // `document.forms` makes a new snapshot, as before, so a name is looked
    // up in the document as it is at that read.
    //
    // The built-ins used are taken now: the lookup is the same whatever a
    // page replaces later, and the descriptors inherit from nothing.
    var HC = g.HTMLCollection, NL = g.NodeList;
    var uncurry = Function.prototype.bind.bind(Function.prototype.call);
    var defProp = Object.defineProperty, objCreate = Object.create, toText = String;
    var E = g.Element.prototype;
    var getAttr = uncurry(E.getAttribute), tagged = uncurry(D.getElementsByTagName);
    var codeAt = uncurry(String.prototype.charCodeAt);
    var nsDesc = Object.getOwnPropertyDescriptor(E, 'namespaceURI');
    var nsOf = nsDesc && typeof nsDesc.get === 'function' ? uncurry(nsDesc.get) : null;
    function isHtml(el) { return !nsOf || nsOf(el) === 'http://www.w3.org/1999/xhtml'; }
    // "0", "17": an array index as a property key is written. Not "01", "-1", "1e3".
    function isIndex(k) {
        var n = k.length;
        if (n === 0 || n > 10) return false;
        for (var i = 0; i < n; i++) {
            var c = codeAt(k, i);
            if (c < 48 || c > 57) return false;
        }
        return n === 1 || codeAt(k, 0) !== 48;
    }
    function slot(value, enumerable, configurable) {
        var d = objCreate(null);
        d.value = value; d.writable = false; d.enumerable = enumerable; d.configurable = configurable;
        return d;
    }
    function nameable(c, k) { return typeof k === 'string' && k !== '' && !isIndex(k) && !(k in c); }
    // `items` is a table of the engine's own: slots 0..n-1 and its count `n`.
    function collection(items, proto, named) {
        var c = objCreate(proto), n = items.n, i, k;
        for (i = 0; i < n; i++) defProp(c, i, slot(items[i], true, true));
        defProp(c, 'length', slot(n, false, true));
        if (!named) return c;
        for (i = 0; i < n; i++) {
            k = getAttr(items[i], 'id');
            if (nameable(c, k)) defProp(c, k, slot(items[i], false, false));
        }
        for (i = 0; i < n; i++) {
            if (!isHtml(items[i])) continue;
            k = getAttr(items[i], 'name');
            if (nameable(c, k)) defProp(c, k, slot(items[i], false, false));
        }
        return c;
    }
    if (HC) {
        // namedItem (DOM §4.2.10.2), with Chrome's order: an id first, then
        // an HTML element's name; the empty string names nothing.
        defProp(HC.prototype, 'namedItem', {
            value: function namedItem(name) {
                name = toText(name);
                if (name === '') return null;
                var n = this.length, i, el;
                for (i = 0; i < n; i++) {
                    el = this[i];
                    if (el && el.nodeType === 1 && getAttr(el, 'id') === name) return el;
                }
                for (i = 0; i < n; i++) {
                    el = this[i];
                    if (el && el.nodeType === 1 && isHtml(el) && getAttr(el, 'name') === name) return el;
                }
                return null;
            },
            writable: true, configurable: true, enumerable: true
        });
    }
    // The document's elements of `tag` that have the attribute `attr` (any,
    // when it is not given), added to the table `out`.
    function gather(out, tag, attr) {
        var all = tagged(document, tag), n = all.length;
        for (var i = 0; i < n; i++) {
            if (attr && getAttr(all[i], attr) === null) continue;
            out[out.n++] = all[i];
        }
        return out;
    }
    function table() { var t = objCreate(null); t.n = 0; return t; }
    function elements(tag, attr) { return collection(gather(table(), tag, attr), HC ? HC.prototype : null, true); }
    getter('forms', function () { return elements('form'); });
    getter('images', function () { return elements('img'); });
    getter('scripts', function () { return elements('script'); });
    getter('embeds', function () { return elements('embed'); });
    getter('plugins', function () { return elements('embed'); });
    getter('links', function () {
        return collection(gather(gather(table(), 'a', 'href'), 'area', 'href'), HC ? HC.prototype : null, true);
    });
    getter('anchors', function () { return elements('a', 'name'); });
    // A NodeList (DOM §4.5), which has no named properties.
    value('getElementsByName', function getElementsByName(name) {
        name = toText(name);
        var all = tagged(document, '*'), n = all.length, out = table();
        for (var i = 0; i < n; i++) if (getAttr(all[i], 'name') === name) out[out.n++] = all[i];
        return collection(out, NL ? NL.prototype : (HC ? HC.prototype : null), false);
    });

    // ---- document.fonts (CSS Font Loading §4): the set exists, resolves, and
    // holds what a page adds; no font is loaded or installed.
    function EventTargetLike() {
        var listeners = {};
        return {
            add: function (type, cb) { (listeners[type] || (listeners[type] = [])).push(cb); },
            remove: function (type, cb) {
                var l = listeners[type]; if (!l) return;
                var i = l.indexOf(cb); if (i >= 0) l.splice(i, 1);
            },
            fire: function (self, type) {
                (listeners[type] || []).slice().forEach(function (cb) { try { cb.call(self, { type: type, target: self }); } catch (_) {} });
            }
        };
    }
    if (typeof g.FontFace !== 'function') {
        var FontFace = function FontFace(family, source, descriptors) {
            if (!(this instanceof FontFace)) throw new TypeError("Failed to construct 'FontFace': Please use the 'new' operator.");
            if (arguments.length < 2) throw new TypeError("Failed to construct 'FontFace': 2 arguments required, but only " + arguments.length + " present.");
            var d = descriptors || {};
            this.family = String(family);
            this.style = d.style === undefined ? 'normal' : String(d.style);
            this.weight = d.weight === undefined ? 'normal' : String(d.weight);
            this.stretch = d.stretch === undefined ? 'normal' : String(d.stretch);
            this.unicodeRange = d.unicodeRange === undefined ? 'U+0-10FFFF' : String(d.unicodeRange);
            this.variant = d.variant === undefined ? 'normal' : String(d.variant);
            this.featureSettings = d.featureSettings === undefined ? 'normal' : String(d.featureSettings);
            this.display = d.display === undefined ? 'auto' : String(d.display);
            this.status = 'unloaded';
            var self = this;
            this.loaded = new Promise(function (resolve) { self._resolve = resolve; });
        };
        FontFace.prototype.load = function load() {
            if (this.status !== 'loaded') { this.status = 'loaded'; this._resolve(this); }
            return this.loaded;
        };
        Object.defineProperty(FontFace.prototype, Symbol.toStringTag, { value: 'FontFace', configurable: true });
        Object.defineProperty(g, 'FontFace', { value: FontFace, writable: true, configurable: true, enumerable: false });
    }
    if (typeof g.FontFaceSet !== 'function') {
        var FontFaceSet = function FontFaceSet() { throw new TypeError('Illegal constructor'); };
        Object.defineProperty(g, 'FontFaceSet', { value: FontFaceSet, writable: true, configurable: true, enumerable: false });
        Object.defineProperty(FontFaceSet.prototype, Symbol.toStringTag, { value: 'FontFaceSet', configurable: true });
        var faces = [], ev = EventTargetLike();
        var set = Object.create(FontFaceSet.prototype);
        var P = FontFaceSet.prototype;
        function m(name, fn) { Object.defineProperty(P, name, { value: fn, writable: true, configurable: true, enumerable: true }); }
        Object.defineProperty(P, 'size', { get: function () { return faces.length; }, configurable: true, enumerable: true });
        Object.defineProperty(P, 'status', { get: function () { return 'loaded'; }, configurable: true, enumerable: true });
        Object.defineProperty(P, 'ready', { get: function () { return Promise.resolve(set); }, configurable: true, enumerable: true });
        m('add', function add(face) { if (faces.indexOf(face) < 0) faces.push(face); return this; });
        m('delete', function (face) { var i = faces.indexOf(face); if (i < 0) return false; faces.splice(i, 1); return true; });
        m('clear', function clear() { faces.length = 0; });
        m('has', function has(face) { return faces.indexOf(face) >= 0; });
        m('forEach', function forEach(cb, thisArg) { faces.slice().forEach(function (f) { cb.call(thisArg, f, f, set); }); });
        m('values', function values() { return faces.slice()[Symbol.iterator](); });
        m('keys', function keys() { return faces.slice()[Symbol.iterator](); });
        m('entries', function entries() { return faces.map(function (f) { return [f, f]; })[Symbol.iterator](); });
        P[Symbol.iterator] = P.values;
        // check(): everything is "available" (nothing is being loaded).
        m('check', function check() { return true; });
        m('load', function load(font) {
            return Promise.resolve(faces.filter(function (f) {
                return String(font).indexOf(f.family) >= 0;
            }));
        });
        m('addEventListener', function (type, cb) { ev.add(String(type), cb); });
        m('removeEventListener', function (type, cb) { ev.remove(String(type), cb); });
        getter('fonts', function () { return set; });
    }

    // ---- document.implementation (DOM §4.5)
    var DI = g.DOMImplementation;
    if (typeof DI !== 'function') {
        DI = function DOMImplementation() { throw new TypeError('Illegal constructor'); };
        Object.defineProperty(DI.prototype, Symbol.toStringTag, { value: 'DOMImplementation', configurable: true });
        Object.defineProperty(g, 'DOMImplementation', { value: DI, writable: true, configurable: true, enumerable: false });
    }
    var DIP = DI.prototype;
    function defDI(name, fn) {
        if (typeof DIP[name] !== 'function') {
            Object.defineProperty(DIP, name, { value: fn, writable: true, configurable: true, enumerable: true });
        }
    }
    defDI('hasFeature', function hasFeature() { return true; });
    defDI('createDocumentType', function createDocumentType(qualifiedName, publicId, systemId) {
        var dt = Object.create(g.DocumentType ? g.DocumentType.prototype : (g.Node ? g.Node.prototype : Object.prototype));
        Object.defineProperty(dt, 'nodeType', { value: 10, configurable: true, enumerable: true });
        Object.defineProperty(dt, 'nodeName', { value: String(qualifiedName), configurable: true, enumerable: true });
        Object.defineProperty(dt, 'name', { value: String(qualifiedName), configurable: true, enumerable: true });
        Object.defineProperty(dt, 'publicId', { value: String(publicId || ''), configurable: true, enumerable: true });
        Object.defineProperty(dt, 'systemId', { value: String(systemId || ''), configurable: true, enumerable: true });
        return dt;
    });
    defDI('createHTMLDocument', function createHTMLDocument(title) {
        var html = '<!DOCTYPE html><html><head>';
        if (title !== undefined) {
            html += '<title>' + String(title).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;') + '</title>';
        }
        html += '</head><body></body></html>';
        return new g.DOMParser().parseFromString(html, 'text/html');
    });
    defDI('createDocument', function createDocument(ns, qname, doctype) {
        var html = '<!DOCTYPE html><html><head></head><body></body></html>';
        return new g.DOMParser().parseFromString(html, 'text/html');
    });
    var impl = Object.create(DIP);
    getter('implementation', function () { return impl; });
})(typeof globalThis === 'object' ? globalThis : this);
