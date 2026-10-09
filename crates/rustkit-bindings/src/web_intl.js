// Intl for page scripts, as a script shim over plain JavaScript (Boa is built
// without its ICU-backed `intl` feature, and no ICU crate is added).
//
// LIMIT: this is an en-US baseline only. Every requested locale is checked
// for well-formedness and then resolves to en-US, so
// `resolvedOptions().locale` is always 'en-US' and output is what Chrome
// prints for en-US whatever locale the page asked for. Time zones are 'UTC'
// and the default (host) zone only; any other valid-looking IANA name
// formats in the default zone. Collator compares by code point after folding
// case and accents, not by the full Unicode Collation Algorithm.
//
// Covers Intl.NumberFormat, DateTimeFormat, PluralRules, Collator,
// ListFormat, RelativeTimeFormat, getCanonicalLocales and supportedLocalesOf,
// and routes Number/BigInt.prototype.toLocaleString and
// Date.prototype.toLocale{,Date,Time}String through them.
(function (g) {
    if (typeof g.Intl === 'object' && g.Intl !== null && typeof g.Intl.NumberFormat === 'function') return;

    var LOCALE = 'en-US';

    function hidden(obj, name, value) {
        Object.defineProperty(obj, name, { value: value, writable: true, configurable: true, enumerable: false });
    }
    function methods(obj, map) {
        for (var k in map) hidden(obj, k, map[k]);
    }
    function tag(proto, name) {
        Object.defineProperty(proto, Symbol.toStringTag, { value: name, configurable: true });
    }
    // A getter that hands out one bound function per instance (`format`, `compare`).
    function boundGetter(proto, name, impl, check) {
        Object.defineProperty(proto, name, {
            configurable: true,
            enumerable: false,
            get: function () {
                var self = check(this, name);
                if (!self['_' + name]) {
                    hidden(self, '_' + name, function (a, b) { return impl.call(self, a, b); });
                }
                return self['_' + name];
            }
        });
    }
    function checker(Ctor, label) {
        return function (self, method) {
            if (!(self instanceof Ctor) || !self._intl) {
                throw new TypeError('Method Intl.' + label + '.prototype.' + method + ' called on incompatible receiver ' + String(self));
            }
            return self;
        };
    }

    // ---- options
    function toOptions(o) {
        if (o === undefined) return Object.create(null);
        if (o === null) throw new TypeError('Cannot convert undefined or null to object');
        return Object(o);
    }
    function getOpt(o, name, allowed, fallback) {
        var v = o[name];
        if (v === undefined) return fallback;
        if (allowed === 'boolean') return Boolean(v);
        v = String(v);
        if (allowed && allowed.indexOf(v) < 0) {
            throw new RangeError('Value ' + v + ' out of range for Intl options property ' + name);
        }
        return v;
    }
    function getNum(o, name, min, max, fallback) {
        var v = o[name];
        if (v === undefined) return fallback;
        v = Number(v);
        if (isNaN(v) || v < min || v > max) throw new RangeError(name + ' value is out of range.');
        return Math.floor(v);
    }

    // ---- locales (BCP 47 well-formedness and canonical case only)
    function canonTag(t) {
        if (typeof t !== 'string' && (typeof t !== 'object' || t === null)) {
            throw new TypeError('Language ID should be string or object.');
        }
        var s = String(t), sub = s.split('-'), out = [], ext = false;
        var bad = !/^([A-Za-z]{2,3}|[A-Za-z]{5,8})$/.test(sub[0]);
        for (var i = 0; i < sub.length && !bad; i++) {
            var p = sub[i];
            if (!/^[A-Za-z0-9]{1,8}$/.test(p)) { bad = true; break; }
            if (i === 0) { out.push(p.toLowerCase()); continue; }
            if (p.length === 1) ext = true;
            if (!ext && p.length === 4 && /^[A-Za-z]+$/.test(p) && i <= 2) {
                out.push(p[0].toUpperCase() + p.slice(1).toLowerCase());
            } else if (!ext && (/^[A-Za-z]{2}$/.test(p) || /^[0-9]{3}$/.test(p))) {
                out.push(p.toUpperCase());
            } else {
                out.push(p.toLowerCase());
            }
        }
        if (bad) throw new RangeError('Incorrect locale information provided');
        return out.join('-');
    }
    function canonicalList(locales) {
        if (locales === undefined) return [];
        var list = typeof locales === 'string' ? [locales] : Object(locales), out = [];
        var n = typeof locales === 'string' ? 1 : (Number(list.length) || 0);
        for (var i = 0; i < n; i++) {
            if (!(i in list)) continue;
            var c = canonTag(list[i]);
            if (out.indexOf(c) < 0) out.push(c);
        }
        return out;
    }
    // Validates the request; the answer is always en-US (see LIMIT above).
    function resolveLocale(locales) {
        canonicalList(locales);
        return LOCALE;
    }
    function supportedOf(locales, options) {
        var list = canonicalList(locales);
        getOpt(toOptions(options), 'localeMatcher', ['lookup', 'best fit'], 'best fit');
        return list.filter(function (t) { return /^en(-Latn)?(-US)?(-[A-Za-z0-9]-.*)?$/.test(t); });
    }

    // ---- exact decimal digits of a number: value = 0.ds * 10^pt
    function decimalOf(x) {
        if (typeof x === 'bigint') {
            var b = String(x < 0 ? -x : x).replace(/^0+/, '');
            var t = b.replace(/0+$/, '');
            return { ds: t, pt: t ? b.length : 0 };
        }
        // String(x) is the shortest round-trip form, which ICU also rounds from.
        var s = String(Math.abs(x)), e = 0, k = s.indexOf('e');
        if (k >= 0) { e = Number(s.slice(k + 1)); s = s.slice(0, k); }
        var dot = s.indexOf('.');
        var ip = dot < 0 ? s : s.slice(0, dot), fp = dot < 0 ? '' : s.slice(dot + 1);
        var ds = ip + fp, pt = ip.length + e, i = 0;
        while (i < ds.length && ds.charAt(i) === '0') { i++; pt--; }
        ds = ds.slice(i).replace(/0+$/, '');
        return { ds: ds, pt: ds ? pt : 0 };
    }
    // Keep `keep` leading digits, rounding half away from zero.
    function roundDigits(d, keep) {
        if (keep >= d.ds.length) return d;
        if (keep < 0) return { ds: '', pt: 0 };
        var pt = d.pt, head = d.ds.slice(0, keep);
        if (d.ds.charCodeAt(keep) >= 53) {
            var a = head.split(''), i = a.length - 1;
            while (i >= 0 && a[i] === '9') { a[i] = '0'; i--; }
            if (i < 0) { a.unshift('1'); pt++; } else { a[i] = String.fromCharCode(a[i].charCodeAt(0) + 1); }
            head = a.join('');
        }
        head = head.replace(/0+$/, '');
        return { ds: head, pt: head ? pt : 0 };
    }
    function zeros(n) { var s = ''; while (n-- > 0) s += '0'; return s; }
    // Rounds per the digit options; returns { int, frac, zero }.
    function digitStrings(d, o) {
        if (o.sig) {
            d = roundDigits(d, o.mxsd);
        } else {
            d = roundDigits(d, d.pt + o.mxfd);
        }
        var ip, fp;
        if (!d.ds) { ip = '0'; fp = ''; }
        else if (d.pt <= 0) { ip = '0'; fp = zeros(-d.pt) + d.ds; }
        else { ip = d.ds.slice(0, d.pt) + zeros(d.pt - d.ds.length); fp = d.ds.slice(d.pt); }
        if (o.sig) {
            var have = d.ds ? d.ds.length + Math.max(0, d.pt - d.ds.length) : 1;
            if (have < o.mnsd) fp += zeros(o.mnsd - have);
        } else if (fp.length < o.mnfd) {
            fp += zeros(o.mnfd - fp.length);
        }
        while (ip.length < o.mnid) ip = '0' + ip;
        return { int: ip, frac: fp, zero: !d.ds };
    }
    function pushInteger(parts, ip, grouping) {
        if (grouping === false || (grouping === 'min2' && ip.length < 5) || ip.length <= 3) {
            parts.push({ type: 'integer', value: ip });
            return;
        }
        var first = ip.length % 3 || 3;
        parts.push({ type: 'integer', value: ip.slice(0, first) });
        for (var i = first; i < ip.length; i += 3) {
            parts.push({ type: 'group', value: ',' });
            parts.push({ type: 'integer', value: ip.slice(i, i + 3) });
        }
    }

    // ---- NumberFormat
    var CURRENCY_SYMBOL = {
        USD: '$', EUR: '€', GBP: '£', JPY: '¥', CAD: 'CA$', AUD: 'A$', NZD: 'NZ$', MXN: 'MX$',
        HKD: 'HK$', BRL: 'R$', INR: '₹', CNY: 'CN¥', KRW: '₩', ILS: '₪', VND: '₫', TWD: 'NT$', XAF: 'FCFA'
    };
    var CURRENCY_NARROW = { CAD: '$', AUD: '$', NZD: '$', MXN: '$', HKD: '$', TWD: '$', CNY: '¥', BRL: 'R$' };
    var CURRENCY_NAME = { USD: ['US dollar', 'US dollars'], EUR: ['euro', 'euros'], GBP: ['British pound', 'British pounds'], JPY: ['Japanese yen', 'Japanese yen'] };
    var ZERO_DIGIT_CURRENCIES = ['JPY', 'KRW', 'VND', 'CLP', 'ISK', 'UGX', 'XAF', 'XOF', 'PYG', 'RWF', 'KMF', 'GNF', 'DJF', 'XPF', 'VUV'];
    var UNIT_SHORT = {
        percent: '%', kilometer: 'km', meter: 'm', centimeter: 'cm', millimeter: 'mm', mile: 'mi', foot: 'ft', inch: 'in',
        kilogram: 'kg', gram: 'g', pound: 'lb', ounce: 'oz', liter: 'L', milliliter: 'mL', second: 'sec', minute: 'min',
        hour: 'hr', millisecond: 'ms', byte: 'byte', kilobyte: 'kB', megabyte: 'MB', gigabyte: 'GB', terabyte: 'TB',
        celsius: '°C', fahrenheit: '°F', 'kilometer-per-hour': 'km/h', 'mile-per-hour': 'mph', degree: 'deg'
    };
    var COMPACT_SHORT = ['', 'K', 'M', 'B', 'T'], COMPACT_LONG = ['', 'thousand', 'million', 'billion', 'trillion'];

    function NumberFormat(locales, options) {
        if (!(this instanceof NumberFormat)) return new NumberFormat(locales, options);
        var o = toOptions(options), r = {};
        r.locale = resolveLocale(locales);
        getOpt(o, 'localeMatcher', ['lookup', 'best fit'], 'best fit');
        getOpt(o, 'numberingSystem', null, undefined);
        r.style = getOpt(o, 'style', ['decimal', 'percent', 'currency', 'unit'], 'decimal');
        var cur = getOpt(o, 'currency', null, undefined);
        if (cur !== undefined && !/^[A-Za-z]{3}$/.test(cur)) throw new RangeError('Invalid currency code : ' + cur);
        var cd = getOpt(o, 'currencyDisplay', ['code', 'symbol', 'narrowSymbol', 'name'], 'symbol');
        var cs = getOpt(o, 'currencySign', ['standard', 'accounting'], 'standard');
        var unit = getOpt(o, 'unit', null, undefined);
        var ud = getOpt(o, 'unitDisplay', ['short', 'narrow', 'long'], 'short');
        if (r.style === 'currency') {
            if (cur === undefined) throw new TypeError('Currency code is required with currency style.');
            r.currency = cur.toUpperCase(); r.currencyDisplay = cd; r.currencySign = cs;
        }
        if (r.style === 'unit') {
            if (unit === undefined) throw new TypeError('Unit is required with unit style.');
            r.unit = unit; r.unitDisplay = ud;
        }
        r.notation = getOpt(o, 'notation', ['standard', 'scientific', 'engineering', 'compact'], 'standard');
        var defMin = 0, defMax = 3;
        if (r.style === 'currency') {
            defMin = defMax = ZERO_DIGIT_CURRENCIES.indexOf(r.currency) >= 0 ? 0 : 2;
        } else if (r.style === 'percent') {
            defMax = 0;
        }
        r.minimumIntegerDigits = getNum(o, 'minimumIntegerDigits', 1, 21, 1);
        var mnfdRaw = o.minimumFractionDigits, mxfdRaw = o.maximumFractionDigits;
        var mnsdRaw = o.minimumSignificantDigits, mxsdRaw = o.maximumSignificantDigits;
        var hasSd = mnsdRaw !== undefined || mxsdRaw !== undefined;
        var hasFd = mnfdRaw !== undefined || mxfdRaw !== undefined;
        r._compactRound = false;
        if (hasSd) {
            r.minimumSignificantDigits = getNum(o, 'minimumSignificantDigits', 1, 21, 1);
            r.maximumSignificantDigits = getNum(o, 'maximumSignificantDigits', r.minimumSignificantDigits, 21, 21);
        } else if (hasFd) {
            var mnfd = getNum(o, 'minimumFractionDigits', 0, 100, undefined);
            var mxfd = getNum(o, 'maximumFractionDigits', 0, 100, undefined);
            if (mnfd === undefined) mnfd = Math.min(defMin, mxfd);
            else if (mxfd === undefined) mxfd = Math.max(defMax, mnfd);
            else if (mnfd > mxfd) throw new RangeError('maximumFractionDigits value is out of range.');
            r.minimumFractionDigits = mnfd; r.maximumFractionDigits = mxfd;
        } else if (r.notation === 'compact') {
            r._compactRound = true;
            r.minimumFractionDigits = 0; r.maximumFractionDigits = 0;
        } else {
            r.minimumFractionDigits = defMin; r.maximumFractionDigits = defMax;
        }
        r.compactDisplay = getOpt(o, 'compactDisplay', ['short', 'long'], 'short');
        var ug = o.useGrouping;
        if (ug === undefined) r.useGrouping = r.notation === 'compact' ? 'min2' : 'auto';
        else if (ug === true || ug === 'always') r.useGrouping = 'always';
        else if (ug === false || ug === null || ug === '' || ug === 0) r.useGrouping = false;
        else r.useGrouping = getOpt(o, 'useGrouping', ['min2', 'auto', 'always', 'true', 'false'], 'auto');
        if (r.useGrouping === 'true') r.useGrouping = 'always';
        if (r.useGrouping === 'false') r.useGrouping = 'auto';
        r.signDisplay = getOpt(o, 'signDisplay', ['auto', 'never', 'always', 'exceptZero', 'negative'], 'auto');
        hidden(this, '_intl', r);
    }
    var nfCheck = checker(NumberFormat, 'NumberFormat');

    function toIntlNumber(x) {
        if (typeof x === 'bigint') return x;
        if (typeof x === 'object' && x !== null && typeof x.valueOf() === 'bigint') return x.valueOf();
        return Number(x);
    }
    function digitOptions(r) {
        if (r.maximumSignificantDigits !== undefined) {
            return { sig: true, mnsd: r.minimumSignificantDigits, mxsd: r.maximumSignificantDigits, mnid: r.minimumIntegerDigits };
        }
        return { sig: false, mnfd: r.minimumFractionDigits, mxfd: r.maximumFractionDigits, mnid: r.minimumIntegerDigits };
    }
    // The number's own parts (no sign, currency or percent), plus whether it rounded to zero.
    function numberBody(r, x) {
        var parts = [];
        if (typeof x === 'number' && isNaN(x)) return { parts: [{ type: 'nan', value: 'NaN' }], zero: false };
        if (typeof x === 'number' && !isFinite(x)) return { parts: [{ type: 'infinity', value: '∞' }], zero: false };
        var d = decimalOf(x);
        if (r.style === 'percent') d = { ds: d.ds, pt: d.ds ? d.pt + 2 : 0 };
        var opts = digitOptions(r), suffix = null, expo = null;
        if (r.notation === 'compact') {
            var k = d.ds ? Math.max(0, Math.min(4, Math.floor((d.pt - 1) / 3))) : 0;
            var sd, s;
            for (;;) {
                sd = { ds: d.ds, pt: d.ds ? d.pt - 3 * k : 0 };
                if (r._compactRound) {
                    // ICU's compact default: round to an integer, but keep two significant digits.
                    s = digitStrings(roundDigits(sd, Math.max(sd.pt, 2)), { sig: false, mnfd: 0, mxfd: 100, mnid: opts.mnid });
                } else {
                    s = digitStrings(sd, opts);
                }
                if (k < 4 && s.int.replace(/^0+/, '').length > 3) { k++; continue; }
                break;
            }
            pushInteger(parts, s.int, r.useGrouping);
            if (s.frac) { parts.push({ type: 'decimal', value: '.' }); parts.push({ type: 'fraction', value: s.frac }); }
            if (k > 0) suffix = r.compactDisplay === 'long' ? COMPACT_LONG[k] : COMPACT_SHORT[k];
            if (suffix && r.compactDisplay === 'long') parts.push({ type: 'literal', value: ' ' });
            if (suffix) parts.push({ type: 'compact', value: suffix });
            return { parts: parts, zero: s.zero };
        }
        if (r.notation === 'scientific' || r.notation === 'engineering') {
            var e = d.ds ? d.pt - 1 : 0;
            if (r.notation === 'engineering') e = Math.floor(e / 3) * 3;
            var sc = digitStrings({ ds: d.ds, pt: d.ds ? d.pt - e : 0 }, opts);
            if (sc.int.length > (r.notation === 'engineering' ? 3 : 1)) {
                e += r.notation === 'engineering' ? 3 : 1;
                sc = digitStrings({ ds: d.ds, pt: d.ds ? d.pt - e : 0 }, opts);
            }
            parts.push({ type: 'integer', value: sc.int });
            if (sc.frac) { parts.push({ type: 'decimal', value: '.' }); parts.push({ type: 'fraction', value: sc.frac }); }
            parts.push({ type: 'exponentSeparator', value: 'E' });
            if (e < 0) parts.push({ type: 'exponentMinusSign', value: '-' });
            parts.push({ type: 'exponentInteger', value: String(Math.abs(e)) });
            return { parts: parts, zero: sc.zero };
        }
        var ds = digitStrings(d, opts);
        pushInteger(parts, ds.int, r.useGrouping);
        if (ds.frac) { parts.push({ type: 'decimal', value: '.' }); parts.push({ type: 'fraction', value: ds.frac }); }
        return { parts: parts, zero: ds.zero };
    }
    function isNegative(x) {
        if (typeof x === 'bigint') return x < 0;
        return x < 0 || (x === 0 && 1 / x < 0);
    }
    function numberToParts(r, x) {
        x = toIntlNumber(x);
        var body = numberBody(r, x), neg = isNegative(x), sign = '';
        var isNaNValue = typeof x === 'number' && isNaN(x);
        switch (r.signDisplay) {
            case 'auto': sign = neg ? '-' : ''; break;
            case 'always': sign = neg ? '-' : '+'; break;
            case 'exceptZero': sign = body.zero || isNaNValue ? '' : (neg ? '-' : '+'); break;
            case 'negative': sign = neg && !body.zero ? '-' : ''; break;
            default: sign = '';
        }
        var accounting = r.style === 'currency' && r.currencySign === 'accounting' && sign === '-';
        var out = [];
        if (accounting) out.push({ type: 'literal', value: '(' });
        else if (sign) out.push({ type: sign === '-' ? 'minusSign' : 'plusSign', value: sign });
        if (r.style === 'currency') {
            var code = r.currency;
            if (r.currencyDisplay === 'name') {
                var names = CURRENCY_NAME[code], one = !body.zero && body.parts.length === 1 && body.parts[0].value === '1';
                out = out.concat(body.parts);
                out.push({ type: 'literal', value: ' ' });
                out.push({ type: 'currency', value: names ? names[one ? 0 : 1] : code });
            } else if (r.currencyDisplay === 'code') {
                out.push({ type: 'currency', value: code });
                out.push({ type: 'literal', value: ' ' });
                out = out.concat(body.parts);
            } else {
                var sym = (r.currencyDisplay === 'narrowSymbol' && CURRENCY_NARROW[code]) || CURRENCY_SYMBOL[code];
                if (sym) {
                    out.push({ type: 'currency', value: sym });
                } else {
                    out.push({ type: 'currency', value: code });
                    out.push({ type: 'literal', value: ' ' });
                }
                out = out.concat(body.parts);
            }
        } else {
            out = out.concat(body.parts);
            if (r.style === 'percent') out.push({ type: 'percentSign', value: '%' });
            if (r.style === 'unit') {
                var u = r.unit, label;
                if (r.unitDisplay === 'long') {
                    var single = !body.zero && body.parts.length === 1 && body.parts[0].value === '1';
                    label = u.replace(/-per-/g, ' per ') + (single ? '' : 's');
                } else {
                    label = UNIT_SHORT[u] || u;
                }
                if (u !== 'percent' && u !== 'celsius' && u !== 'fahrenheit' && !(r.unitDisplay === 'narrow' && label.length <= 2)) {
                    out.push({ type: 'literal', value: ' ' });
                }
                out.push({ type: 'unit', value: label });
            }
        }
        if (accounting) out.push({ type: 'literal', value: ')' });
        return out;
    }
    function joinParts(parts) {
        var s = '';
        for (var i = 0; i < parts.length; i++) s += parts[i].value;
        return s;
    }
    boundGetter(NumberFormat.prototype, 'format', function (x) {
        return joinParts(numberToParts(this._intl, x));
    }, nfCheck);
    methods(NumberFormat.prototype, {
        formatToParts: function formatToParts(x) {
            return numberToParts(nfCheck(this, 'formatToParts')._intl, x);
        },
        resolvedOptions: function resolvedOptions() {
            var r = nfCheck(this, 'resolvedOptions')._intl, o = {};
            o.locale = r.locale; o.numberingSystem = 'latn'; o.style = r.style;
            if (r.style === 'currency') { o.currency = r.currency; o.currencyDisplay = r.currencyDisplay; o.currencySign = r.currencySign; }
            if (r.style === 'unit') { o.unit = r.unit; o.unitDisplay = r.unitDisplay; }
            o.minimumIntegerDigits = r.minimumIntegerDigits;
            if (r.maximumSignificantDigits !== undefined) {
                o.minimumSignificantDigits = r.minimumSignificantDigits;
                o.maximumSignificantDigits = r.maximumSignificantDigits;
            } else {
                o.minimumFractionDigits = r.minimumFractionDigits;
                o.maximumFractionDigits = r.maximumFractionDigits;
            }
            o.useGrouping = r.useGrouping; o.notation = r.notation;
            if (r.notation === 'compact') o.compactDisplay = r.compactDisplay;
            o.signDisplay = r.signDisplay;
            o.roundingIncrement = 1; o.roundingMode = 'halfExpand'; o.roundingPriority = 'auto'; o.trailingZeroDisplay = 'auto';
            return o;
        }
    });
    tag(NumberFormat.prototype, 'Intl.NumberFormat');

    // ---- PluralRules (CLDR en)
    function PluralRules(locales, options) {
        if (!(this instanceof PluralRules)) throw new TypeError("Constructor Intl.PluralRules requires 'new'");
        var o = toOptions(options), r = {};
        r.locale = resolveLocale(locales);
        getOpt(o, 'localeMatcher', ['lookup', 'best fit'], 'best fit');
        r.type = getOpt(o, 'type', ['cardinal', 'ordinal'], 'cardinal');
        r.nf = new NumberFormat(LOCALE, {
            minimumIntegerDigits: o.minimumIntegerDigits,
            minimumFractionDigits: o.minimumFractionDigits,
            maximumFractionDigits: o.maximumFractionDigits,
            minimumSignificantDigits: o.minimumSignificantDigits,
            maximumSignificantDigits: o.maximumSignificantDigits,
            useGrouping: false
        });
        hidden(this, '_intl', r);
    }
    var prCheck = checker(PluralRules, 'PluralRules');
    function pluralOf(r, n) {
        n = Number(n);
        if (!isFinite(n)) return 'other';
        var s = r.nf.format(Math.abs(n)), dot = s.indexOf('.');
        var ip = dot < 0 ? s : s.slice(0, dot), fp = dot < 0 ? '' : s.slice(dot + 1);
        if (r.type === 'cardinal') return ip === '1' && fp === '' ? 'one' : 'other';
        if (/[1-9]/.test(fp)) return 'other';
        var m10 = Number(ip.slice(-1)), m100 = Number(ip.slice(-2));
        if (m10 === 1 && m100 !== 11) return 'one';
        if (m10 === 2 && m100 !== 12) return 'two';
        if (m10 === 3 && m100 !== 13) return 'few';
        return 'other';
    }
    methods(PluralRules.prototype, {
        select: function select(n) { return pluralOf(prCheck(this, 'select')._intl, n); },
        resolvedOptions: function resolvedOptions() {
            var r = prCheck(this, 'resolvedOptions')._intl, n = r.nf.resolvedOptions(), o = { locale: r.locale, type: r.type };
            o.minimumIntegerDigits = n.minimumIntegerDigits;
            if (n.maximumSignificantDigits !== undefined) {
                o.minimumSignificantDigits = n.minimumSignificantDigits; o.maximumSignificantDigits = n.maximumSignificantDigits;
            } else {
                o.minimumFractionDigits = n.minimumFractionDigits; o.maximumFractionDigits = n.maximumFractionDigits;
            }
            o.pluralCategories = r.type === 'cardinal' ? ['one', 'other'] : ['few', 'one', 'two', 'other'];
            o.roundingIncrement = 1; o.roundingMode = 'halfExpand'; o.roundingPriority = 'auto'; o.trailingZeroDisplay = 'auto';
            return o;
        }
    });
    tag(PluralRules.prototype, 'Intl.PluralRules');

    // ---- Collator (code points after folding; see LIMIT)
    var hasNormalize = (function () {
        try { return typeof ''.normalize === 'function' && 'á'.normalize('NFC') === 'á'; } catch (e) { return false; }
    })();
    var MARKS = /[̀-ͯ᪰-᫿᷀-᷿⃐-⃿︠-︯]/g;
    function decompose(s) { return hasNormalize ? s.normalize('NFD') : s; }
    function cmpCodePoints(a, b) { return a < b ? -1 : a > b ? 1 : 0; }
    function cmpNumeric(a, b) {
        var re = /(\d+)|(\D+)/g, ta = a.match(re) || [], tb = b.match(re) || [];
        for (var i = 0; i < ta.length && i < tb.length; i++) {
            var x = ta[i], y = tb[i], dx = /^\d/.test(x), dy = /^\d/.test(y);
            if (dx && dy) {
                var nx = x.replace(/^0+(?=\d)/, ''), ny = y.replace(/^0+(?=\d)/, '');
                if (nx.length !== ny.length) return nx.length < ny.length ? -1 : 1;
                var c = cmpCodePoints(nx, ny);
                if (c) return c;
            } else {
                var c2 = cmpCodePoints(x, y);
                if (c2) return c2;
            }
        }
        return ta.length === tb.length ? 0 : (ta.length < tb.length ? -1 : 1);
    }
    function Collator(locales, options) {
        if (!(this instanceof Collator)) return new Collator(locales, options);
        var o = toOptions(options), r = {};
        r.locale = resolveLocale(locales);
        r.usage = getOpt(o, 'usage', ['sort', 'search'], 'sort');
        getOpt(o, 'localeMatcher', ['lookup', 'best fit'], 'best fit');
        getOpt(o, 'collation', null, undefined);
        r.numeric = getOpt(o, 'numeric', 'boolean', false);
        r.caseFirst = getOpt(o, 'caseFirst', ['upper', 'lower', 'false'], 'false');
        r.sensitivity = getOpt(o, 'sensitivity', ['base', 'accent', 'case', 'variant'], 'variant');
        r.ignorePunctuation = getOpt(o, 'ignorePunctuation', 'boolean', false);
        hidden(this, '_intl', r);
    }
    var coCheck = checker(Collator, 'Collator');
    function collate(r, a, b) {
        a = String(a); b = String(b);
        if (r.ignorePunctuation) {
            a = a.replace(/[\s!-\/:-@\[-`{-~¡-¿‐-‧]/g, '');
            b = b.replace(/[\s!-\/:-@\[-`{-~¡-¿‐-‧]/g, '');
        }
        var cmp = r.numeric ? cmpNumeric : cmpCodePoints;
        var da = decompose(a), db = decompose(b);
        var ba = da.replace(MARKS, ''), bb = db.replace(MARKS, '');
        var c = cmp(ba.toLowerCase(), bb.toLowerCase());
        if (c || r.sensitivity === 'base') return c;
        if (r.sensitivity === 'accent' || r.sensitivity === 'variant') {
            c = cmp(da.toLowerCase(), db.toLowerCase());
            if (c || r.sensitivity === 'accent') return c;
        }
        // Case: lowercase first unless caseFirst is 'upper'.
        for (var i = 0; i < ba.length && i < bb.length; i++) {
            var x = ba.charAt(i), y = bb.charAt(i);
            if (x !== y && x.toLowerCase() === y.toLowerCase()) {
                var xLower = x === x.toLowerCase();
                var lowerFirst = r.caseFirst !== 'upper';
                return xLower === lowerFirst ? -1 : 1;
            }
        }
        return 0;
    }
    boundGetter(Collator.prototype, 'compare', function (a, b) { return collate(this._intl, a, b); }, coCheck);
    methods(Collator.prototype, {
        resolvedOptions: function resolvedOptions() {
            var r = coCheck(this, 'resolvedOptions')._intl;
            return {
                locale: r.locale, usage: r.usage, sensitivity: r.sensitivity, ignorePunctuation: r.ignorePunctuation,
                collation: 'default', numeric: r.numeric, caseFirst: r.caseFirst
            };
        }
    });
    tag(Collator.prototype, 'Intl.Collator');

    // ---- ListFormat (CLDR en)
    var LIST = {
        conjunction: { long: [' and ', ', and '], short: [' & ', ', & '], narrow: [', ', ', '] },
        disjunction: { long: [' or ', ', or '], short: [' or ', ', or '], narrow: [' or ', ', or '] },
        unit: { long: [', ', ', '], short: [', ', ', '], narrow: [' ', ' '] }
    };
    function ListFormat(locales, options) {
        if (!(this instanceof ListFormat)) throw new TypeError("Constructor Intl.ListFormat requires 'new'");
        var o = toOptions(options), r = {};
        r.locale = resolveLocale(locales);
        getOpt(o, 'localeMatcher', ['lookup', 'best fit'], 'best fit');
        r.type = getOpt(o, 'type', ['conjunction', 'disjunction', 'unit'], 'conjunction');
        r.style = getOpt(o, 'style', ['long', 'short', 'narrow'], 'long');
        hidden(this, '_intl', r);
    }
    var lfCheck = checker(ListFormat, 'ListFormat');
    function listParts(r, list) {
        var items = [];
        if (list !== undefined) {
            var arr = Array.from(list);
            for (var i = 0; i < arr.length; i++) {
                if (typeof arr[i] !== 'string') throw new TypeError('Iterable yielded ' + String(arr[i]) + ' which is not a string');
                items.push(arr[i]);
            }
        }
        var pat = LIST[r.type][r.style], mid = r.type === 'unit' && r.style === 'narrow' ? ' ' : ', ', out = [];
        for (var j = 0; j < items.length; j++) {
            if (j > 0) {
                var sep = items.length === 2 ? pat[0] : (j === items.length - 1 ? pat[1] : mid);
                out.push({ type: 'literal', value: sep });
            }
            out.push({ type: 'element', value: items[j] });
        }
        return out;
    }
    methods(ListFormat.prototype, {
        format: function format(list) { return joinParts(listParts(lfCheck(this, 'format')._intl, list)); },
        formatToParts: function formatToParts(list) { return listParts(lfCheck(this, 'formatToParts')._intl, list); },
        resolvedOptions: function resolvedOptions() {
            var r = lfCheck(this, 'resolvedOptions')._intl;
            return { locale: r.locale, type: r.type, style: r.style };
        }
    });
    tag(ListFormat.prototype, 'Intl.ListFormat');

    // ---- RelativeTimeFormat (CLDR en)
    var RT_UNITS = ['second', 'minute', 'hour', 'day', 'week', 'month', 'quarter', 'year'];
    var RT_SHORT = {
        second: ['sec.', 'sec.'], minute: ['min.', 'min.'], hour: ['hr.', 'hr.'], day: ['day', 'days'],
        week: ['wk.', 'wk.'], month: ['mo.', 'mo.'], quarter: ['qtr.', 'qtrs.'], year: ['yr.', 'yr.']
    };
    var RT_AUTO = {
        second: { 0: 'now' }, minute: { 0: 'this minute' }, hour: { 0: 'this hour' },
        day: { '-1': 'yesterday', 0: 'today', 1: 'tomorrow' }
    };
    function RelativeTimeFormat(locales, options) {
        if (!(this instanceof RelativeTimeFormat)) throw new TypeError("Constructor Intl.RelativeTimeFormat requires 'new'");
        var o = toOptions(options), r = {};
        r.locale = resolveLocale(locales);
        getOpt(o, 'localeMatcher', ['lookup', 'best fit'], 'best fit');
        getOpt(o, 'numberingSystem', null, undefined);
        r.style = getOpt(o, 'style', ['long', 'short', 'narrow'], 'long');
        r.numeric = getOpt(o, 'numeric', ['always', 'auto'], 'always');
        r.nf = new NumberFormat(LOCALE);
        hidden(this, '_intl', r);
    }
    var rtCheck = checker(RelativeTimeFormat, 'RelativeTimeFormat');
    function relativeParts(r, value, unit) {
        value = Number(value);
        if (!isFinite(value)) throw new RangeError('Invalid value: ' + String(value));
        var u = String(unit);
        if (RT_UNITS.indexOf(u) < 0 && u.slice(-1) === 's' && RT_UNITS.indexOf(u.slice(0, -1)) >= 0) u = u.slice(0, -1);
        if (RT_UNITS.indexOf(u) < 0) throw new RangeError('Invalid unit argument for format() \'' + String(unit) + '\'');
        if (r.numeric === 'auto' && (value === -1 || value === 0 || value === 1)) {
            var key = value === 0 ? 0 : value, fixed = RT_AUTO[u] && RT_AUTO[u][key];
            if (!fixed) {
                var label = r.style === 'long' ? u : RT_SHORT[u][0];
                fixed = (value < 0 ? 'last ' : value > 0 ? 'next ' : 'this ') + label;
            }
            return [{ type: 'literal', value: fixed }];
        }
        var past = value < 0 || (value === 0 && 1 / value < 0);
        var num = numberToParts(r.nf._intl, Math.abs(value));
        var one = num.length === 1 && num[0].value === '1';
        var name = r.style === 'long' ? (one ? u : u + 's') : RT_SHORT[u][one ? 0 : 1];
        var out = [];
        if (!past) out.push({ type: 'literal', value: 'in ' });
        for (var i = 0; i < num.length; i++) out.push({ type: num[i].type, value: num[i].value, unit: u });
        out.push({ type: 'literal', value: ' ' + name + (past ? ' ago' : '') });
        return out;
    }
    methods(RelativeTimeFormat.prototype, {
        format: function format(value, unit) { return joinParts(relativeParts(rtCheck(this, 'format')._intl, value, unit)); },
        formatToParts: function formatToParts(value, unit) { return relativeParts(rtCheck(this, 'formatToParts')._intl, value, unit); },
        resolvedOptions: function resolvedOptions() {
            var r = rtCheck(this, 'resolvedOptions')._intl;
            return { locale: r.locale, style: r.style, numeric: r.numeric, numberingSystem: 'latn' };
        }
    });
    tag(RelativeTimeFormat.prototype, 'Intl.RelativeTimeFormat');

    // ---- DateTimeFormat (Gregorian; 'UTC' or the default zone)
    var MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
    var DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
    var UTC_NAMES = ['UTC', 'ETC/UTC', 'GMT', 'ETC/GMT', 'UCT', 'ETC/UCT', 'ZULU', 'ETC/ZULU', 'UNIVERSAL', 'ETC/UNIVERSAL',
        'GREENWICH', 'ETC/GREENWICH', 'GMT0', 'ETC/GMT0', 'GMT+0', 'ETC/GMT+0', 'GMT-0', 'ETC/GMT-0'];
    var COMPONENTS = [
        ['weekday', ['narrow', 'short', 'long']], ['era', ['narrow', 'short', 'long']],
        ['year', ['2-digit', 'numeric']], ['month', ['2-digit', 'numeric', 'narrow', 'short', 'long']],
        ['day', ['2-digit', 'numeric']], ['dayPeriod', ['narrow', 'short', 'long']],
        ['hour', ['2-digit', 'numeric']], ['minute', ['2-digit', 'numeric']], ['second', ['2-digit', 'numeric']],
        ['fractionalSecondDigits', null],
        ['timeZoneName', ['short', 'long', 'shortOffset', 'longOffset', 'shortGeneric', 'longGeneric']]
    ];
    var STYLES = ['full', 'long', 'medium', 'short'];
    function pad2(n) { return n < 10 ? '0' + n : String(n); }
    function offsetName(minutesEast, long) {
        if (minutesEast === 0) return 'GMT';
        var sign = minutesEast < 0 ? '-' : '+', a = Math.abs(minutesEast), h = Math.floor(a / 60), m = a % 60;
        if (long) return 'GMT' + sign + pad2(h) + ':' + pad2(m);
        return 'GMT' + sign + h + (m ? ':' + pad2(m) : '');
    }
    function defaultZoneName() {
        var off = new Date().getTimezoneOffset();
        if (!off) return 'UTC';
        if (off % 60 === 0) return 'Etc/GMT' + (off > 0 ? '+' : '-') + Math.abs(off / 60);
        var a = Math.abs(off);
        return (off > 0 ? '-' : '+') + pad2(Math.floor(a / 60)) + ':' + pad2(a % 60);
    }
    function resolveZone(tz) {
        if (tz === undefined) return { name: defaultZoneName(), utc: false };
        var s = String(tz), up = s.toUpperCase();
        if (UTC_NAMES.indexOf(up) >= 0) return { name: 'UTC', utc: true };
        var def = defaultZoneName();
        if (up === def.toUpperCase()) return { name: def, utc: def === 'UTC' };
        if (!/^[A-Za-z][A-Za-z0-9_+\-]*(\/[A-Za-z0-9_+\-]+)*$/.test(s) && !/^[+-]\d{2}(:?\d{2})?$/.test(s)) {
            throw new RangeError('Invalid time zone specified: ' + s);
        }
        // Another zone: not modelled (see LIMIT); format in the default zone.
        return { name: def, utc: def === 'UTC' };
    }
    function DateTimeFormat(locales, options) {
        if (!(this instanceof DateTimeFormat)) return new DateTimeFormat(locales, options);
        hidden(this, '_intl', makeDateTime(locales, options, 'any', 'date'));
    }
    var dtfCheck = checker(DateTimeFormat, 'DateTimeFormat');
    function makeDateTime(locales, options, required, defaults) {
        var o = toOptions(options), r = {};
        r.locale = resolveLocale(locales);
        getOpt(o, 'localeMatcher', ['lookup', 'best fit'], 'best fit');
        var cal = getOpt(o, 'calendar', null, undefined);
        getOpt(o, 'numberingSystem', null, undefined);
        var h12 = o.hour12 === undefined ? undefined : Boolean(o.hour12);
        var hc = getOpt(o, 'hourCycle', ['h11', 'h12', 'h23', 'h24'], undefined);
        r.zone = resolveZone(o.timeZone);
        var c = {}, any = false, anyDate = false, anyTime = false;
        for (var i = 0; i < COMPONENTS.length; i++) {
            var name = COMPONENTS[i][0], v;
            if (name === 'fractionalSecondDigits') v = getNum(o, name, 1, 3, undefined);
            else v = getOpt(o, name, COMPONENTS[i][1], undefined);
            if (v !== undefined) {
                c[name] = v;
                if (name !== 'timeZoneName') any = true;
                if (name === 'weekday' || name === 'year' || name === 'month' || name === 'day') anyDate = true;
                if (name === 'dayPeriod' || name === 'hour' || name === 'minute' || name === 'second' || name === 'fractionalSecondDigits') anyTime = true;
            }
        }
        getOpt(o, 'formatMatcher', ['basic', 'best fit'], 'best fit');
        r.dateStyle = getOpt(o, 'dateStyle', STYLES, undefined);
        r.timeStyle = getOpt(o, 'timeStyle', STYLES, undefined);
        if (cal !== undefined && cal !== 'gregory' && cal !== 'iso8601') cal = undefined;
        if (r.dateStyle || r.timeStyle) {
            if (any || c.timeZoneName) {
                throw new TypeError("Can't set option " + Object.keys(c)[0] + ' when ' + (r.dateStyle ? 'dateStyle' : 'timeStyle') + ' is used');
            }
            if (required === 'date' && !r.dateStyle) throw new TypeError('Invalid option : timeStyle');
            if (required === 'time' && !r.timeStyle) throw new TypeError('Invalid option : dateStyle');
            var ds = r.dateStyle, ts = r.timeStyle;
            if (ds === 'full') { c.weekday = 'long'; c.month = 'long'; c.day = 'numeric'; c.year = 'numeric'; }
            if (ds === 'long') { c.month = 'long'; c.day = 'numeric'; c.year = 'numeric'; }
            if (ds === 'medium') { c.month = 'short'; c.day = 'numeric'; c.year = 'numeric'; }
            if (ds === 'short') { c.month = 'numeric'; c.day = 'numeric'; c.year = '2-digit'; }
            if (ts) { c.hour = 'numeric'; c.minute = '2-digit'; }
            if (ts === 'full' || ts === 'long' || ts === 'medium') c.second = '2-digit';
            if (ts === 'full') c.timeZoneName = 'long';
            if (ts === 'long') c.timeZoneName = 'short';
            r.at = ds === 'full' || ds === 'long';
        } else {
            var need = required === 'date' ? !anyDate : required === 'time' ? !anyTime : !(anyDate || anyTime);
            if (need && (defaults === 'date' || defaults === 'all')) { c.year = 'numeric'; c.month = 'numeric'; c.day = 'numeric'; }
            if (need && (defaults === 'time' || defaults === 'all')) { c.hour = 'numeric'; c.minute = 'numeric'; c.second = 'numeric'; }
            r.at = c.month === 'long' && c.day !== undefined;
        }
        if (c.hour !== undefined) {
            r.hourCycle = h12 !== undefined ? (h12 ? 'h12' : 'h23') : (hc || 'h12');
            if (r.hourCycle === 'h23' || r.hourCycle === 'h24') c.hour = '2-digit';
            if (c.minute !== undefined) c.minute = '2-digit';
            if (c.second !== undefined && (c.minute !== undefined || c.hour !== undefined)) c.second = '2-digit';
        }
        r.c = c;
        return r;
    }
    function dateFields(r, t) {
        var d = new Date(t), u = r.zone.utc;
        return {
            y: u ? d.getUTCFullYear() : d.getFullYear(), mo: u ? d.getUTCMonth() : d.getMonth(),
            d: u ? d.getUTCDate() : d.getDate(), wd: u ? d.getUTCDay() : d.getDay(),
            h: u ? d.getUTCHours() : d.getHours(), mi: u ? d.getUTCMinutes() : d.getMinutes(),
            s: u ? d.getUTCSeconds() : d.getSeconds(), ms: u ? d.getUTCMilliseconds() : d.getMilliseconds(),
            off: u ? 0 : -d.getTimezoneOffset()
        };
    }
    function zoneLabel(r, f, style) {
        var long = style === 'long' || style === 'longOffset' || style === 'longGeneric';
        if (f.off === 0 && (style === 'short' || style === 'long' || style === 'shortGeneric' || style === 'longGeneric')) {
            return long ? 'Coordinated Universal Time' : 'UTC';
        }
        return offsetName(f.off, long);
    }
    function dateTimeParts(r, x) {
        var t = x === undefined ? Date.now() : Number(x);
        if (!isFinite(t) || Math.abs(t) > 8.64e15) throw new RangeError('Invalid time value');
        var c = r.c, f = dateFields(r, t), date = [], time = [];
        function lit(v, list) { list.push({ type: 'literal', value: v }); }
        // Date portion.
        var year = c.era && f.y <= 0 ? 1 - f.y : f.y;
        var yv = c.year === '2-digit' ? pad2(((year % 100) + 100) % 100) : String(year);
        var textMonth = c.month === 'short' || c.month === 'long' || c.month === 'narrow';
        var wd = c.weekday && (c.weekday === 'long' ? DAYS[f.wd] : c.weekday === 'short' ? DAYS[f.wd].slice(0, 3) : DAYS[f.wd].charAt(0));
        var dayv = c.day === '2-digit' ? pad2(f.d) : String(f.d);
        if (textMonth) {
            var mv = c.month === 'long' ? MONTHS[f.mo] : c.month === 'short' ? MONTHS[f.mo].slice(0, 3) : MONTHS[f.mo].charAt(0);
            if (wd) { date.push({ type: 'weekday', value: wd }); lit(', ', date); }
            date.push({ type: 'month', value: mv });
            if (c.day) { lit(' ', date); date.push({ type: 'day', value: dayv }); }
            if (c.year) { lit(c.day ? ', ' : ' ', date); date.push({ type: 'year', value: yv }); }
        } else {
            var nums = [];
            if (c.month) nums.push({ type: 'month', value: c.month === '2-digit' ? pad2(f.mo + 1) : String(f.mo + 1) });
            if (c.day) nums.push({ type: 'day', value: dayv });
            if (c.year) nums.push({ type: 'year', value: yv });
            if (wd) { date.push({ type: 'weekday', value: wd }); if (nums.length) lit(', ', date); }
            for (var i = 0; i < nums.length; i++) { if (i) lit('/', date); date.push(nums[i]); }
        }
        if (c.era && date.length) {
            var bc = f.y <= 0, era = c.era === 'long' ? (bc ? 'Before Christ' : 'Anno Domini') : c.era === 'short' ? (bc ? 'BC' : 'AD') : (bc ? 'B' : 'A');
            lit(' ', date); date.push({ type: 'era', value: era });
        }
        // Time portion.
        var twelve = r.hourCycle === 'h12' || r.hourCycle === 'h11';
        if (c.hour) {
            var h = f.h;
            if (r.hourCycle === 'h12') h = h % 12 || 12;
            else if (r.hourCycle === 'h11') h = h % 12;
            else if (r.hourCycle === 'h24') h = h || 24;
            time.push({ type: 'hour', value: c.hour === '2-digit' ? pad2(h) : String(h) });
            if (c.minute) { lit(':', time); time.push({ type: 'minute', value: pad2(f.mi) }); }
            if (c.second) { lit(':', time); time.push({ type: 'second', value: pad2(f.s) }); }
        } else if (c.minute) {
            time.push({ type: 'minute', value: c.second ? pad2(f.mi) : String(f.mi) });
            if (c.second) { lit(':', time); time.push({ type: 'second', value: pad2(f.s) }); }
        } else if (c.second) {
            time.push({ type: 'second', value: c.second === '2-digit' ? pad2(f.s) : String(f.s) });
        }
        if (c.fractionalSecondDigits) {
            if (time.length) lit('.', time);
            time.push({ type: 'fractionalSecond', value: String(f.ms + 1000).slice(1, 1 + c.fractionalSecondDigits) });
        }
        if (c.hour && twelve) {
            var period;
            if (c.dayPeriod) {
                period = f.h === 12 && f.mi === 0 ? 'noon' : f.h >= 6 && f.h < 12 ? 'in the morning' : f.h >= 12 && f.h < 18 ? 'in the afternoon' : f.h >= 18 && f.h < 21 ? 'in the evening' : 'at night';
            } else {
                period = f.h < 12 ? 'AM' : 'PM';
            }
            lit(' ', time); time.push({ type: 'dayPeriod', value: period });
        } else if (c.dayPeriod && !c.hour) {
            time.push({ type: 'dayPeriod', value: f.h >= 6 && f.h < 12 ? 'in the morning' : f.h >= 12 && f.h < 18 ? 'in the afternoon' : f.h >= 18 && f.h < 21 ? 'in the evening' : 'at night' });
        }
        var out = date.slice();
        if (time.length) {
            if (out.length) lit(r.at ? ' at ' : ', ', out);
            out = out.concat(time);
        }
        if (c.timeZoneName) {
            if (out.length) lit(time.length ? ' ' : ', ', out);
            out.push({ type: 'timeZoneName', value: zoneLabel(r, f, c.timeZoneName) });
        }
        return out;
    }
    boundGetter(DateTimeFormat.prototype, 'format', function (x) {
        return joinParts(dateTimeParts(this._intl, x));
    }, dtfCheck);
    methods(DateTimeFormat.prototype, {
        formatToParts: function formatToParts(x) { return dateTimeParts(dtfCheck(this, 'formatToParts')._intl, x); },
        resolvedOptions: function resolvedOptions() {
            var r = dtfCheck(this, 'resolvedOptions')._intl, o = {
                locale: r.locale, calendar: 'gregory', numberingSystem: 'latn', timeZone: r.zone.name
            };
            if (r.hourCycle) { o.hourCycle = r.hourCycle; o.hour12 = r.hourCycle === 'h12' || r.hourCycle === 'h11'; }
            if (r.dateStyle || r.timeStyle) {
                if (r.dateStyle) o.dateStyle = r.dateStyle;
                if (r.timeStyle) o.timeStyle = r.timeStyle;
                return o;
            }
            for (var i = 0; i < COMPONENTS.length; i++) {
                var n = COMPONENTS[i][0];
                if (r.c[n] !== undefined) o[n] = r.c[n];
            }
            return o;
        }
    });
    tag(DateTimeFormat.prototype, 'Intl.DateTimeFormat');

    // ---- Segmenter (ECMA-402 Intl.Segmenter)
    function Segmenter(locales, options) {
        if (!(this instanceof Segmenter)) throw new TypeError("Constructor Intl.Segmenter requires 'new'");
        var o = toOptions(options), r = {};
        r.locale = resolveLocale(locales);
        getOpt(o, 'localeMatcher', ['lookup', 'best fit'], 'best fit');
        r.granularity = getOpt(o, 'granularity', ['grapheme', 'word', 'sentence'], 'grapheme');
        hidden(this, '_intl', r);
    }
    var segCheck = checker(Segmenter, 'Segmenter');

    function segmentGraphemes(str) {
        var segments = [], i = 0, len = str.length;
        while (i < len) {
            var start = i;
            var code = str.charCodeAt(i);
            if (code >= 0xD800 && code <= 0xDBFF && i + 1 < len) {
                var next = str.charCodeAt(i + 1);
                if (next >= 0xDC00 && next <= 0xDFFF) {
                    i += 2;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
            while (i < len) {
                var c = str.charCodeAt(i);
                if ((c >= 0x0300 && c <= 0x036F) || (c >= 0x1DC0 && c <= 0x1DFF) || (c >= 0x20D0 && c <= 0x20FF) || (c >= 0xFE20 && c <= 0xFE2F) || c === 0x200D) {
                    i++;
                    if (c === 0x200D && i < len) {
                        var c2 = str.charCodeAt(i);
                        if (c2 >= 0xD800 && c2 <= 0xDBFF && i + 1 < len) i += 2;
                        else i += 1;
                    }
                } else {
                    break;
                }
            }
            segments.push({ segment: str.slice(start, i), index: start, input: str, isWordLike: undefined });
        }
        return segments;
    }

    function segmentWords(str) {
        var segments = [], i = 0, len = str.length;
        var isWordChar = function (c) {
            var code = c.charCodeAt(0);
            return (code >= 65 && code <= 90) || (code >= 97 && code <= 122) || (code >= 48 && code <= 57) || code === 95 || code > 127;
        };
        while (i < len) {
            var start = i;
            var wordLike = isWordChar(str.charAt(i));
            if (wordLike) {
                while (i < len && isWordChar(str.charAt(i))) i++;
            } else {
                var isSpace = /\s/.test(str.charAt(i));
                if (isSpace) {
                    while (i < len && /\s/.test(str.charAt(i))) i++;
                } else {
                    i++;
                }
            }
            segments.push({ segment: str.slice(start, i), index: start, input: str, isWordLike: wordLike });
        }
        return segments;
    }

    function segmentSentences(str) {
        var segments = [], i = 0, len = str.length;
        while (i < len) {
            var start = i;
            while (i < len) {
                var ch = str.charAt(i);
                i++;
                if (ch === '.' || ch === '!' || ch === '?' || ch === '\n') {
                    while (i < len && str.charAt(i) === ' ') i++;
                    break;
                }
            }
            segments.push({ segment: str.slice(start, i), index: start, input: str, isWordLike: undefined });
        }
        return segments;
    }

    function getSegments(granularity, str) {
        if (granularity === 'word') return segmentWords(str);
        if (granularity === 'sentence') return segmentSentences(str);
        return segmentGraphemes(str);
    }

    function Segments(r, input) {
        hidden(this, '_intl', r);
        hidden(this, '_input', input);
    }
    methods(Segments.prototype, {
        containing: function containing(index) {
            index = Number(index);
            if (isNaN(index)) index = 0;
            else index = Math.floor(index);
            var str = this._input;
            if (index < 0 || index >= str.length) return undefined;
            var segs = getSegments(this._intl.granularity, str);
            for (var i = 0; i < segs.length; i++) {
                var s = segs[i];
                if (index >= s.index && index < s.index + s.segment.length) {
                    return s;
                }
            }
            return undefined;
        }
    });
    if (typeof Symbol === 'function' && Symbol.iterator) {
        hidden(Segments.prototype, Symbol.iterator, function () {
            var segs = getSegments(this._intl.granularity, this._input);
            var idx = 0;
            var iter = {};
            methods(iter, {
                next: function next() {
                    if (idx < segs.length) {
                        return { value: segs[idx++], done: false };
                    }
                    return { value: undefined, done: true };
                }
            });
            hidden(iter, Symbol.iterator, function () { return this; });
            return iter;
        });
    }
    tag(Segments.prototype, 'Intl.Segments');

    methods(Segmenter.prototype, {
        segment: function segment(input) {
            var r = segCheck(this, 'segment')._intl;
            if (input === undefined) input = '';
            else input = String(input);
            return new Segments(r, input);
        },
        resolvedOptions: function resolvedOptions() {
            var r = segCheck(this, 'resolvedOptions')._intl;
            return { locale: r.locale, granularity: r.granularity };
        }
    });
    tag(Segmenter.prototype, 'Intl.Segmenter');

    // ---- the namespace
    var Intl = {};
    methods(Intl, {
        getCanonicalLocales: function getCanonicalLocales(locales) { return canonicalList(locales); },
        NumberFormat: NumberFormat, DateTimeFormat: DateTimeFormat, PluralRules: PluralRules, Collator: Collator,
        ListFormat: ListFormat, RelativeTimeFormat: RelativeTimeFormat, Segmenter: Segmenter
    });
    [NumberFormat, DateTimeFormat, PluralRules, Collator, ListFormat, RelativeTimeFormat, Segmenter].forEach(function (C) {
        hidden(C, 'supportedLocalesOf', function supportedLocalesOf(locales, options) { return supportedOf(locales, options); });
    });
    tag(Intl, 'Intl');
    hidden(g, 'Intl', Intl);

    // ---- toLocaleString and friends, through the formatters above
    var defaults = Object.create(null);
    function cached(key, make, locales, options) {
        if (locales !== undefined || options !== undefined) return make();
        return defaults[key] || (defaults[key] = make());
    }
    hidden(Number.prototype, 'toLocaleString', function toLocaleString(locales, options) {
        var x = Number.prototype.valueOf.call(this);
        return cached('n', function () { return new NumberFormat(locales, options); }, locales, options).format(x);
    });
    if (typeof BigInt === 'function') {
        hidden(BigInt.prototype, 'toLocaleString', function toLocaleString(locales, options) {
            var x = BigInt.prototype.valueOf.call(this);
            return cached('n', function () { return new NumberFormat(locales, options); }, locales, options).format(x);
        });
    }
    function dateMethod(name, required, defs) {
        hidden(Date.prototype, name, function (locales, options) {
            var t = Date.prototype.getTime.call(this);
            if (isNaN(t)) return 'Invalid Date';
            var dtf = cached(name, function () {
                var f = Object.create(DateTimeFormat.prototype);
                hidden(f, '_intl', makeDateTime(locales, options, required, defs));
                return f;
            }, locales, options);
            return joinParts(dateTimeParts(dtf._intl, t));
        });
    }
    dateMethod('toLocaleString', 'any', 'all');
    dateMethod('toLocaleDateString', 'date', 'date');
    dateMethod('toLocaleTimeString', 'time', 'time');
})(globalThis);
