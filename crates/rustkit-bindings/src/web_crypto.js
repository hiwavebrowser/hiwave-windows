// crypto.getRandomValues / crypto.randomUUID (Web Crypto §10). The bytes are
// the OS random source's, reached through one host function that returns
// them as hex. crypto.subtle is not provided.
(function (g) {
    var randomHex = g.__rustkit_random_bytes;
    delete g.__rustkit_random_bytes;
    if (g.crypto !== undefined) return;

    function domError(message, name) {
        var D = g.DOMException;
        if (typeof D === 'function') return new D(message, name);
        var e = new Error(message); e.name = name; return e;
    }
    function randomBytes(n) {
        var hex = randomHex(n);
        if (typeof hex !== 'string') throw domError('The operating system random source could not be read.', 'OperationError');
        var out = new Uint8Array(n);
        for (var i = 0; i < n; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
        return out;
    }
    // The integer typed arrays; floats and DataView are TypeMismatchError.
    var INTEGER = ['Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array',
                   'Int32Array', 'Uint32Array', 'BigInt64Array', 'BigUint64Array'];

    function Crypto() { throw new TypeError('Illegal constructor'); }
    Crypto.prototype.getRandomValues = function getRandomValues(array) {
        if (!ArrayBuffer.isView(array)) {
            throw new TypeError("Failed to execute 'getRandomValues' on 'Crypto': parameter 1 is not of type 'ArrayBufferView'.");
        }
        var tag = Object.prototype.toString.call(array).slice(8, -1);
        if (INTEGER.indexOf(tag) < 0) {
            throw domError("Failed to execute 'getRandomValues' on 'Crypto': The provided ArrayBufferView is of type '" + tag + "', which is not an integer array type.", 'TypeMismatchError');
        }
        if (array.byteLength > 65536) {
            throw domError("Failed to execute 'getRandomValues' on 'Crypto': The ArrayBufferView's byte length (" + array.byteLength + ") exceeds the number of bytes of entropy available via this API (65536).", 'QuotaExceededError');
        }
        new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(randomBytes(array.byteLength));
        return array;
    };
    // RFC 9562 version 4: 122 random bits, version nibble 4, variant 10xx.
    Crypto.prototype.randomUUID = function randomUUID() {
        var b = randomBytes(16), h = '';
        b[6] = (b[6] & 0x0F) | 0x40;
        b[8] = (b[8] & 0x3F) | 0x80;
        for (var i = 0; i < 16; i++) {
            if (i === 4 || i === 6 || i === 8 || i === 10) h += '-';
            h += (b[i] < 16 ? '0' : '') + b[i].toString(16);
        }
        return h;
    };
    Object.defineProperty(Crypto.prototype, Symbol.toStringTag, { value: 'Crypto', configurable: true });

    var crypto = Object.create(Crypto.prototype);
    Object.defineProperty(g, 'Crypto', { value: Crypto, writable: true, configurable: true, enumerable: false });
    Object.defineProperty(g, 'crypto', { value: crypto, writable: true, configurable: true, enumerable: true });
})(globalThis);
