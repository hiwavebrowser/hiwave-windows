// HTML drag and drop (HTML §6.11): a constructible DataTransfer over a drag
// data store with its protection modes, a minimal DataTransferItemList, and
// the hook the engine's drag-and-drop processing model drives
// (`__rkDragStore`, read by dom.rs's `__rkFireDrag`). Evaluated after
// web_interfaces.js, whose interface-only DataTransfer this replaces.
//
// Modes (§6.11.2.2): 'rw' in dragstart (and for `new DataTransfer()`), 'ro'
// in drop, 'protected' everywhere else during a drag (the types are
// visible, the data is not), 'disabled' once the drag is over.
(function (g) {
    var EFFECTS = ['none', 'copy', 'link', 'move'];
    var ALLOWED = ['none', 'copy', 'copyLink', 'copyMove', 'link', 'linkMove', 'move', 'all', 'uninitialized'];
    var STORE = Symbol('rustkit.dragstore');
    function hidden(o, k, v) {
        Object.defineProperty(o, k, { value: v, configurable: true, enumerable: false, writable: true });
    }
    function normalize(format) {
        var f = String(format).toLowerCase();
        if (f === 'text') return 'text/plain';
        if (f === 'url') return 'text/uri-list';
        return f;
    }
    function storeOf(dt) {
        var s = dt && dt[STORE];
        if (!s) throw new TypeError('Illegal invocation');
        return s;
    }
    function notSupported(msg) {
        var D = g.DOMException;
        return D ? new D(msg, 'NotSupportedError') : new Error(msg);
    }

    // ---- DataTransferItem / DataTransferItemList (string items only)
    function DataTransferItem() { throw new TypeError('Illegal constructor'); }
    function makeItem(store, entry) {
        var item = Object.create(DataTransferItem.prototype);
        hidden(item, '__rkStore', store);
        hidden(item, '__rkEntry', entry);
        return item;
    }
    function live(item) {
        var s = item.__rkStore;
        return s.mode !== 'disabled' && s.items.indexOf(item.__rkEntry) >= 0;
    }
    Object.defineProperty(DataTransferItem.prototype, 'kind', {
        get: function () { return live(this) ? this.__rkEntry.kind : ''; }, configurable: true, enumerable: true
    });
    Object.defineProperty(DataTransferItem.prototype, 'type', {
        get: function () { return live(this) ? this.__rkEntry.type : ''; }, configurable: true, enumerable: true
    });
    DataTransferItem.prototype.getAsString = function (callback) {
        if (typeof callback !== 'function' || !live(this)) return;
        var s = this.__rkStore, e = this.__rkEntry;
        if (s.mode !== 'rw' && s.mode !== 'ro') return;
        var data = e.data;
        Promise.resolve().then(function () { callback(data); });
    };
    DataTransferItem.prototype.getAsFile = function () { return null; };
    DataTransferItem.prototype.webkitGetAsEntry = function () { return null; };

    function DataTransferItemList() { throw new TypeError('Illegal constructor'); }
    function syncList(list) {
        var s = list.__rkStore;
        var n = s.mode === 'disabled' ? 0 : s.items.length;
        for (var k in list) {
            if (/^\d+$/.test(k)) delete list[k];
        }
        for (var i = 0; i < n; i++) {
            var e = s.items[i];
            if (!e.item) e.item = makeItem(s, e);
            Object.defineProperty(list, String(i), { value: e.item, configurable: true, enumerable: true });
        }
    }
    Object.defineProperty(DataTransferItemList.prototype, 'length', {
        get: function () { var s = this.__rkStore; return s.mode === 'disabled' ? 0 : s.items.length; },
        configurable: true, enumerable: true
    });
    DataTransferItemList.prototype.add = function (data, type) {
        var s = this.__rkStore;
        if (s.mode !== 'rw') return null;
        if (typeof data !== 'string') return null; // File items are not supported.
        var t = String(type).toLowerCase();
        for (var i = 0; i < s.items.length; i++) {
            if (s.items[i].kind === 'string' && s.items[i].type === t) {
                throw notSupported("Failed to execute 'add' on 'DataTransferItemList': An item already exists for type '" + t + "'.");
            }
        }
        var e = { kind: 'string', type: t, data: data };
        s.items.push(e);
        syncList(this);
        return e.item;
    };
    DataTransferItemList.prototype.remove = function (index) {
        var s = this.__rkStore;
        if (s.mode !== 'rw') {
            var D = g.DOMException;
            throw D ? new D('The data transfer is not writable.', 'InvalidStateError') : new Error('InvalidStateError');
        }
        s.items.splice(index >>> 0, 1);
        syncList(this);
    };
    DataTransferItemList.prototype.clear = function () {
        var s = this.__rkStore;
        if (s.mode !== 'rw') return;
        s.items.length = 0;
        syncList(this);
    };
    DataTransferItemList.prototype.item = function (i) { return this[i] || null; };

    // ---- DataTransfer
    function DataTransfer() {
        if (!(this instanceof DataTransfer)) {
            throw new TypeError("Failed to construct 'DataTransfer': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        }
        var s = { mode: 'rw', items: [], effectAllowed: 'none', dropEffect: 'none' };
        hidden(this, STORE, s);
        var list = Object.create(DataTransferItemList.prototype);
        hidden(list, '__rkStore', s);
        hidden(s, 'list', list);
        var files = [];
        files.item = function (i) { return files[i] || null; };
        Object.freeze(files);
        hidden(s, 'files', files);
    }
    Object.defineProperty(DataTransfer.prototype, Symbol.toStringTag, { value: 'DataTransfer', configurable: true });
    Object.defineProperty(DataTransfer.prototype, 'dropEffect', {
        get: function () { return storeOf(this).dropEffect; },
        set: function (v) { var s = storeOf(this); v = String(v); if (EFFECTS.indexOf(v) >= 0) s.dropEffect = v; },
        configurable: true, enumerable: true
    });
    Object.defineProperty(DataTransfer.prototype, 'effectAllowed', {
        get: function () { return storeOf(this).effectAllowed; },
        set: function (v) {
            var s = storeOf(this); v = String(v);
            if (s.mode === 'rw' && ALLOWED.indexOf(v) >= 0) s.effectAllowed = v;
        },
        configurable: true, enumerable: true
    });
    Object.defineProperty(DataTransfer.prototype, 'items', {
        get: function () { var s = storeOf(this); syncList(s.list); return s.list; },
        configurable: true, enumerable: true
    });
    Object.defineProperty(DataTransfer.prototype, 'files', {
        get: function () { return storeOf(this).files; }, configurable: true, enumerable: true
    });
    Object.defineProperty(DataTransfer.prototype, 'types', {
        get: function () {
            var s = storeOf(this);
            if (s.mode === 'disabled') return Object.freeze([]);
            var out = [];
            s.items.forEach(function (e) { if (e.kind === 'string') out.push(e.type); });
            return Object.freeze(out);
        },
        configurable: true, enumerable: true
    });
    DataTransfer.prototype.getData = function (format) {
        var s = storeOf(this);
        if (s.mode !== 'rw' && s.mode !== 'ro') return '';
        var f = normalize(format), url = String(format).toLowerCase() === 'url';
        for (var i = 0; i < s.items.length; i++) {
            var e = s.items[i];
            if (e.kind === 'string' && e.type === f) {
                if (!url) return e.data;
                // `url`: the first URL of the uri-list (§6.11.3).
                var lines = e.data.split(/\r?\n/);
                for (var j = 0; j < lines.length; j++) {
                    if (lines[j] && lines[j].charAt(0) !== '#') return lines[j];
                }
                return '';
            }
        }
        return '';
    };
    DataTransfer.prototype.setData = function (format, data) {
        var s = storeOf(this);
        if (s.mode !== 'rw') return;
        var f = normalize(format);
        s.items = s.items.filter(function (e) { return !(e.kind === 'string' && e.type === f); });
        s.items.push({ kind: 'string', type: f, data: String(data) });
    };
    DataTransfer.prototype.clearData = function (format) {
        var s = storeOf(this);
        if (s.mode !== 'rw') return;
        if (format === undefined) {
            s.items = s.items.filter(function (e) { return e.kind !== 'string'; });
        } else {
            var f = normalize(format);
            s.items = s.items.filter(function (e) { return !(e.kind === 'string' && e.type === f); });
        }
    };
    // The drag image is the platform's; a custom one is accepted and unused.
    DataTransfer.prototype.setDragImage = function (image, x, y) {
        storeOf(this);
        if (arguments.length < 3) {
            throw new TypeError("Failed to execute 'setDragImage' on 'DataTransfer': 3 arguments required, but only " + arguments.length + ' present.');
        }
    };

    [['DataTransfer', DataTransfer], ['DataTransferItem', DataTransferItem], ['DataTransferItemList', DataTransferItemList]]
        .forEach(function (p) {
            Object.defineProperty(p[1], 'name', { value: p[0] });
            Object.defineProperty(p[1].prototype, Symbol.toStringTag, { value: p[0], configurable: true });
            Object.defineProperty(g, p[0], { value: p[1], writable: true, configurable: true, enumerable: false });
        });

    // The drag operation the dropEffect and effectAllowed allow
    // (§6.11.5, "the current drag operation" after a cancelled dragover).
    function operation(allowed, effect) {
        if (allowed === 'none' || effect === 'none') return 'none';
        var ok = {
            copy: ['uninitialized', 'copy', 'copyLink', 'copyMove', 'all'],
            link: ['uninitialized', 'link', 'copyLink', 'linkMove', 'all'],
            move: ['uninitialized', 'move', 'copyMove', 'linkMove', 'all']
        }[effect];
        return ok && ok.indexOf(allowed) >= 0 ? effect : 'none';
    }
    // The dropEffect a dragenter or dragover starts with (§6.11.5's table).
    function initialEffect(allowed, linkSource) {
        switch (allowed) {
            case 'none': return 'none';
            case 'copy': case 'copyLink': case 'copyMove': case 'all': return 'copy';
            case 'link': return 'link';
            case 'move': case 'linkMove': return 'move';
            default: return linkSource ? 'link' : 'move';
        }
    }

    // The drag in progress: its DataTransfer, and whether its source is a
    // link (for the uninitialized effectAllowed).
    var current = null, linkSource = false;
    hidden(g, '__rkDragStore', {
        // dragstart: a fresh store, with the data the user agent puts in
        // for a link or an image (§6.11.5 step "drag data store item list").
        begin: function (source) {
            var dt = new DataTransfer();
            var s = dt[STORE];
            s.effectAllowed = 'uninitialized';
            var tag = source && source.localName;
            linkSource = false;
            if (tag === 'a' && source.hasAttribute('href')) {
                var href = source.href || source.getAttribute('href');
                s.items.push({ kind: 'string', type: 'text/uri-list', data: href });
                s.items.push({ kind: 'string', type: 'text/plain', data: href });
                linkSource = true;
            } else if (tag === 'img' && source.hasAttribute('src')) {
                var src = source.src || source.getAttribute('src');
                s.items.push({ kind: 'string', type: 'text/uri-list', data: src });
            }
            current = dt;
            return dt;
        },
        // The DataTransfer an event of `type` carries, in the mode and with
        // the dropEffect that event starts with; `op` is the current drag
        // operation.
        prepare: function (type, op) {
            var dt = current;
            if (!dt) return null;
            var s = dt[STORE];
            s.mode = type === 'dragstart' ? 'rw' : type === 'drop' ? 'ro' : 'protected';
            if (type === 'dragenter' || type === 'dragover') s.dropEffect = initialEffect(s.effectAllowed, linkSource);
            else if (type === 'dragleave' || type === 'drop' || type === 'dragend') s.dropEffect = op;
            else s.dropEffect = 'none';
            return dt;
        },
        // After the listeners: the operation the store's effects allow now.
        settle: function (type) {
            var dt = current;
            if (!dt) return 'none';
            var s = dt[STORE];
            var op = operation(s.effectAllowed, s.dropEffect);
            if (type === 'dragend') {
                s.mode = 'disabled';
                current = null;
            } else {
                s.mode = 'protected';
            }
            return op;
        }
    });
})(globalThis);
