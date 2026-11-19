// FileReader (File API §6; STR-11), over web_blob.js's Blob. Evaluated after
// the DOM wrappers: it is an EventTarget.
(function (g) {
    var blobBytes = g.__rkBlobBytes;
    delete g.__rkBlobBytes;
    if (typeof blobBytes !== 'function' || typeof g.EventTarget !== 'function' || g.FileReader !== undefined) return;
    // A Blob's bytes are already in
    // memory, so a read is one task: `loadstart`, `progress`, `load` (or
    // `error`), `loadend`, after the calling script (Chrome for Testing 148
    // fires `loadstart` there too, not inside the read call). `abort()` ends
    // it at once with `abort` and `loadend`, and the queued task does nothing.
    {
        var STATE = new WeakMap();
        var HANDLERS = ['loadstart', 'progress', 'load', 'abort', 'error', 'loadend'];
        var FileReader = function FileReader() {
            if (!new.target) throw new TypeError("Failed to construct 'FileReader': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
            var self = Reflect.construct(g.EventTarget, [], new.target);
            STATE.set(self, { state: 0, result: null, error: null, read: 0, handlers: {} });
            return self;
        };
        FileReader.prototype = Object.create(g.EventTarget.prototype, {
            constructor: { value: FileReader, writable: true, configurable: true }
        });
        Object.setPrototypeOf(FileReader, g.EventTarget);
        Object.defineProperty(FileReader.prototype, Symbol.toStringTag, { value: 'FileReader', configurable: true });
        var st = function (r) {
            var s = STATE.get(r);
            if (!s) throw new TypeError('Illegal invocation');
            return s;
        };
        [['EMPTY', 0], ['LOADING', 1], ['DONE', 2]].forEach(function (pair) {
            [FileReader, FileReader.prototype].forEach(function (o) {
                Object.defineProperty(o, pair[0], { value: pair[1], enumerable: true });
            });
        });
        ['readyState', 'result', 'error'].forEach(function (name) {
            Object.defineProperty(FileReader.prototype, name, {
                get: function () { var s = st(this); return name === 'readyState' ? s.state : s[name]; },
                enumerable: true, configurable: true
            });
        });
        // An on<type> attribute is a listener in the place it was first set.
        HANDLERS.forEach(function (type) {
            Object.defineProperty(FileReader.prototype, 'on' + type, {
                get: function () { return st(this).handlers[type] || null; },
                set: function (fn) {
                    var s = st(this), self = this, had = type in s.handlers;
                    s.handlers[type] = typeof fn === 'function' ? fn : null;
                    if (!had) {
                        this.addEventListener(type, function (e) {
                            var h = s.handlers[type];
                            if (h) h.call(self, e);
                        });
                    }
                },
                enumerable: true, configurable: true
            });
        });
        var fireProgress = function (reader, type, loaded, total) {
            var init = { lengthComputable: true, loaded: loaded, total: total };
            reader.dispatchEvent(typeof g.ProgressEvent === 'function' ? new g.ProgressEvent(type, init) : new g.Event(type));
        };
        var base64 = function (bytes) {
            var s = '';
            for (var i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
            return g.btoa(s);
        };
        var startRead = function (reader, method, blob, produce) {
            var s = st(reader);
            if (!(blob instanceof g.Blob)) {
                throw new TypeError("Failed to execute '" + method + "' on 'FileReader': parameter 1 is not of type 'Blob'.");
            }
            if (s.state === 1) {
                throw new g.DOMException("Failed to execute '" + method + "' on 'FileReader': The object is already busy reading Blobs.", 'InvalidStateError');
            }
            s.state = 1; s.result = null; s.error = null;
            var read = ++s.read, bytes = blobBytes(blob), total = bytes.length;
            Promise.resolve().then(function () {
                if (s.read !== read || s.state !== 1) return;
                fireProgress(reader, 'loadstart', 0, total);
                if (s.read !== read || s.state !== 1) return;
                fireProgress(reader, 'progress', total, total);
                if (s.read !== read || s.state !== 1) return;
                var failed = null;
                try { s.result = produce(bytes, blob); } catch (e) { failed = e; }
                s.state = 2;
                if (failed) {
                    s.result = null;
                    s.error = new g.DOMException('The file could not be read.', 'NotReadableError');
                    fireProgress(reader, 'error', 0, total);
                } else {
                    fireProgress(reader, 'load', total, total);
                }
                // A handler may have started another read: no `loadend` then.
                if (s.read === read && s.state === 2) fireProgress(reader, 'loadend', failed ? 0 : total, total);
            });
        };
        FileReader.prototype.readAsText = function readAsText(blob, encoding) {
            startRead(this, 'readAsText', blob, function (bytes) {
                var label = encoding === undefined || encoding === null ? 'utf-8' : String(encoding);
                if (typeof g.TextDecoder === 'function') {
                    try { return new g.TextDecoder(label).decode(bytes); } catch (_) { return new g.TextDecoder().decode(bytes); }
                }
                return String.fromCharCode.apply(null, bytes);
            });
        };
        FileReader.prototype.readAsDataURL = function readAsDataURL(blob) {
            startRead(this, 'readAsDataURL', blob, function (bytes, b) {
                return 'data:' + (b.type || 'application/octet-stream') + ';base64,' + base64(bytes);
            });
        };
        FileReader.prototype.readAsArrayBuffer = function readAsArrayBuffer(blob) {
            startRead(this, 'readAsArrayBuffer', blob, function (bytes) {
                return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
            });
        };
        FileReader.prototype.readAsBinaryString = function readAsBinaryString(blob) {
            startRead(this, 'readAsBinaryString', blob, function (bytes) {
                var out = '';
                for (var i = 0; i < bytes.length; i++) out += String.fromCharCode(bytes[i]);
                return out;
            });
        };
        FileReader.prototype.abort = function abort() {
            var s = st(this);
            if (s.state !== 1) { s.result = null; return; }
            s.state = 2; s.result = null;
            var read = ++s.read;
            fireProgress(this, 'abort', 0, 0);
            if (s.read === read && s.state === 2) fireProgress(this, 'loadend', 0, 0);
        };
        Object.defineProperty(g, 'FileReader', { value: FileReader, writable: true, configurable: true });
    }
})(globalThis);
