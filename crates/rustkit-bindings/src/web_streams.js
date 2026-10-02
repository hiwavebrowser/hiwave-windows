// Streams (WHATWG Streams Standard): ReadableStream, WritableStream,
// TransformStream, their default readers/writers/controllers, and the two
// queuing strategies. The common subset, promise based:
//
//  - default (non-byte) streams only; `type: 'bytes'` is treated as default
//    and `getReader({ mode: 'byob' })` throws NotSupportedError;
//  - backpressure through `desiredSize`, `highWaterMark` and `size()`;
//  - `pull`/`start`/`cancel`/`write`/`close`/`abort`/`transform`/`flush`;
//  - `tee`, `pipeTo`, `pipeThrough`, `locked`, `releaseLock`, and async
//    iteration (`for await`, `values()`).
//
// Nothing here touches the network; `Response.body` etc. use it later. Each
// name is only defined when nothing else defines it.
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
    function typeError(m) { return new TypeError(m); }
    function noop() {}
    var asyncIter = typeof Symbol === 'function' ? Symbol.asyncIterator : undefined;
    function tag(C, name) {
        if (typeof Symbol === 'function') Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
    }
    function deferred() {
        var d = {};
        d.promise = new Promise(function (res, rej) { d.resolve = res; d.reject = rej; });
        d.promise.catch(noop);          // an unobserved rejection is not an error here
        return d;
    }
    function strategyOf(strategy, defaultHwm, defaultSize) {
        strategy = strategy || {};
        var hwm = strategy.highWaterMark === undefined ? defaultHwm : Number(strategy.highWaterMark);
        if (isNaN(hwm) || hwm < 0) throw new RangeError('The strategy is invalid: highWaterMark must be a non-negative number.');
        var size = strategy.size === undefined ? defaultSize : strategy.size;
        if (typeof size !== 'function') throw typeError('The strategy is invalid: size must be a function.');
        return { hwm: hwm, size: size };
    }

    // ---- Queuing strategies (§7)
    function CountQueuingStrategy(init) {
        if (!(this instanceof CountQueuingStrategy)) throw typeError("Failed to construct 'CountQueuingStrategy': Please use the 'new' operator.");
        this.highWaterMark = Number(init && init.highWaterMark);
        this.size = function () { return 1; };
    }
    function ByteLengthQueuingStrategy(init) {
        if (!(this instanceof ByteLengthQueuingStrategy)) throw typeError("Failed to construct 'ByteLengthQueuingStrategy': Please use the 'new' operator.");
        this.highWaterMark = Number(init && init.highWaterMark);
        this.size = function (chunk) { return chunk.byteLength; };
    }
    def('CountQueuingStrategy', CountQueuingStrategy);
    def('ByteLengthQueuingStrategy', ByteLengthQueuingStrategy);

    // ================= ReadableStream (§4) =================
    var RS = typeof Symbol === 'function' ? Symbol('readable') : '__rs';
    function ReadableStream(underlyingSource, strategy) {
        if (!(this instanceof ReadableStream)) throw typeError("Failed to construct 'ReadableStream': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        var src = underlyingSource === undefined || underlyingSource === null ? {} : underlyingSource;
        var st = strategyOf(strategy, 1, function () { return 1; });
        var s = {
            state: 'readable', stored: undefined, queue: [], queueSize: 0, hwm: st.hwm, sizeFn: st.size,
            started: false, pulling: false, pullAgain: false, closeRequested: false,
            reader: null, pending: [], source: src, controller: null
        };
        Object.defineProperty(this, RS, { value: s });
        var controller = Object.create(ReadableStreamDefaultController.prototype);
        Object.defineProperty(controller, RS, { value: { stream: s } });
        s.controller = controller;
        try {
            Promise.resolve(typeof src.start === 'function' ? src.start.call(src, controller) : undefined).then(function () {
                s.started = true;
                pullIfNeeded(s);
            }, function (e) { errorStream(s, e); });
        } catch (e) { errorStream(s, e); }
    }
    function desiredSize(s) {
        return s.state === 'errored' ? null : s.state === 'closed' ? 0 : s.hwm - s.queueSize;
    }
    function shouldPull(s) {
        if (s.state !== 'readable' || s.closeRequested || !s.started) return false;
        if (s.reader && s.pending.length > 0) return true;
        return desiredSize(s) > 0;
    }
    function pullIfNeeded(s) {
        if (!shouldPull(s)) return;
        if (s.pulling) { s.pullAgain = true; return; }
        s.pulling = true;
        var p;
        try { p = Promise.resolve(typeof s.source.pull === 'function' ? s.source.pull.call(s.source, s.controller) : undefined); }
        catch (e) { errorStream(s, e); return; }
        p.then(function () {
            s.pulling = false;
            if (s.pullAgain) { s.pullAgain = false; pullIfNeeded(s); }
        }, function (e) { errorStream(s, e); });
    }
    function finishClose(s) {
        s.state = 'closed';
        var r = s.reader;
        s.pending.splice(0).forEach(function (p) { p.resolve({ value: undefined, done: true }); });
        if (r) r.closed.resolve();
    }
    function errorStream(s, e) {
        if (s.state !== 'readable') return;
        s.state = 'errored'; s.stored = e; s.queue = []; s.queueSize = 0;
        s.pending.splice(0).forEach(function (p) { p.reject(e); });
        if (s.reader) s.reader.closed.reject(e);
    }
    function ReadableStreamDefaultController() { throw typeError('Illegal constructor'); }
    var CP = ReadableStreamDefaultController.prototype;
    Object.defineProperty(CP, 'desiredSize', { get: function () { return desiredSize(this[RS].stream); }, enumerable: true, configurable: true });
    CP.enqueue = function (chunk) {
        var s = this[RS].stream;
        if (s.closeRequested || s.state !== 'readable') throw typeError("Failed to execute 'enqueue' on 'ReadableStreamDefaultController': Cannot enqueue a chunk into a closed or errored stream.");
        if (s.reader && s.pending.length > 0) {
            s.pending.shift().resolve({ value: chunk, done: false });
        } else {
            var size;
            try { size = Number(s.sizeFn(chunk)); if (isNaN(size) || size < 0 || !isFinite(size)) throw new RangeError('chunk size is invalid'); }
            catch (e) { errorStream(s, e); throw e; }
            s.queue.push({ value: chunk, size: size }); s.queueSize += size;
        }
        pullIfNeeded(s);
    };
    CP.close = function () {
        var s = this[RS].stream;
        if (s.closeRequested || s.state !== 'readable') throw typeError("Failed to execute 'close' on 'ReadableStreamDefaultController': The stream is not in a state that permits close.");
        s.closeRequested = true;
        if (s.queue.length === 0) finishClose(s);
    };
    CP.error = function (e) { errorStream(this[RS].stream, e); };
    tag(ReadableStreamDefaultController, 'ReadableStreamDefaultController');

    function streamCancel(s, reason) {
        if (s.state === 'closed') return Promise.resolve();
        if (s.state === 'errored') return Promise.reject(s.stored);
        s.queue = []; s.queueSize = 0;
        finishClose(s);
        try {
            return Promise.resolve(typeof s.source.cancel === 'function' ? s.source.cancel.call(s.source, reason) : undefined).then(function () {});
        } catch (e) { return Promise.reject(e); }
    }

    function ReadableStreamDefaultReader(stream) {
        if (!(this instanceof ReadableStreamDefaultReader)) throw typeError("Failed to construct 'ReadableStreamDefaultReader': Please use the 'new' operator.");
        if (!stream || !stream[RS]) throw typeError("Failed to construct 'ReadableStreamDefaultReader': parameter 1 is not of type 'ReadableStream'.");
        var s = stream[RS];
        if (s.reader) throw typeError("Failed to construct 'ReadableStreamDefaultReader': This stream has already been locked for exclusive reading by another reader.");
        var closed = deferred();
        Object.defineProperty(this, RS, { value: { stream: s, closed: closed, released: false } });
        s.reader = this[RS];
        if (s.state === 'closed') closed.resolve();
        else if (s.state === 'errored') closed.reject(s.stored);
    }
    var RP = ReadableStreamDefaultReader.prototype;
    Object.defineProperty(RP, 'closed', { get: function () { return this[RS].closed.promise; }, enumerable: true, configurable: true });
    RP.read = function () {
        var r = this[RS], s = r.stream;
        if (r.released) return Promise.reject(typeError('This readable stream reader has been released and cannot be used to read from its previous owner stream'));
        if (s.state === 'errored') return Promise.reject(s.stored);
        if (s.queue.length > 0) {
            var item = s.queue.shift(); s.queueSize -= item.size;
            if (s.closeRequested && s.queue.length === 0) finishClose(s); else pullIfNeeded(s);
            return Promise.resolve({ value: item.value, done: false });
        }
        if (s.state === 'closed') return Promise.resolve({ value: undefined, done: true });
        var d = deferred();
        s.pending.push(d);
        pullIfNeeded(s);
        return d.promise;
    };
    RP.releaseLock = function () {
        var r = this[RS], s = r.stream;
        if (r.released) return;
        r.released = true; s.reader = null;
        var err = typeError('Reader was released');
        s.pending.splice(0).forEach(function (p) { p.reject(err); });
        if (s.state === 'readable') r.closed.reject(err);
    };
    RP.cancel = function (reason) {
        var r = this[RS];
        if (r.released) return Promise.reject(typeError('This readable stream reader has been released and cannot be used to cancel its previous owner stream'));
        return streamCancel(r.stream, reason);
    };
    tag(ReadableStreamDefaultReader, 'ReadableStreamDefaultReader');

    var P = ReadableStream.prototype;
    Object.defineProperty(P, 'locked', { get: function () { return this[RS].reader !== null; }, enumerable: true, configurable: true });
    P.getReader = function (options) {
        if (options && options.mode === 'byob') throw domError("Failed to execute 'getReader' on 'ReadableStream': BYOB readers are not supported.", 'NotSupportedError');
        return new ReadableStreamDefaultReader(this);
    };
    P.cancel = function (reason) {
        var s = this[RS];
        if (s.reader) return Promise.reject(typeError("Failed to execute 'cancel' on 'ReadableStream': Cannot cancel a locked stream"));
        return streamCancel(s, reason);
    };
    P.tee = function () {
        var reader = this.getReader(), a, b, done = false;
        function make() {
            var self = {};
            self.stream = new ReadableStream({
                start: function (c) { self.c = c; },
                pull: function () { return pump(); },
                cancel: function () { self.cancelled = true; if (a && b && a.cancelled && b.cancelled) return reader.cancel(); }
            });
            return self;
        }
        a = make(); b = make();
        function pump() {
            if (done) return Promise.resolve();
            return reader.read().then(function (r) {
                if (r.done) {
                    done = true;
                    [a, b].forEach(function (br) { if (!br.cancelled) { try { br.c.close(); } catch (e) {} } });
                    return;
                }
                [a, b].forEach(function (br) { if (!br.cancelled) { try { br.c.enqueue(r.value); } catch (e) {} } });
            }, function (e) { done = true; [a, b].forEach(function (br) { try { br.c.error(e); } catch (x) {} }); });
        }
        return [a.stream, b.stream];
    };
    P.pipeTo = function (dest, options) {
        options = options || {};
        if (!dest || !dest[WS]) return Promise.reject(typeError("Failed to execute 'pipeTo' on 'ReadableStream': parameter 1 is not of type 'WritableStream'."));
        if (this[RS].reader) return Promise.reject(typeError("Failed to execute 'pipeTo' on 'ReadableStream': Cannot pipe a locked stream"));
        var reader = this.getReader(), writer = dest.getWriter();
        function step() {
            return reader.read().then(function (r) {
                if (r.done) {
                    reader.releaseLock();
                    return options.preventClose ? writer.releaseLock() : writer.close().then(function () { writer.releaseLock(); });
                }
                return writer.write(r.value).then(step);
            });
        }
        return step().catch(function (e) {
            if (!options.preventAbort) { try { writer.abort(e); } catch (x) {} }
            if (!options.preventCancel) { try { reader.cancel(e); } catch (x) {} }
            throw e;
        });
    };
    P.pipeThrough = function (pair, options) {
        if (!pair || !pair.readable || !pair.writable) throw typeError("Failed to execute 'pipeThrough' on 'ReadableStream': parameter 1 is not a transform pair.");
        this.pipeTo(pair.writable, options).catch(noop);
        return pair.readable;
    };
    P.values = function (options) {
        var reader = this.getReader(), preventCancel = !!(options && options.preventCancel);
        var it = {
            next: function () {
                return reader.read().then(function (r) {
                    if (r.done) reader.releaseLock();
                    return r;
                }, function (e) { reader.releaseLock(); throw e; });
            },
            'return': function (value) {
                var p = preventCancel ? Promise.resolve() : reader.cancel(value);
                return p.then(function () { reader.releaseLock(); return { value: value, done: true }; });
            }
        };
        if (asyncIter) it[asyncIter] = function () { return this; };
        return it;
    };
    if (asyncIter) P[asyncIter] = P.values;
    ReadableStream.from = function (iterable) {
        var it = iterable && (iterable[asyncIter] ? iterable[asyncIter]() : iterable[Symbol.iterator] ? iterable[Symbol.iterator]() : null);
        if (!it) throw typeError("Failed to execute 'from' on 'ReadableStream': The provided value is not iterable.");
        return new ReadableStream({
            pull: function (c) {
                return Promise.resolve(it.next()).then(function (r) { if (r.done) c.close(); else c.enqueue(r.value); });
            },
            cancel: function (reason) { if (typeof it['return'] === 'function') return it['return'](reason); }
        });
    };
    tag(ReadableStream, 'ReadableStream');
    def('ReadableStream', ReadableStream);
    def('ReadableStreamDefaultReader', ReadableStreamDefaultReader);
    def('ReadableStreamDefaultController', ReadableStreamDefaultController);

    // ================= WritableStream (§5) =================
    var WS = typeof Symbol === 'function' ? Symbol('writable') : '__ws';
    function WritableStream(underlyingSink, strategy) {
        if (!(this instanceof WritableStream)) throw typeError("Failed to construct 'WritableStream': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        var sink = underlyingSink === undefined || underlyingSink === null ? {} : underlyingSink;
        var st = strategyOf(strategy, 1, function () { return 1; });
        var s = { state: 'writable', stored: undefined, sink: sink, hwm: st.hwm, sizeFn: st.size, queued: 0,
                  chain: null, writer: null, closeRequested: false, controller: null };
        Object.defineProperty(this, WS, { value: s });
        var controller = Object.create(WritableStreamDefaultController.prototype);
        Object.defineProperty(controller, WS, { value: { stream: s } });
        s.controller = controller;
        try {
            s.chain = Promise.resolve(typeof sink.start === 'function' ? sink.start.call(sink, controller) : undefined);
        } catch (e) { s.chain = Promise.reject(e); }
        s.chain = s.chain.then(noop, function (e) { errorWritable(s, e); });
    }
    function errorWritable(s, e) {
        if (s.state === 'errored' || s.state === 'closed') return;
        s.state = 'errored'; s.stored = e;
        if (s.writer) { s.writer.closed.reject(e); s.writer.ready.reject(e); }
    }
    function WritableStreamDefaultController() { throw typeError('Illegal constructor'); }
    WritableStreamDefaultController.prototype.error = function (e) { errorWritable(this[WS].stream, e); };
    tag(WritableStreamDefaultController, 'WritableStreamDefaultController');

    function WritableStreamDefaultWriter(stream) {
        if (!(this instanceof WritableStreamDefaultWriter)) throw typeError("Failed to construct 'WritableStreamDefaultWriter': Please use the 'new' operator.");
        if (!stream || !stream[WS]) throw typeError("Failed to construct 'WritableStreamDefaultWriter': parameter 1 is not of type 'WritableStream'.");
        var s = stream[WS];
        if (s.writer) throw typeError("Failed to construct 'WritableStreamDefaultWriter': This stream has already been locked for exclusive writing by another writer.");
        var w = { stream: s, closed: deferred(), ready: deferred(), released: false };
        Object.defineProperty(this, WS, { value: w });
        s.writer = w;
        if (s.state === 'errored') { w.closed.reject(s.stored); w.ready.reject(s.stored); } else w.ready.resolve();
    }
    var WP = WritableStreamDefaultWriter.prototype;
    Object.defineProperty(WP, 'closed', { get: function () { return this[WS].closed.promise; }, enumerable: true, configurable: true });
    Object.defineProperty(WP, 'ready', { get: function () { return this[WS].ready.promise; }, enumerable: true, configurable: true });
    Object.defineProperty(WP, 'desiredSize', { get: function () {
        var s = this[WS].stream;
        return s.state === 'errored' ? null : s.state === 'closed' ? 0 : s.hwm - s.queued;
    }, enumerable: true, configurable: true });
    WP.write = function (chunk) {
        var w = this[WS], s = w.stream;
        if (w.released) return Promise.reject(typeError('This writable stream writer has been released and cannot be used to write to its previous owner stream'));
        if (s.state === 'errored') return Promise.reject(s.stored);
        if (s.state === 'closed' || s.closeRequested) return Promise.reject(typeError('Cannot write to a closing or closed stream'));
        var size;
        try { size = Number(s.sizeFn(chunk)); if (isNaN(size) || size < 0) size = 1; } catch (e) { return Promise.reject(e); }
        s.queued += size;
        var p = s.chain.then(function () {
            if (s.state === 'errored') throw s.stored;
            return typeof s.sink.write === 'function' ? s.sink.write.call(s.sink, chunk, s.controller) : undefined;
        }).then(function () { s.queued -= size; }, function (e) { s.queued -= size; errorWritable(s, e); throw e; });
        s.chain = p.then(noop, noop);
        return p.then(noop);
    };
    WP.close = function () {
        var w = this[WS], s = w.stream;
        if (w.released) return Promise.reject(typeError('This writable stream writer has been released'));
        if (s.state === 'errored') return Promise.reject(s.stored);
        if (s.state === 'closed' || s.closeRequested) return Promise.reject(typeError('Cannot close a closing or closed stream'));
        s.closeRequested = true;
        var p = s.chain.then(function () {
            if (s.state === 'errored') throw s.stored;
            return typeof s.sink.close === 'function' ? s.sink.close.call(s.sink) : undefined;
        }).then(function () { s.state = 'closed'; if (s.writer) s.writer.closed.resolve(); }, function (e) { errorWritable(s, e); throw e; });
        s.chain = p.then(noop, noop);
        return p.then(noop);
    };
    WP.abort = function (reason) {
        var w = this[WS], s = w.stream;
        if (w.released) return Promise.reject(typeError('This writable stream writer has been released'));
        if (s.state === 'closed' || s.state === 'errored') return Promise.resolve();
        errorWritable(s, reason);
        try { return Promise.resolve(typeof s.sink.abort === 'function' ? s.sink.abort.call(s.sink, reason) : undefined).then(noop); }
        catch (e) { return Promise.reject(e); }
    };
    WP.releaseLock = function () {
        var w = this[WS];
        if (w.released) return;
        w.released = true; w.stream.writer = null;
    };
    tag(WritableStreamDefaultWriter, 'WritableStreamDefaultWriter');

    var WSP = WritableStream.prototype;
    Object.defineProperty(WSP, 'locked', { get: function () { return this[WS].writer !== null; }, enumerable: true, configurable: true });
    WSP.getWriter = function () { return new WritableStreamDefaultWriter(this); };
    WSP.abort = function (reason) {
        if (this[WS].writer) return Promise.reject(typeError("Failed to execute 'abort' on 'WritableStream': Cannot abort a locked stream"));
        return new WritableStreamDefaultWriter(this).abort(reason);
    };
    WSP.close = function () {
        if (this[WS].writer) return Promise.reject(typeError("Failed to execute 'close' on 'WritableStream': Cannot close a locked stream"));
        return new WritableStreamDefaultWriter(this).close();
    };
    tag(WritableStream, 'WritableStream');
    def('WritableStream', WritableStream);
    def('WritableStreamDefaultWriter', WritableStreamDefaultWriter);
    def('WritableStreamDefaultController', WritableStreamDefaultController);

    // ================= TransformStream (§6) =================
    function TransformStreamDefaultController() { throw typeError('Illegal constructor'); }
    function TransformStream(transformer, writableStrategy, readableStrategy) {
        if (!(this instanceof TransformStream)) throw typeError("Failed to construct 'TransformStream': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        var t = transformer === undefined || transformer === null ? {} : transformer, rc;
        var readable = new ReadableStream({ start: function (c) { rc = c; } }, readableStrategy || { highWaterMark: 0 });
        var tc = Object.create(TransformStreamDefaultController.prototype);
        Object.defineProperty(tc, 'desiredSize', { get: function () { return rc.desiredSize; }, enumerable: true, configurable: true });
        tc.enqueue = function (chunk) { rc.enqueue(chunk); };
        tc.error = function (e) { rc.error(e); };
        tc.terminate = function () { try { rc.close(); } catch (e) {} };
        var writable = new WritableStream({
            start: function () { return typeof t.start === 'function' ? t.start.call(t, tc) : undefined; },
            write: function (chunk) {
                if (typeof t.transform === 'function') return t.transform.call(t, chunk, tc);
                tc.enqueue(chunk);
            },
            close: function () {
                return Promise.resolve(typeof t.flush === 'function' ? t.flush.call(t, tc) : undefined).then(function () {
                    try { rc.close(); } catch (e) {}
                });
            },
            abort: function (reason) { rc.error(reason); }
        }, writableStrategy);
        Object.defineProperty(this, 'readable', { value: readable, enumerable: true });
        Object.defineProperty(this, 'writable', { value: writable, enumerable: true });
    }
    tag(TransformStream, 'TransformStream');
    tag(TransformStreamDefaultController, 'TransformStreamDefaultController');
    def('TransformStream', TransformStream);
    def('TransformStreamDefaultController', TransformStreamDefaultController);

    // ================= Text streams (Encoding Standard §9) =================
    // TextEncoderStream / TextDecoderStream: a TransformStream over
    // TextEncoder / TextDecoder (web_encoding.js). The decoder is run in
    // streaming mode so a character split across chunks decodes whole.
    if (typeof g.TextEncoder === 'function' && typeof g.TextDecoder === 'function') {
        var TS = g.TransformStream;
        var TEXT = typeof Symbol === 'function' ? Symbol('text-stream') : '__ts';
        var TextEncoderStream = function TextEncoderStream() {
            if (!(this instanceof TextEncoderStream)) throw typeError("Failed to construct 'TextEncoderStream': Please use the 'new' operator.");
            var enc = new g.TextEncoder();
            Object.defineProperty(this, TEXT, { value: new TS({
                transform: function (chunk, c) { c.enqueue(enc.encode(String(chunk))); }
            }) });
        };
        Object.defineProperty(TextEncoderStream.prototype, 'encoding', { get: function () { return 'utf-8'; }, enumerable: true, configurable: true });
        Object.defineProperty(TextEncoderStream.prototype, 'readable', { get: function () { return this[TEXT].readable; }, enumerable: true, configurable: true });
        Object.defineProperty(TextEncoderStream.prototype, 'writable', { get: function () { return this[TEXT].writable; }, enumerable: true, configurable: true });
        tag(TextEncoderStream, 'TextEncoderStream');
        def('TextEncoderStream', TextEncoderStream);

        var TextDecoderStream = function TextDecoderStream(label, options) {
            if (!(this instanceof TextDecoderStream)) throw typeError("Failed to construct 'TextDecoderStream': Please use the 'new' operator.");
            var dec = new g.TextDecoder(label, options);
            Object.defineProperty(this, '_dec', { value: dec });
            Object.defineProperty(this, TEXT, { value: new TS({
                transform: function (chunk, c) {
                    var text = dec.decode(chunk, { stream: true });
                    if (text !== '') c.enqueue(text);
                },
                flush: function (c) {
                    var text = dec.decode();
                    if (text !== '') c.enqueue(text);
                }
            }) });
        };
        ['encoding', 'fatal', 'ignoreBOM'].forEach(function (k) {
            Object.defineProperty(TextDecoderStream.prototype, k, { get: function () { return this._dec[k]; }, enumerable: true, configurable: true });
        });
        Object.defineProperty(TextDecoderStream.prototype, 'readable', { get: function () { return this[TEXT].readable; }, enumerable: true, configurable: true });
        Object.defineProperty(TextDecoderStream.prototype, 'writable', { get: function () { return this[TEXT].writable; }, enumerable: true, configurable: true });
        tag(TextDecoderStream, 'TextDecoderStream');
        def('TextDecoderStream', TextDecoderStream);
    }
})(globalThis);
