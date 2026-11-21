// XMLSerializer (DOM Parsing §3.2 "XML serialization"; STR-11), over the
// public DOM. What it writes for the trees an HTML page hands it:
//
// * an element whose namespace differs from its parent's (or a root) carries
//   an `xmlns` declaration; HTML elements are written with their local name;
// * an HTML element with no children is `<br />` when void and `<b></b>`
//   otherwise; an empty element of another namespace is `<g/>`;
// * attribute values escape & < > " and tab / newline / return; text escapes
//   & < >;
// * a prefixed attribute (`xlink:href`) declares its prefix on the element
//   when no ancestor being serialized has.
//
// Not here: the well-formedness checks (a serializer in a browser never
// throws for them either), CDATA sections and processing instructions from
// XML documents, which the HTML parser does not produce.
(function (g) {
    if (g.XMLSerializer !== undefined || typeof g.Node !== 'function') return;
    var HTML_NS = 'http://www.w3.org/1999/xhtml';
    var PREFIXES = { xlink: 'http://www.w3.org/1999/xlink', xml: 'http://www.w3.org/XML/1998/namespace' };
    var VOID = { area: 1, base: 1, basefont: 1, bgsound: 1, br: 1, col: 1, embed: 1, frame: 1, hr: 1, img: 1,
                 input: 1, keygen: 1, link: 1, menuitem: 1, meta: 1, param: 1, source: 1, track: 1, wbr: 1 };

    function text(s) {
        return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    }
    function attribute(s) {
        return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
            .replace(/\t/g, '&#9;').replace(/\n/g, '&#10;').replace(/\r/g, '&#13;');
    }

    // `inherited` is the default namespace in scope, `declared` the prefixes.
    function element(el, inherited, declared) {
        var ns = el.namespaceURI, html = ns === HTML_NS;
        var name = html ? el.localName : el.tagName;
        var out = '<' + name, seen = {};
        var attrs = el.attributes, i, a;
        if (ns !== inherited && ns !== null) {
            out += ' xmlns="' + attribute(ns) + '"';
        }
        for (i = 0; i < attrs.length; i++) {
            a = attrs[i];
            // The declaration above stands for an `xmlns` attribute; one
            // naming another namespace cannot be honoured and is dropped.
            if (a.name === 'xmlns') continue;
            var colon = a.name.indexOf(':'), prefix = colon > 0 ? a.name.slice(0, colon) : '';
            if (prefix === 'xmlns') { seen[a.name.slice(colon + 1)] = true; }
            out += ' ' + a.name + '="' + attribute(a.value) + '"';
            if (prefix && prefix !== 'xml' && prefix !== 'xmlns' && PREFIXES[prefix] && !declared[prefix] && !seen[prefix]) {
                var known = false;
                for (var j = 0; j < attrs.length; j++) if (attrs[j].name === 'xmlns:' + prefix) known = true;
                if (!known) {
                    out += ' xmlns:' + prefix + '="' + PREFIXES[prefix] + '"';
                    seen[prefix] = true;
                }
            }
        }
        var scope = declared;
        for (var p in seen) {
            if (scope === declared) { scope = {}; for (var q in declared) scope[q] = true; }
            scope[p] = true;
        }
        // A template's children live in its content fragment.
        var kids = html && el.localName === 'template' && el.content ? el.content.childNodes : el.childNodes;
        if (kids.length === 0) {
            if (!html) return out + '/>';
            if (VOID[el.localName]) return out + ' />';
            return out + '></' + name + '>';
        }
        out += '>';
        for (i = 0; i < kids.length; i++) out += node(kids[i], ns, scope);
        return out + '</' + name + '>';
    }
    function node(n, inherited, declared) {
        var i, out = '';
        switch (n.nodeType) {
            case 1: return element(n, inherited, declared);
            case 3: return text(n.data);
            case 8: return '<!--' + n.data + '-->';
            case 7: return '<?' + n.target + ' ' + n.data + '?>';
            case 10:
                return '<!DOCTYPE ' + n.name + (n.publicId ? ' PUBLIC "' + n.publicId + '"' : '') +
                    (n.systemId ? (n.publicId ? '' : ' SYSTEM') + ' "' + n.systemId + '"' : '') + '>';
            case 9:
            case 11:
                for (i = 0; i < n.childNodes.length; i++) out += node(n.childNodes[i], inherited, declared);
                return out;
        }
        return '';
    }

    var XMLSerializer = function XMLSerializer() {
        if (!new.target) throw new TypeError("Failed to construct 'XMLSerializer': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
    };
    XMLSerializer.prototype.serializeToString = function serializeToString(root) {
        if (!(this instanceof XMLSerializer)) throw new TypeError('Illegal invocation');
        if (!(root instanceof g.Node)) {
            throw new TypeError("Failed to execute 'serializeToString' on 'XMLSerializer': parameter 1 is not of type 'Node'.");
        }
        return node(root, null, {});
    };
    Object.defineProperty(XMLSerializer.prototype, Symbol.toStringTag, { value: 'XMLSerializer', configurable: true });
    Object.defineProperty(g, 'XMLSerializer', { value: XMLSerializer, writable: true, configurable: true });
})(globalThis);
