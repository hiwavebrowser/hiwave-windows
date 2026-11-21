//! Named access on the document's collections: `document.forms["id"]`, `document.forms.name`,
//! and the same on `images`, `links`, `anchors`, `scripts`, `embeds` and `plugins`.
//!
//! The expected values are the pinned Chrome's (CfT 148.0.7778.216, headless, 2026-11-15): the
//! probe page `docs/diagnostics/2026-11-15/named_collections.html` with `named_collections_page.js`
//! was run there by `named_collections_chrome.mjs`, and its answers are
//! `named_collections_cft148.json`. The tests run the same markup and the same script here and
//! compare step by step. What Chrome does:
//!   - a named property is an element's id, or its `name` attribute when the element is an HTML
//!     element; a match by id anywhere in the collection comes before a match by name;
//!     the first in tree order wins among ids, and among names;
//!   - a name that is an array index is never a named property (`forms["40"]` with 21 forms is
//!     undefined), a name the interface or `Object.prototype` has is the interface's
//!     (`length`, `item`, `toString`), and an empty name is no name;
//!   - a named property is an own, read-only, non-enumerable property: `in` and
//!     `hasOwnProperty` see it, `Object.keys` and `for-in` do not, assignment does nothing (it
//!     throws in strict code), and it cannot be deleted or redefined;
//!   - `getElementsByName` gives a NodeList, which has no named properties.
//!
//! Where this engine still answers otherwise is listed in [`DIFFERENT`], each with its reason,
//! and is asserted to differ so that a later fix is noticed here.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-15/named_collections.html");
const SCRIPT: &str = include_str!("../../../docs/diagnostics/2026-11-15/named_collections_page.js");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-15/named_collections_cft148.json");

/// Steps of the probe that answer as Chrome does.
const SAME: &[&str] = &[
    "forms.tag",
    "forms[id]",
    "forms[name]",
    "forms.id",
    "forms.name",
    "forms.missing",
    "forms.namedItem(id)",
    "forms.namedItem(name)",
    "forms.namedItem(missing)",
    "forms.namedItem(empty)",
    "forms[empty]",
    "forms.x",
    "forms.y",
    "forms.dup",
    "forms.namedItem(dup)",
    "forms.length",
    "forms.item",
    "forms.namedItem",
    "forms.toString",
    "forms.namedItem(length)",
    "forms.namedItem(item)",
    "forms.namedItem(toString)",
    "own(item)",
    "own(toString)",
    "forms.__proto__",
    "forms.constructor",
    "forms.hasOwnProperty",
    "forms.then",
    "own(then)",
    "own(__proto__)",
    "forms[\"0\"]",
    "forms[\"2\"]",
    "forms[\"40\"]",
    "forms[\"01\"]",
    "forms[\"-1\"]",
    "forms[\"1e3\"]",
    "forms.namedItem(\"40\")",
    "forms.item(\"fid\")",
    "in",
    "hasOwn",
    "keys",
    "forIn.named",
    "own(0)",
    "own(nope)",
    "iterate",
    "assign.sloppy",
    "assign.strict",
    "delete.sloppy",
    "delete.strict",
    "define",
    "live.fresh",
    "images",
    "links",
    "anchors",
    "scripts",
    "embeds",
    "plugins",
    "getElementsByName",
];

/// Steps that do not answer as Chrome does yet, and why.
const DIFFERENT: &[(&str, &str)] = &[
    ("forms.sameObject", "each read of document.forms makes a new snapshot; Chrome keeps one live object"),
    ("expando", "as forms.sameObject: a property a page puts on the collection is gone at the next read"),
    ("live.held", "a held collection is a snapshot: it does not follow the document"),
    ("plugins.sameAsEmbeds", "as forms.sameObject"),
    ("own(length)", "length is an own property of each collection here, an accessor of the interface in Chrome"),
    ("forIn", "as own(length): for-in lists no `length`"),
    ("ownNames", "Chrome also lists the names its interface shadows (length, item, toString), and ids and names in tree order"),
    ("own(fid)", "Chrome reports a named property as configurable and still refuses to delete or redefine it; here it is not configurable"),
    ("own(fname)", "as own(fid)"),
    ("getElementsByTagName", "its result has no named properties (namedItem() works)"),
    ("getElementsByClassName", "as getElementsByTagName"),
    ("children", "as getElementsByTagName"),
    ("form.elements", "an HTMLCollection here, not an HTMLFormControlsCollection, and a radio group is its first control, not a RadioNodeList"),
    ("document[name]", "no named access on the Document"),
];

fn object(json: &str) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::from_str(json).expect("json") {
        serde_json::Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

/// The probe's answers in the pinned Chrome.
fn chrome() -> serde_json::Map<String, serde_json::Value> {
    let pin = object(CHROME);
    assert_eq!(pin["browser"], "148.0.7778.216");
    pin["out"].as_object().expect("out").clone()
}

fn page() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => panic!("{other:?}"),
    }
}

/// The probe's answers here.
fn ours() -> serde_json::Map<String, serde_json::Value> {
    let b = page();
    b.evaluate(SCRIPT).unwrap();
    object(&ev(&b, "window.__out"))
}

#[test]
fn named_access_on_the_documents_collections_answers_as_chrome_does() {
    let (chrome, ours) = (chrome(), ours());
    let mut wrong = Vec::new();
    for step in SAME {
        let (want, got) = (&chrome[*step], ours.get(*step));
        if got != Some(want) {
            wrong.push(format!("{step}: chrome {want}, here {}", got.map_or("nothing".to_string(), |g| g.to_string())));
        }
    }
    assert!(wrong.is_empty(), "{} of {} steps differ from Chrome:\n{}", wrong.len(), SAME.len(), wrong.join("\n"));
}

#[test]
fn every_step_of_the_probe_is_accounted_for() {
    let (chrome, ours) = (chrome(), ours());
    for step in chrome.keys() {
        let same = SAME.contains(&step.as_str());
        let different = DIFFERENT.iter().any(|(name, _)| name == step);
        assert!(same != different, "{step} must be in exactly one of SAME and DIFFERENT");
    }
    assert_eq!(chrome.len(), SAME.len() + DIFFERENT.len(), "a listed step the probe does not have");
    // A step listed as different that now answers as Chrome does belongs in SAME.
    let fixed: Vec<&str> =
        DIFFERENT.iter().filter(|(step, _)| ours.get(*step) == Some(&chrome[*step])).map(|(step, _)| *step).collect();
    assert!(fixed.is_empty(), "these now answer as Chrome does; move them to SAME: {fixed:?}");
}

/// What MotionMark's start page does with its options form.
#[test]
fn a_form_is_read_from_document_forms_by_its_id_and_a_listener_is_added_to_it() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><body><form id=\"benchmark-options\"><input type=radio name=q value=a checked></form></body></html>")
            .unwrap(),
    ))
    .unwrap();
    let got = ev(
        &b,
        "var f = document.forms['benchmark-options'], seen = 0; \
         f.addEventListener('change', function () { seen++; }, true); \
         f.dispatchEvent(new Event('change')); \
         [Object.prototype.toString.call(f), f === document.querySelector('form'), seen].join()",
    );
    assert_eq!(got, "[object HTMLFormElement],true,1");
}

/// The named lookup is the engine's own: what a page puts in place of the built-ins it reads
/// the ids and names with changes nothing. (The wrappers and the index path under it are older
/// code and are not part of this check: `Object.defineProperty` and `Object.create` stay.)
#[test]
fn named_access_does_not_go_through_built_ins_a_page_replaced() {
    let b = page();
    let got = ev(
        &b,
        "var calls = 0; function spy() { calls++; throw new Error('replaced'); } \
         Object.getOwnPropertyDescriptor = spy; Object.prototype.hasOwnProperty = spy; \
         Reflect.apply = spy; Reflect.has = spy; Reflect.defineProperty = spy; \
         Array.prototype.slice = spy; Array.prototype.filter = spy; Array.prototype.concat = spy; \
         Array.prototype.push = spy; Array.prototype.indexOf = spy; \
         Function.prototype.call = spy; Function.prototype.apply = spy; \
         Element.prototype.getAttribute = spy; Element.prototype.hasAttribute = spy; \
         Document.prototype.getElementsByTagName = spy; String.prototype.charCodeAt = spy; \
         Object.defineProperty(Element.prototype, 'id', { get: spy, configurable: true }); \
         Object.defineProperty(Element.prototype, 'namespaceURI', { get: spy, configurable: true }); \
         var f = document.forms, i = document.images, l = document.links, a = document.anchors; \
         var ids = [f.fid, f.fname, f.dup, f[1], i.pic, l.arname, a.nohref].map(function (e) { return typeof e; }); \
         [ids.join(''), f.length, String(f.nope), f.fid === f[0], f.dup === f[1], i.pic === i[0], calls].join()",
    );
    assert_eq!(got, "objectobjectobjectobjectobjectobjectobject,21,undefined,true,true,true,0");
}
