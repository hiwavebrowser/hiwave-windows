//! Z plan C9 item (0): `getComputedStyle` reads Chromium's serialized value for
//! every property the engine stores. The expected values are Chromium
//! 143.0.7499.4's answers on this same page, written into
//! `tools/parity_oracle/computed_read_cases.json` by `computed_read_log.mjs`;
//! none is typed by hand. A case with a `gap` is one the engine cannot answer
//! yet, with the reason: it is required to stay wrong, so the list of gaps
//! cannot go stale.

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

const CASES: &str = include_str!("../../../tools/parity_oracle/computed_read_cases.json");

fn page_of(cases: &[serde_json::Value]) -> String {
    let text = |c: &serde_json::Value, k: &str| c[k].as_str().unwrap_or("").to_string();
    let decl = |c: &serde_json::Value| {
        let value = text(c, "value");
        let own = if value.is_empty() {
            String::new()
        } else {
            format!("{}:{value}", text(c, "prop"))
        };
        format!("{};{own}", text(c, "base"))
            .trim_start_matches(';')
            .to_string()
    };
    let inline = |c: &serde_json::Value| c["inline"].as_bool().unwrap_or(false);
    let mut rules = String::new();
    let mut divs = String::new();
    for (i, c) in cases.iter().enumerate() {
        if inline(c) {
            divs.push_str(&format!(
                "<div id=c{i} class=box style=\"{}\">x</div>",
                decl(c)
            ));
        } else {
            rules.push_str(&format!("#c{i}{{{}}}\n", decl(c)));
            divs.push_str(&format!("<div id=c{i} class=box>x</div>"));
        }
    }
    format!(
        "<!doctype html><style>body{{margin:0}}.box{{width:100px;height:50px}}\n{rules}</style><body>{divs}</body>"
    )
}

fn eval(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let value = engine.execute_script(id, script).expect("script");
    let inner = value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value);
    inner.replace("\\\"", "\"").replace("\\\\", "\\")
}

/// Every case of a family against Chromium's answer. Fails listing all of them.
fn family(name: &str) {
    let doc: serde_json::Value = serde_json::from_str(CASES).expect("cases");
    assert_eq!(doc["oracle"], "143.0.7499.4");
    let cases = doc["families"][name].as_array().expect("family").clone();
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 400, 300))
        .expect("view");
    engine.load_html(id, &page_of(&cases)).expect("load");
    let mut wrong = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let prop = c["prop"].as_str().unwrap();
        let chrome = c["chrome"].as_str().unwrap();
        let read = eval(
            &mut engine,
            id,
            &format!(
                "getComputedStyle(document.getElementById('c{i}')).getPropertyValue('{prop}')"
            ),
        );
        let gap = c.get("gap").is_some();
        if (read == chrome) == gap {
            let why = if gap {
                "gap now right, drop the mark"
            } else {
                "wrong"
            };
            wrong.push(format!(
                "{prop}: {:?} read {read:?}, Chromium {chrome:?} ({why})",
                c["value"]
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{name}: {} of {}\n{}",
        wrong.len(),
        cases.len(),
        wrong.join("\n")
    );
}

macro_rules! families {
    ($($name:ident),* $(,)?) => { $(#[test] fn $name() { family(stringify!($name)); })* };
}

families!(
    transform,
    clip,
    filter,
    backdrop_filter,
    box_shadow,
    text_decoration,
    text,
    background,
    object,
    transition,
    animation,
    container,
    sizing,
    box_shorthands,
    border,
    flex,
    grid,
    mask,
    font,
    inline,
);

/// A script's write to `style` is read back through the serializer, not as the
/// text it wrote. Expected: the `transform` and `inline` families' Chromium rows.
#[test]
fn a_style_write_reads_back_serialized() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 400, 300))
        .expect("view");
    engine
        .load_html(
            id,
            "<!doctype html><style>#a{width:100px;height:50px}</style><div id=a>x</div>",
        )
        .expect("load");
    let read = eval(
        &mut engine,
        id,
        "var a = document.getElementById('a'); a.style.transform = 'translate(10px,5px)'; getComputedStyle(a).transform",
    );
    assert_eq!(read, "matrix(1, 0, 0, 1, 10, 5)");
}

/// The #866 R1's 124 cases (its own pages, not this file's). Each says what it
/// must read: Chromium's answer, or what develop read before this change where
/// the engine still cannot answer (`base`), or `gap` (neither, listed in the PR).
#[test]
fn r1_cases() {
    const R1: &str = include_str!("../../../tools/parity_oracle/computed_read_r1_cases.json");
    let doc: serde_json::Value = serde_json::from_str(R1).expect("r1 cases");
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut wrong = Vec::new();
    let mut counts = std::collections::BTreeMap::new();
    for c in doc["cases"].as_array().expect("cases") {
        let s = |k: &str| c[k].as_str().unwrap_or("").to_string();
        let id = engine.create_headless_view(Bounds::new(0, 0, 400, 300)).expect("view");
        engine.load_html(id, &s("html")).expect("load");
        let read = eval(&mut engine, id, &s("expr"));
        let (chrome, base, expect) = (s("chrome"), s("base"), s("expect"));
        let ok = match expect.as_str() {
            "chrome" => read == chrome,
            "base" => read == base || read == chrome,
            "gap" => read != chrome,
            _ => true,
        };
        *counts.entry(if read == chrome { "chrome" } else if read == base { "base" } else { "other" }).or_insert(0) += 1;
        if std::env::var("R1_DUMP").is_ok() {
            eprintln!("R1\t{}\t{read}", s("name"));
        }
        if !ok {
            wrong.push(format!("{} ({expect}): read {read:?}, Chromium {chrome:?}, base {base:?}", s("name")));
        }
        let _ = engine.destroy_view(id);
    }
    eprintln!("r1 reads: {counts:?}");
    assert!(wrong.is_empty(), "{} wrong\n{}", wrong.len(), wrong.join("\n"));
}
