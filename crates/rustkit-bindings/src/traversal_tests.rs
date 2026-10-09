//! NodeFilter, TreeWalker and NodeIterator (web_traversal.js).

use super::*;

const PAGE: &str = "<html><body><div id=a><p id=p1>one<b id=b>bold</b></p><!--c--><p id=p2>two</p></div><span id=s>s</span></body></html>";

fn bound() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b.evaluate("var a = document.getElementById('a');").unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(x) => x.to_string(),
        JsValue::Number(n) => format!("{n}"),
        JsValue::Undefined => "undefined".into(),
        JsValue::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

#[test]
fn nodefilter_has_the_constants() {
    let b = bound();
    assert_eq!(
        ev(&b, "[NodeFilter.SHOW_ALL, NodeFilter.SHOW_ELEMENT, NodeFilter.SHOW_TEXT, NodeFilter.SHOW_COMMENT, NodeFilter.SHOW_DOCUMENT, NodeFilter.FILTER_ACCEPT, NodeFilter.FILTER_REJECT, NodeFilter.FILTER_SKIP].join()"),
        "4294967295,1,4,128,256,1,2,3"
    );
}

#[test]
fn a_tree_walker_visits_in_tree_order() {
    let b = bound();
    assert_eq!(
        ev(&b, "var w = document.createTreeWalker(a, NodeFilter.SHOW_ELEMENT); var seen = []; for (var n; (n = w.nextNode());) seen.push(n.id); seen.join()"),
        "p1,b,p2"
    );
    assert_eq!(ev(&b, "[w.root === a, w.currentNode.id, w.whatToShow, String(w.filter), w instanceof TreeWalker].join()"), "true,p2,1,null,true");
    assert_eq!(ev(&b, "var back = []; for (var n; (n = w.previousNode());) back.push(n.id); back.join()"), "b,p1,a", "previousNode can return the root, as in the specification");
    // Text and comments with SHOW_ALL.
    assert_eq!(
        ev(&b, "var w2 = document.createTreeWalker(a, NodeFilter.SHOW_ALL); var all = []; for (var n; (n = w2.nextNode());) all.push(n.nodeName); all.join()"),
        "P,#text,B,#text,#comment,P,#text"
    );
}

#[test]
fn a_filter_accepts_rejects_or_skips() {
    let b = bound();
    // REJECT prunes the subtree (p1 and its <b>); SKIP only hides the node.
    assert_eq!(
        ev(&b, "var w = document.createTreeWalker(a, NodeFilter.SHOW_ELEMENT, function (n) { return n.id === 'p1' ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT; }); var s = []; for (var n; (n = w.nextNode());) s.push(n.id); s.join()"),
        "p2"
    );
    assert_eq!(
        ev(&b, "var w = document.createTreeWalker(a, NodeFilter.SHOW_ELEMENT, { acceptNode: function (n) { return n.id === 'p1' ? NodeFilter.FILTER_SKIP : NodeFilter.FILTER_ACCEPT; } }); var s = []; for (var n; (n = w.nextNode());) s.push(n.id); s.join()"),
        "b,p2"
    );
}

#[test]
fn tree_walker_navigation_methods() {
    let b = bound();
    assert_eq!(
        ev(&b, "var w = document.createTreeWalker(a, NodeFilter.SHOW_ELEMENT); [w.firstChild().id, w.nextSibling().id, String(w.nextSibling()), w.previousSibling().id, w.parentNode() === a, w.lastChild().id, w.currentNode.id].join()"),
        "p1,p2,null,p1,true,p2,p2"
    );
    assert_eq!(ev(&b, "w.currentNode = document.getElementById('p2'); [w.parentNode().id, w.currentNode.id].join()"), "a,a");
}

#[test]
fn a_node_iterator_walks_forward_and_back() {
    let b = bound();
    assert_eq!(
        ev(&b, "var it = document.createNodeIterator(a, NodeFilter.SHOW_ELEMENT); var f = []; for (var n; (n = it.nextNode());) f.push(n.id || n.nodeName); f.join()"),
        "a,p1,b,p2"
    );
    assert_eq!(ev(&b, "[it.root === a, it.referenceNode.id, it.pointerBeforeReferenceNode, it instanceof NodeIterator].join()"), "true,p2,false,true");
    assert_eq!(ev(&b, "var r = []; for (var n; (n = it.previousNode());) r.push(n.id); r.join()"), "p2,b,p1,a");
    assert_eq!(ev(&b, "typeof it.detach"), "function");
}

#[test]
fn creation_validates() {
    let b = bound();
    assert_eq!(ev(&b, "var e; try { document.createTreeWalker(null); } catch (x) { e = x.name; } e"), "TypeError");
    assert_eq!(ev(&b, "var e2; try { document.createNodeIterator(a, 1, 5); } catch (x) { e2 = x.name; } e2"), "TypeError");
    assert_eq!(ev(&b, "var e3; try { new TreeWalker(); } catch (x) { e3 = x.name; } e3"), "TypeError");
}
