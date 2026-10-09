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
// taken when read (not live), and the document is always visible and focused.
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
    value('hasFocus', function hasFocus() { return true; });
    value('exitFullscreen', function exitFullscreen() { return Promise.resolve(); });
    value('adoptNode', function adoptNode(node) {
        if (node === null || typeof node !== 'object' || typeof node.nodeType !== 'number') {
            throw new TypeError("Failed to execute 'adoptNode' on 'Document': parameter 1 is not of type 'Node'.");
        }
        if (node.nodeType === 9) {
            throw new g.DOMException("Failed to execute 'adoptNode' on 'Document': The node provided is a document, which may not be adopted.", 'NotSupportedError');
        }
        if (node.parentNode) node.parentNode.removeChild(node);
        return node;
    });

    // ---- collections: HTMLCollection-shaped snapshots.
    var HC = g.HTMLCollection;
    function collection(items) {
        var c = HC ? Object.create(HC.prototype) : {};
        for (var i = 0; i < items.length; i++) {
            Object.defineProperty(c, i, { value: items[i], enumerable: true, configurable: true });
        }
        Object.defineProperty(c, 'length', { value: items.length, configurable: true });
        return c;
    }
    if (HC && typeof HC.prototype.namedItem !== 'function') {
        Object.defineProperty(HC.prototype, 'namedItem', {
            value: function namedItem(name) {
                name = String(name);
                for (var i = 0; i < this.length; i++) {
                    var el = this[i];
                    if (el && (el.id === name || (el.getAttribute && el.getAttribute('name') === name))) return el;
                }
                return null;
            },
            writable: true, configurable: true
        });
    }
    function byTag(tag) { return Array.prototype.slice.call(document.getElementsByTagName(tag)); }
    getter('forms', function () { return collection(byTag('form')); });
    getter('images', function () { return collection(byTag('img')); });
    getter('scripts', function () { return collection(byTag('script')); });
    getter('embeds', function () { return collection(byTag('embed')); });
    getter('plugins', function () { return collection(byTag('embed')); });
    getter('links', function () {
        return collection(byTag('a').concat(byTag('area')).filter(function (el) { return el.hasAttribute('href'); }));
    });
    getter('anchors', function () {
        return collection(byTag('a').filter(function (el) { return el.hasAttribute('name'); }));
    });
    value('getElementsByName', function getElementsByName(name) {
        name = String(name);
        var all = document.getElementsByTagName('*'), out = [];
        for (var i = 0; i < all.length; i++) if (all[i].getAttribute('name') === name) out.push(all[i]);
        return collection(out);
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
