// Text and base64 encoding globals: btoa / atob, escape / unescape,
// TextEncoder / TextDecoder. The byte work itself (UTF-8, base64, binary
// strings) is the host's (`__rk`, rustkit-js bytes.rs): done here a byte per
// loop turn it took seconds per megabyte. Each name is only defined when
// nothing else has defined it.
(function (g) {
    function def(name, value) {
        if (g[name] === undefined) {
            Object.defineProperty(g, name, { value: value, writable: true, configurable: true, enumerable: false });
        }
    }
    function domError(message, name) {
        var D = g.DOMException;
        if (typeof D === 'function') return new D(message, name);
        var e = new Error(message); e.name = name; return e;
    }

    // ---- btoa / atob (HTML §8.3 "forgiving-base64"). Latin-1 only.
    def('btoa', function btoa(data) {
        if (arguments.length === 0) throw new TypeError("Failed to execute 'btoa' on 'Window': 1 argument required, but only 0 present.");
        var bytes = __rk.binaryEncode(String(data));
        if (bytes === null) {
            throw domError("Failed to execute 'btoa' on 'Window': The string to be encoded contains characters outside of the Latin1 range.", 'InvalidCharacterError');
        }
        return __rk.base64Encode(bytes);
    });
    def('atob', function atob(data) {
        if (arguments.length === 0) throw new TypeError("Failed to execute 'atob' on 'Window': 1 argument required, but only 0 present.");
        var s = String(data).replace(/[\t\n\f\r ]/g, '');
        if (s.length % 4 === 0) s = s.replace(/==?$/, '');
        if (s.length % 4 === 1 || /[^A-Za-z0-9+/]/.test(s)) {
            throw domError("Failed to execute 'atob' on 'Window': The string to be decoded is not correctly encoded.", 'InvalidCharacterError');
        }
        var bytes = __rk.base64Decode(s);
        if (bytes === null) {
            throw domError("Failed to execute 'atob' on 'Window': The string to be decoded is not correctly encoded.", 'InvalidCharacterError');
        }
        return __rk.binaryDecode(bytes);
    });

    // ---- escape / unescape (ECMA-262 Annex B.2.1).
    var ESC_OK = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789@*_+-./';
    def('escape', function escape(str) {
        var s = String(str), out = '';
        for (var i = 0; i < s.length; i++) {
            var c = s.charCodeAt(i);
            if (ESC_OK.indexOf(s[i]) >= 0) out += s[i];
            else if (c < 256) out += '%' + (c < 16 ? '0' : '') + c.toString(16).toUpperCase();
            else out += '%u' + ('0000' + c.toString(16).toUpperCase()).slice(-4);
        }
        return out;
    });
    def('unescape', function unescape(str) {
        var s = String(str), out = '';
        for (var i = 0; i < s.length; i++) {
            var c = s[i];
            if (c === '%') {
                if (s[i + 1] === 'u' && /^[0-9a-fA-F]{4}$/.test(s.slice(i + 2, i + 6))) {
                    out += String.fromCharCode(parseInt(s.slice(i + 2, i + 6), 16)); i += 5; continue;
                }
                if (/^[0-9a-fA-F]{2}$/.test(s.slice(i + 1, i + 3))) {
                    out += String.fromCharCode(parseInt(s.slice(i + 1, i + 3), 16)); i += 2; continue;
                }
            }
            out += c;
        }
        return out;
    });

    // ---- TextEncoder (Encoding Standard §8.3): always UTF-8.
    function utf8Bytes(str) {
        var bytes = [];
        for (var i = 0; i < str.length; i++) {
            var c = str.charCodeAt(i);
            if (c >= 0xD800 && c <= 0xDBFF && i + 1 < str.length) {
                var d = str.charCodeAt(i + 1);
                if (d >= 0xDC00 && d <= 0xDFFF) { c = 0x10000 + ((c - 0xD800) << 10) + (d - 0xDC00); i++; }
                else c = 0xFFFD;
            } else if (c >= 0xD800 && c <= 0xDFFF) {
                c = 0xFFFD;                                  // a lone surrogate
            }
            if (c < 0x80) bytes.push(c);
            else if (c < 0x800) bytes.push(0xC0 | (c >> 6), 0x80 | (c & 63));
            else if (c < 0x10000) bytes.push(0xE0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
            else bytes.push(0xF0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
        }
        return bytes;
    }
    function TextEncoder() {
        if (!(this instanceof TextEncoder)) throw new TypeError("Failed to construct 'TextEncoder': Please use the 'new' operator");
    }
    Object.defineProperty(TextEncoder.prototype, 'encoding', { get: function () { return 'utf-8'; }, configurable: true, enumerable: true });
    TextEncoder.prototype.encode = function (input) {
        return __rk.utf8Encode(input === undefined ? '' : String(input));
    };
    TextEncoder.prototype.encodeInto = function (source, dest) {
        var s = String(source), read = 0, written = 0;
        for (var i = 0; i < s.length; i++) {
            var units = (s.charCodeAt(i) >= 0xD800 && s.charCodeAt(i) <= 0xDBFF && i + 1 < s.length &&
                         s.charCodeAt(i + 1) >= 0xDC00 && s.charCodeAt(i + 1) <= 0xDFFF) ? 2 : 1;
            var b = utf8Bytes(s.slice(i, i + units));
            if (written + b.length > dest.length) break;
            for (var k = 0; k < b.length; k++) dest[written++] = b[k];
            read += units; i += units - 1;
        }
        return { read: read, written: written };
    };
    Object.defineProperty(TextEncoder.prototype, Symbol.toStringTag, { value: 'TextEncoder', configurable: true });
    def('TextEncoder', TextEncoder);

    // ---- TextDecoder (Encoding Standard §8.2): utf-8, utf-16le, latin1.
    var LABELS = {
        'utf-8': 'utf-8', 'utf8': 'utf-8', 'unicode-1-1-utf-8': 'utf-8',
        'utf-16le': 'utf-16le', 'utf-16': 'utf-16le', 'ucs-2': 'utf-16le', 'unicode': 'utf-16le',
        'iso-8859-1': 'windows-1252', 'latin1': 'windows-1252', 'ascii': 'windows-1252',
        'us-ascii': 'windows-1252', 'windows-1252': 'windows-1252', 'l1': 'windows-1252'
    };
    function toBytes(input) {
        if (input === undefined) return new Uint8Array(0);
        if (input instanceof ArrayBuffer) return new Uint8Array(input);
        if (ArrayBuffer.isView(input)) return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
        throw new TypeError("Failed to execute 'decode' on 'TextDecoder': The provided value is not of type '(ArrayBuffer or ArrayBufferView)'.");
    }
    function TextDecoder(label, options) {
        if (!(this instanceof TextDecoder)) throw new TypeError("Failed to construct 'TextDecoder': Please use the 'new' operator");
        var l = String(label === undefined ? 'utf-8' : label).trim().toLowerCase();
        var enc = LABELS[l];
        if (!enc) {
            var RE = g.RangeError || Error;
            throw new RE("Failed to construct 'TextDecoder': The encoding label provided ('" + label + "') is invalid.");
        }
        Object.defineProperty(this, '_enc', { value: enc });
        Object.defineProperty(this, '_fatal', { value: !!(options && options.fatal) });
        Object.defineProperty(this, '_ignoreBOM', { value: !!(options && options.ignoreBOM) });
        Object.defineProperty(this, '_pending', { value: [], writable: true });
        Object.defineProperty(this, '_bomSeen', { value: false, writable: true });
    }
    Object.defineProperty(TextDecoder.prototype, 'encoding', { get: function () { return this._enc; }, configurable: true, enumerable: true });
    Object.defineProperty(TextDecoder.prototype, 'fatal', { get: function () { return this._fatal; }, configurable: true, enumerable: true });
    Object.defineProperty(TextDecoder.prototype, 'ignoreBOM', { get: function () { return this._ignoreBOM; }, configurable: true, enumerable: true });
    TextDecoder.prototype.decode = function (input, options) {
        var stream = !!(options && options.stream);
        var bytes = toBytes(input);
        if (this._pending.length) {
            // What the last chunk left unfinished comes first.
            var joined = new Uint8Array(this._pending.length + bytes.length);
            joined.set(this._pending, 0);
            joined.set(bytes, this._pending.length);
            bytes = joined;
        }
        var text = '', used = bytes.length;
        if (this._enc === 'utf-8') {
            // [text, bytes used, saw a malformed sequence]; an incomplete
            // tail is left for the next chunk while streaming.
            var r = __rk.utf8Decode(bytes, !stream);
            if (r[2] && this._fatal) throw new TypeError("Failed to execute 'decode' on 'TextDecoder': The encoded data was not valid for encoding utf-8");
            text = r[0]; used = r[1];
        } else if (this._enc === 'utf-16le') {
            used = bytes.length - (bytes.length % 2);
            for (var i = 0; i < used; i += 2) text += String.fromCharCode(bytes[i] | (bytes[i + 1] << 8));
            if (!stream && used < bytes.length) text += '\ufffd';
        } else {
            text = __rk.binaryDecode(bytes);
        }
        this._pending = stream ? bytes.slice(used) : [];
        if (!this._bomSeen && text.length) {
            if (!this._ignoreBOM && text.charCodeAt(0) === 0xFEFF) text = text.slice(1);
            this._bomSeen = stream;
        }
        if (!stream) this._bomSeen = false;
        return text;
    };
    Object.defineProperty(TextDecoder.prototype, Symbol.toStringTag, { value: 'TextDecoder', configurable: true });
    def('TextDecoder', TextDecoder);
})(globalThis);
