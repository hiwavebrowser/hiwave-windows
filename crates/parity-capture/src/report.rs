//! `--report` / `--report-text`: one page that says what is most likely
//! wrong with the captured page, facts below (dev-tools T4a).
//!
//! Built from the element diagnostics dump (`Engine::all_element_diagnostics`,
//! read-only over the final layout) and the record of the element hit at the
//! viewport's centre. Rules are deterministic and each cause names the facts
//! it read. Rules over the event log (scripts, errors, requests, load holds)
//! are not run here yet; the report says so instead of leaving them out
//! silently.

use serde::Serialize;
use serde_json::Value;

pub const REPORT_SCHEMA: &str = "hiwave-page-report/1";

/// A box at least this share of the viewport is a covering box.
const COVERING_SHARE: f64 = 0.30;
/// A covering box that takes the centre hit and at least this share is an
/// overlay (R2).
const OVERLAY_SHARE: f64 = 0.50;
/// Bands of one viewport height, from the top of the document.
const BANDS: usize = 3;
/// Selectors are page-chosen names: capped.
const MAX_SELECTOR_CHARS: usize = 64;
/// A placeholder is at least this thick and this long; anything smaller is a
/// rule or a dot.
const MIN_PLACEHOLDER_PX: (f64, f64) = (4.0, 16.0);
/// This many empty painted boxes in a band fire R11; fewer are decoration.
const MIN_EMPTY_PAINTED: u64 = 8;
/// This many overlapping boxes, or boxes past the right edge, in a band fire
/// R12; fewer are a stray badge.
const MIN_MISPLACED: u64 = 3;
/// A box lying over this share of the smaller of it and another overlaps it.
const OVERLAP_SHARE: f64 = 0.5;
/// A layout this close to the capture's end means the page was still moving.
const SETTLED_MS: f64 = 2000.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub schema: &'static str,
    pub url: Option<String>,
    pub viewport: (f64, f64),
    pub scroll: (f64, f64),
    pub document_height: f64,
    pub elements: u64,
    /// `overflow-y` and `position` of `html` and `body`.
    pub root_styles: Vec<RootStyle>,
    /// Positioned boxes covering at least 30% of the viewport, outermost
    /// only, largest first.
    pub covering: Vec<Covering>,
    /// Content per band of one viewport height, from the document's top.
    pub bands: Vec<Band>,
    /// The capture installed the shield (`--shield`).
    pub shield: bool,
    /// Was the page still moving when a `--live-ms` capture ended; `None`
    /// without a live window.
    pub settled: Option<Settled>,
    /// Most likely first.
    pub ranked: Vec<Cause>,
    /// Rules this build did not run, and why.
    pub not_checked: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RootStyle {
    pub element: String,
    pub overflow_y: Option<String>,
    pub position: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Covering {
    pub selector: String,
    pub position: String,
    pub z_index: Option<String>,
    /// Share of the viewport the box covers, in percent.
    pub covers_pct: f64,
    /// The element hit at the viewport's centre is this box or inside it.
    pub takes_centre: bool,
    #[serde(skip)]
    structural: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Band {
    pub top: f64,
    pub bottom: f64,
    /// Elements holding text directly (text length > 0, no element
    /// children) whose box starts in the band.
    pub text: u64,
    /// `img`, `svg`, `picture`, `video`, `canvas` boxes starting in the band.
    pub images: u64,
    /// In-flow boxes starting in the band that paint a background and hold
    /// nothing: no text, no element children, no `url()` image. Skeleton rows
    /// look like this; so does an empty ad slot.
    pub empty_painted: u64,
    /// In-flow text leaves and images starting in the band that lie over an
    /// earlier one, anywhere in the tree, by more than half of the smaller box.
    pub overlapping: u64,
    /// In-flow boxes starting in the band whose right edge passes the
    /// viewport, outermost only.
    pub past_right: u64,
}

/// What the capture knew when it ended.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CaptureFacts {
    pub shield: bool,
    /// `--live-ms`: milliseconds from the load's start to the window's end.
    pub capture_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settled {
    /// A layout within [`SETTLED_MS`] of the end, a request still open, or a
    /// navigation during the capture.
    pub changing: bool,
    /// `None`: no layout recorded, or the page navigated during the capture
    /// and its record times restarted.
    pub last_relayout_ms_before: Option<f64>,
    pub open_requests: u64,
    pub navigated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cause {
    /// `R1`..`R10` (see the T4 design note).
    pub rule: &'static str,
    pub line: String,
}

fn style<'a>(element: &'a Value, property: &str) -> Option<&'a str> {
    element["style"][property].as_str()
}

fn rect(element: &Value) -> Option<(f64, f64, f64, f64)> {
    let b = &element["box"]["border_box"];
    Some((b["x"].as_f64()?, b["y"].as_f64()?, b["w"].as_f64()?, b["h"].as_f64()?))
}

/// `tag#id.class.class`, capped. An element with neither is named by the
/// last two steps of its structural selector (`body > div:nth-child(41)`).
fn label(element: &Value) -> String {
    let identity = &element["identity"];
    let plain = identity["id"].as_str().is_none()
        && identity["classes"].as_array().is_none_or(|classes| classes.is_empty());
    if plain {
        let steps: Vec<&str> = structural(element).split(" > ").collect();
        if steps.len() >= 2 {
            return steps[steps.len() - 2..].join(" > ");
        }
    }
    let mut out = identity["tag"].as_str().unwrap_or("?").to_string();
    if let Some(id) = identity["id"].as_str() {
        out.push('#');
        out.push_str(id);
    }
    for class in identity["classes"].as_array().into_iter().flatten().take(2) {
        if let Some(class) = class.as_str() {
            out.push('.');
            out.push_str(class);
        }
    }
    if out.chars().count() > MAX_SELECTOR_CHARS {
        out = out.chars().take(MAX_SELECTOR_CHARS - 1).collect::<String>() + "…";
    }
    out
}

fn structural(element: &Value) -> &str {
    element["identity"]["selector"].as_str().unwrap_or("")
}

fn is_within(inner: &str, outer: &str) -> bool {
    inner == outer || inner.starts_with(&format!("{outer} > "))
}

/// `html` and `body`: a page that locks scrolling often makes the body
/// `position: fixed`. That is the lock (R1), not an overlay, and what is in
/// the body is still the document's content.
fn is_root(element: &Value) -> bool {
    matches!(element["identity"]["tag"].as_str(), Some("html" | "body"))
}

fn visible(element: &Value) -> bool {
    element["box"]["has_box"].as_bool() == Some(true)
        && style(element, "visibility") != Some("hidden")
        && style(element, "opacity").and_then(|o| o.parse::<f64>().ok()) != Some(0.0)
}

/// An in-flow box with a background and nothing in it. A positioned one is
/// an overlay: yahoo stacks 12 gradient scrims over its first viewport's
/// carousel. Looks at the computed style only: a white box on a white page
/// counts, and a `url()` background counts as an image even if it never loaded.
fn empty_painted(element: &Value, width: f64, height: f64) -> bool {
    let tag = element["identity"]["tag"].as_str().unwrap_or("");
    if matches!(tag, "input" | "button" | "select" | "textarea" | "hr" | "iframe" | "embed" | "object" | "meter" | "progress")
        || matches!(style(element, "position"), Some("absolute" | "fixed"))
        || element["tree"]["text_length"].as_u64() != Some(0)
        || element["tree"]["child_count"].as_u64() != Some(0)
        || width.min(height) < MIN_PLACEHOLDER_PX.0
        || width.max(height) < MIN_PLACEHOLDER_PX.1
    {
        return false;
    }
    !style(element, "background-image").unwrap_or("none").contains("url(") && has_background(element)
}

fn has_background(element: &Value) -> bool {
    let colour = style(element, "background-color").is_some_and(|c| !(c.starts_with("rgba(") && c.ends_with(", 0)")));
    colour || style(element, "background-image").unwrap_or("none") != "none"
}

fn is_image(element: &Value) -> bool {
    matches!(element["identity"]["tag"].as_str(), Some("img" | "svg" | "picture" | "video" | "canvas"))
}

/// Does this box's layout rect say where it paints something, in the flow?
/// Not a box out of flow, sticky or inline (a wrapped link's rect is the union
/// of its lines), clipped by an ancestor, or moved or faded by one.
fn paints_in_flow(element: &Value) -> bool {
    visible(element)
        && !is_root(element)
        && !matches!(style(element, "position"), Some("absolute" | "fixed" | "sticky"))
        && !matches!(style(element, "display"), Some("inline" | "contents"))
        && element["paint"]["clipped"].as_bool() != Some(true)
        && element["paint"]["in_transform_layer"].as_bool() != Some(true)
        && element["paint"]["effective_opacity"].as_f64() != Some(0.0)
        && (element["tree"]["text_length"].as_u64().unwrap_or(0) > 0 || is_image(element) || has_background(element))
}

fn parent_of(path: &str) -> Option<&str> {
    path.rsplit_once(" > ").map(|(parent, _)| parent)
}

/// Fill each band's `overlapping` and `past_right` from the layout rects.
/// Overlap is between content leaves anywhere in the tree, not siblings: on
/// yahoo no two siblings overlap, its columns' content runs out of their boxes
/// and over each other. A sibling test read 0 there and fired on wikipedia. The
/// later leaf in document order is the one counted: it is the one on top.
fn count_misplaced(elements: &[&Value], viewport_width: f64, scroll_y: f64, bands: &mut [Band]) {
    let out_of_flow: Vec<&str> = elements
        .iter()
        .filter(|e| !is_root(e) && matches!(style(e, "position"), Some("absolute" | "fixed")))
        .map(|e| structural(e))
        .collect();
    let past_right = |e: &Value| rect(e).is_some_and(|(x, _, w, _)| x + w > viewport_width + 1.0);
    let by_path: std::collections::HashMap<&str, &Value> = elements.iter().map(|e| (structural(e), *e)).collect();
    let mut earlier: Vec<(&str, (f64, f64, f64, f64))> = Vec::new();
    for element in elements {
        let path = structural(element);
        if !paints_in_flow(element) || out_of_flow.iter().any(|o| is_within(path, o)) {
            continue;
        }
        let Some(me) = rect(element).filter(|r| r.2 > 0.0 && r.3 > 0.0) else { continue };
        let leaf = is_image(element)
            || (element["tree"]["text_length"].as_u64().unwrap_or(0) > 0 && element["tree"]["child_count"].as_u64() == Some(0));
        let over = leaf
            && earlier.iter().any(|(other, o)| {
                let w = (me.0 + me.2).min(o.0 + o.2) - me.0.max(o.0);
                let h = (me.1 + me.3).min(o.1 + o.3) - me.1.max(o.1);
                w > 0.0
                    && h > 0.0
                    && w * h > OVERLAP_SHARE * (me.2 * me.3).min(o.2 * o.3)
                    && !is_within(path, other)
                    && !is_within(other, path)
            });
        if leaf {
            earlier.push((path, me));
        }
        let parent = parent_of(path).unwrap_or("");
        let top = me.1 + scroll_y;
        let Some(band) = bands.iter_mut().find(|b| top >= b.top && top < b.bottom) else { continue };
        band.overlapping += u64::from(over);
        // A wide box's children are the same fact. A wide body is no box of
        // its own, so what is in it still counts.
        let outermost = by_path.get(parent).is_none_or(|p| is_root(p) || !past_right(p));
        band.past_right += u64::from(past_right(element) && outermost);
    }
}

/// The report for a diagnostics dump (as `all_element_diagnostics`
/// serialises it) and the record of the element at the viewport's centre.
pub fn page_report(dump: &Value, centre: Option<&Value>) -> Report {
    let page = &dump["page"];
    let viewport = (
        page["viewport"][0].as_f64().unwrap_or(0.0),
        page["viewport"][1].as_f64().unwrap_or(0.0),
    );
    let scroll = (page["scroll"][0].as_f64().unwrap_or(0.0), page["scroll"][1].as_f64().unwrap_or(0.0));
    let elements: Vec<&Value> = dump["elements"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let centre_selector = centre.map(structural).unwrap_or("");

    let mut document_height = viewport.1;
    let mut root_styles = Vec::new();
    for element in &elements {
        let tag = element["identity"]["tag"].as_str().unwrap_or("");
        if tag == "html" || tag == "body" {
            root_styles.push(RootStyle {
                element: tag.to_string(),
                overflow_y: style(element, "overflow-y").map(str::to_string),
                position: style(element, "position").map(str::to_string),
            });
        }
        if style(element, "position") == Some("fixed") && !is_root(element) {
            continue;
        }
        if let Some((_, y, _, h)) = rect(element) {
            document_height = document_height.max(y + scroll.1 + h);
        }
    }

    let viewport_area = (viewport.0 * viewport.1).max(1.0);
    let share_of = |e: &Value| {
        let (x, y, w, h) = rect(e)?;
        let cw = (x + w).min(viewport.0) - x.max(0.0);
        let ch = (y + h).min(viewport.1) - y.max(0.0);
        Some((cw.max(0.0) * ch.max(0.0)) / viewport_area)
    };
    let positioned: Vec<(&Value, f64)> = elements
        .iter()
        .copied()
        .filter(|e| !is_root(e) && visible(e) && matches!(style(e, "position"), Some("fixed" | "absolute" | "sticky")))
        .filter_map(|e| Some((e, share_of(e)?)))
        .collect();
    // The largest fixed box on screen, whatever its share: what R1 names
    // when nothing covers the viewport (a consent bar).
    let largest_fixed = positioned
        .iter()
        .filter(|(e, share)| style(e, "position") == Some("fixed") && *share > 0.0)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(e, share)| Covering {
            selector: label(e),
            position: "fixed".to_string(),
            z_index: style(e, "z-index").filter(|z| *z != "auto").map(str::to_string),
            covers_pct: (share * 1000.0).round() / 10.0,
            takes_centre: false,
            structural: structural(e).to_string(),
        });
    let mut covering: Vec<Covering> = Vec::new();
    let mut candidates: Vec<(&Value, f64)> =
        positioned.iter().copied().filter(|(_, share)| *share >= COVERING_SHARE).collect();
    // Tree order keeps an outer box before the boxes inside it.
    candidates.sort_by(|a, b| structural(a.0).len().cmp(&structural(b.0).len()));
    for (element, share) in candidates {
        let path = structural(element);
        if covering.iter().any(|c| is_within(path, &c.structural)) {
            continue;
        }
        covering.push(Covering {
            selector: label(element),
            position: style(element, "position").unwrap_or("").to_string(),
            z_index: style(element, "z-index").filter(|z| *z != "auto").map(str::to_string),
            covers_pct: (share * 1000.0).round() / 10.0,
            takes_centre: !centre_selector.is_empty() && is_within(centre_selector, path),
            structural: path.to_string(),
        });
    }
    covering.sort_by(|a, b| b.covers_pct.partial_cmp(&a.covers_pct).unwrap_or(std::cmp::Ordering::Equal));

    let mut bands: Vec<Band> = (0..BANDS)
        .map(|i| Band { top: i as f64 * viewport.1, bottom: (i + 1) as f64 * viewport.1, text: 0, images: 0, empty_painted: 0, overlapping: 0, past_right: 0 })
        .collect();
    // What is inside a fixed box is not the document's content.
    let fixed: Vec<&str> = elements
        .iter()
        .filter(|e| !is_root(e) && style(e, "position") == Some("fixed"))
        .map(|e| structural(e))
        .collect();
    for element in &elements {
        if !visible(element) || fixed.iter().any(|f| is_within(structural(element), f)) {
            continue;
        }
        let Some((_, y, w, h)) = rect(element) else { continue };
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let top = y + scroll.1;
        let Some(band) = bands.iter_mut().find(|b| top >= b.top && top < b.bottom) else { continue };
        if is_image(element) {
            band.images += 1;
        } else if element["tree"]["text_length"].as_u64().unwrap_or(0) > 0
            && element["tree"]["child_count"].as_u64() == Some(0)
        {
            band.text += 1;
        } else if empty_painted(element, w, h) {
            band.empty_painted += 1;
        }
    }
    count_misplaced(&elements, viewport.0, scroll.1, &mut bands);

    let mut ranked = Vec::new();
    let overlay = covering.iter().find(|c| c.takes_centre).or_else(|| covering.first()).or(largest_fixed.as_ref());
    let describe = |c: &Covering| {
        format!(
            "{} {}{} covers {}% of the viewport",
            c.position,
            c.selector,
            c.z_index.as_deref().map(|z| format!(" (z {z})")).unwrap_or_default(),
            c.covers_pct
        )
    };
    // R1: the document is taller than the viewport and cannot scroll.
    let lock = root_styles.iter().find(|r| {
        r.overflow_y.as_deref() == Some("hidden") || r.position.as_deref() == Some("fixed")
    });
    let mut named_overlay = None;
    if let (Some(lock), true) = (lock, document_height > viewport.1 + 1.0) {
        let why = if lock.overflow_y.as_deref() == Some("hidden") {
            format!("{} overflow-y:hidden", lock.element)
        } else {
            format!("{} position:fixed", lock.element)
        };
        let line = match overlay {
            Some(c) => {
                named_overlay = Some(c.selector.clone());
                format!("scroll locked ({why}) by overlay: {}", describe(c))
            }
            None => format!("scroll locked ({why}); document is {document_height:.0}px tall"),
        };
        ranked.push(Cause { rule: "R1", line });
    }
    // R2: a box over the centre of the viewport.
    for c in covering.iter().filter(|c| c.takes_centre && c.covers_pct >= OVERLAY_SHARE * 100.0) {
        if named_overlay.as_deref() != Some(c.selector.as_str()) {
            ranked.push(Cause { rule: "R2", line: format!("overlay: {}", describe(c)) });
        }
    }
    // R9: content at the top, an empty band below it inside the document.
    let top_has_content = bands.first().is_some_and(|b| b.text + b.images > 0);
    if top_has_content {
        if let Some(empty) = bands
            .iter()
            .skip(1)
            .find(|b| b.bottom <= document_height && b.text == 0 && b.images == 0)
        {
            ranked.push(Cause {
                rule: "R9",
                line: format!(
                    "below the fold is empty from y={:.0} (document {document_height:.0}px tall): no text or image starts in {:.0}..{:.0}",
                    empty.top, empty.top, empty.bottom
                ),
            });
        }
    } else if bands.first().is_some() {
        // R10: nothing to read at all. yahoo with #777 merged ended as 77
        // elements and no text after React's error boundary took the page.
        ranked.push(Cause {
            rule: "R10",
            line: format!(
                "the first viewport has no text and no image ({} elements, document {document_height:.0}px tall)",
                dump["total_elements"].as_u64().unwrap_or(elements.len() as u64)
            ),
        });
    }
    // R11: placeholders count as neither text nor image, so a feed of grey
    // rows read as a band with some content in it (yahoo, T9).
    for band in bands.iter().filter(|b| b.empty_painted >= MIN_EMPTY_PAINTED) {
        ranked.push(Cause {
            rule: "R11",
            line: format!(
                "y {:.0}..{:.0}: {} painted boxes with no text and no image (skeleton?)",
                band.top, band.bottom, band.empty_painted
            ),
        });
    }
    // R12: yahoo's footer and labels painted over its feed, and its rows ran
    // past the right edge (T9).
    for band in bands.iter().filter(|b| b.overlapping >= MIN_MISPLACED || b.past_right >= MIN_MISPLACED) {
        ranked.push(Cause {
            rule: "R12",
            line: format!(
                "y {:.0}..{:.0}: {} boxes overlap other content, {} run past the right edge",
                band.top, band.bottom, band.overlapping, band.past_right
            ),
        });
    }

    Report {
        schema: REPORT_SCHEMA,
        url: page["url"].as_str().map(str::to_string),
        viewport,
        scroll,
        document_height,
        elements: dump["total_elements"].as_u64().unwrap_or(elements.len() as u64),
        root_styles,
        covering,
        bands,
        shield: false,
        settled: None,
        ranked,
        not_checked: vec![
            "R3-R8 (scripts that never finished or starved, uncaught errors, blocked or failed requests, \
             late load, silent gaps): need the event-log rules (T4b)"
                .to_string(),
        ],
    }
}

/// An `inserted` or `async` script that waited at least this long between
/// its body arriving and running was starved of turns (R4).
const STARVED_MS: f64 = 3000.0;
/// `load` this long after `DOMContentLoaded` is late (R7).
const LATE_LOAD_MS: f64 = 5000.0;
/// No record for this long before the load finished is a silent gap (R8).
const SILENT_MS: f64 = 3000.0;
/// At most this many lines per rule.
const PER_RULE: usize = 3;

/// R5 lines about the page's own site start with this, and rank before R4.
const OWN_SITE: &str = "the page's own script: ";
/// An R5 key for a `console.error` of a string, up to its place.
const LOGGED_STRING: &str = "logged string in ";

/// The last two host labels (`www.yahoo.com` -> `yahoo.com`). Close enough
/// for ranking; a two-label public suffix (`co.uk`) reads as one site. An
/// IPv4 address is its own site, whole (an IPv6 one is not split out by
/// `host_of`). No host (`file://`) is no site, so a `file://` page ranks no
/// script error as its own.
fn site_of(url: &str) -> Option<String> {
    let host = host_of(url).filter(|h| !h.is_empty())?;
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return Some(host);
    }
    let labels: Vec<&str> = host.split('.').collect();
    Some(labels[labels.len().saturating_sub(2)..].join("."))
}

fn host_of(url: &str) -> Option<String> {
    Some(url.split("://").nth(1)?.split(['/', ':', '?', '#']).next()?.to_string())
}

/// The site of the URL in an R5 key (`uncaught X in <url>:line:col`,
/// `logged X thrown in <url>:line:col`).
fn site_of_error(key: &str) -> Option<String> {
    site_of(key.split(" in ").nth(1)?)
}

/// The error name in a console message: the shape `<TypeError message len=46>`
/// or the kept text `TypeError: ...`. The error need not be the first
/// argument: `console.error("text", err)` is `<string len=4> <TypeError ...>`.
fn error_name(message: &str) -> &str {
    let shaped = message.find(" message len=").and_then(|end| message[..end].rfind('<').map(|start| &message[start..]));
    let name = shaped.unwrap_or(message).trim_start_matches('<');
    let end = name.find([' ', ':', '>']).unwrap_or(name.len());
    if end == 0 { "error" } else { &name[..end] }
}

fn ms(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("-")
}

/// The event-log rules (R3-R8) over the load's records, as the JSON Lines
/// sink writes them. Fields are read by name, so a build without T2's or
/// T3's records skips those rules and says so (the second list).
pub fn event_causes(records: &[Value]) -> (Vec<Cause>, Vec<String>) {
    let of = |kind: &'static str| records.iter().filter(move |r| r["type"] == kind);
    let mut causes = Vec::new();
    let mut not_checked = Vec::new();
    let scripts: Vec<&Value> = of("script").collect();
    let has_lifecycle = of("lifecycle").next().is_some();

    // R4: scripts starved of turns, and script requests never answered.
    if scripts.is_empty() {
        not_checked.push("R3, R4: no `script` records on this build (dev-tools T3)".to_string());
    }
    // A record is written when its script is over, so it ran over
    // [t - ran, t] and waited over [t - ran - waited, t - ran]. A wait spent
    // while other scripts ran is a queue, not starvation: only the idle part
    // of the wait counts.
    let run_span = |s: &Value| {
        let end = ms(&s["t_ms"]).unwrap_or(0.0);
        (end - ms(&s["ran_ms"]).unwrap_or(0.0), end)
    };
    let busy_during = |me: &Value, from: f64, to: f64| -> f64 {
        scripts
            .iter()
            .filter(|o| !std::ptr::eq(**o, me))
            .map(|o| {
                let (a, b) = run_span(o);
                (b.min(to) - a.max(from)).max(0.0)
            })
            .sum::<f64>()
            .min(to - from)
    };
    // Before the first page script runs, every script waits on the load
    // itself, not for a turn (yahoo: bodies by 1.7 s, first script at 5.8 s,
    // after first layout and the last image). That is one fact, said once.
    let first_start = scripts.iter().map(|s| run_span(s).0).fold(f64::INFINITY, f64::min);
    let early: Vec<f64> = scripts
        .iter()
        .filter_map(|s| Some(run_span(s).0 - ms(&s["waited_ms"])?))
        .filter(|arrived| first_start - arrived >= STARVED_MS)
        .collect();
    if let Some(first_arrival) = early.iter().copied().reduce(f64::min) {
        let kind_of: std::collections::HashMap<u64, &str> = of("request")
            .filter_map(|r| Some((r["id"].as_u64()?, r["resource_type"].as_str()?)))
            .collect();
        let last_answer = records
            .iter()
            .filter(|r| r["type"] == "response" || r["type"] == "failure")
            .filter(|r| ms(&r["t_ms"]).is_some_and(|t| t <= first_start))
            .last();
        let mut line = format!(
            "no page script ran until t={:.1} s: {} script body(ies) had arrived {:.1} s or more before (first at t={:.1} s)",
            first_start / 1000.0,
            early.len(),
            STARVED_MS / 1000.0,
            first_arrival / 1000.0
        );
        if let Some(t) = of("first_layout").next().and_then(|r| ms(&r["t_ms"])) {
            line.push_str(&format!("; first layout at t={:.1} s", t / 1000.0));
        }
        if let Some(r) = last_answer {
            let kind = r["id"].as_u64().and_then(|id| kind_of.get(&id)).copied().unwrap_or("request");
            line.push_str(&format!("; last answer before it: {kind} at t={:.1} s", ms(&r["t_ms"]).unwrap_or(0.0) / 1000.0));
        }
        causes.push(Cause { rule: "R4", line });
    }
    let mut starved: Vec<(&Value, f64, f64)> = scripts
        .iter()
        .filter(|s| matches!(s["kind"].as_str(), Some("inserted" | "async")))
        .filter_map(|s| {
            let (start, _) = run_span(s);
            // Only the wait after the first script ran can be a missed turn.
            let from = (start - ms(&s["waited_ms"])?).max(first_start);
            let waited = (start - from).max(0.0);
            let busy = busy_during(s, from, start);
            (waited - busy >= STARVED_MS).then_some((*s, waited, busy))
        })
        .collect();
    starved.sort_by(|a, b| (b.1 - b.2).partial_cmp(&(a.1 - a.2)).unwrap_or(std::cmp::Ordering::Equal));
    for (s, waited, busy) in starved.iter().take(PER_RULE) {
        let queued = if *busy >= 100.0 {
            format!(" ({:.1} s of it behind other scripts running)", busy / 1000.0)
        } else {
            String::new()
        };
        causes.push(Cause {
            rule: "R4",
            line: format!(
                "{} script {} arrived in {:.0} ms and waited {:.1} s for a turn{queued}",
                text(&s["kind"]),
                text(&s["script"]),
                ms(&s["fetch_ms"]).unwrap_or(0.0),
                waited / 1000.0
            ),
        });
    }
    let answered: std::collections::HashSet<u64> = records
        .iter()
        .filter(|r| r["type"] == "response" || r["type"] == "failure")
        .filter_map(|r| r["id"].as_u64())
        .collect();
    let open_scripts: Vec<&Value> = of("request")
        .filter(|r| matches!(r["resource_type"].as_str(), Some("script" | "module")))
        .filter(|r| r["id"].as_u64().is_some_and(|id| !answered.contains(&id)))
        .collect();
    if let Some(first) = open_scripts.first() {
        causes.push(Cause {
            rule: "R4",
            line: format!(
                "{} script request(s) still open when the capture ended, first {} at t={:.1} s",
                open_scripts.len(),
                text(&first["url"]),
                ms(&first["t_ms"]).unwrap_or(0.0) / 1000.0
            ),
        });
    }

    // R3: scripts that never finished.
    for s in scripts
        .iter()
        .filter(|s| matches!(s["outcome"].as_str(), Some("skipped" | "over_budget" | "stopped" | "fetch_failed")))
        .filter(|s| s["reason"] != "nomodule")
        .take(PER_RULE)
    {
        causes.push(Cause {
            rule: "R3",
            line: format!(
                "{} script {} {} ({})",
                text(&s["kind"]),
                text(&s["script"]),
                text(&s["outcome"]),
                text(&s["reason"])
            ),
        });
    }

    // R6: requests the shield blocked, then other failures; scripts and
    // stylesheets first.
    let urls: std::collections::HashMap<u64, (&str, &str)> = of("request")
        .filter_map(|r| Some((r["id"].as_u64()?, (text(&r["url"]), text(&r["resource_type"])))))
        .collect();
    let mut failures: Vec<&Value> = of("failure").collect();
    let weight = |f: &Value| {
        let blocked = !f["blocked_by"].is_null();
        let kind = f["id"].as_u64().and_then(|id| urls.get(&id)).map(|u| u.1).unwrap_or("");
        (!blocked, !matches!(kind, "script" | "module" | "stylesheet"))
    };
    failures.sort_by_key(|f| weight(f));
    for f in failures.iter().filter(|f| f["stage"] != "cancelled").take(PER_RULE) {
        let (url, kind) = f["id"].as_u64().and_then(|id| urls.get(&id)).copied().unwrap_or(("-", "-"));
        let line = match f["blocked_by"].as_object() {
            Some(b) => format!(
                "shield blocked {kind} {url} ({} {})",
                b.get("list").and_then(Value::as_str).unwrap_or("-"),
                b.get("rule").and_then(Value::as_str).unwrap_or("-")
            ),
            None => format!("{kind} {url} failed: {} ({})", text(&f["stage"]), text(&f["reason"])),
        };
        causes.push(Cause { rule: "R6", line });
    }

    // R5: uncaught errors, grouped by name and place.
    let mut errors: Vec<(String, usize, f64)> = Vec::new();
    for e in of("script_error") {
        let at = &e["location"];
        let key = format!(
            "uncaught {}{} in {}",
            if e["name"].as_str().unwrap_or("").is_empty() { text(&e["value"]) } else { text(&e["name"]) },
            if e["unhandled_rejection"] == true { " (unhandled rejection)" } else { "" },
            if at.is_null() {
                text(&e["script"]).to_string()
            } else {
                format!("{}:{}:{}", text(&at["url"]), at["line"], at["column"])
            }
        );
        match errors.iter_mut().find(|(k, _, _)| *k == key) {
            Some((_, n, _)) => *n += 1,
            None => errors.push((key, 1, ms(&e["t_ms"]).unwrap_or(0.0))),
        }
    }
    // An error the page caught and passed to console.error carries its own
    // stack (`error_stack`); it is named by where it was thrown. yahoo with
    // #777 merged: two such TypeErrors, no uncaught one, and a blank page.
    let page_url = of("navigation_start").next().and_then(|r| r["url"].as_str());
    let site = page_url.and_then(site_of);
    let page_host = page_url.and_then(host_of);
    // A string has no stack of its own, so it is named by the first of the
    // page's own frames that logged it (T9 gap 7): the call's place, then its
    // callers, since yahoo wraps `console.error` from another host. Other
    // sites' strings are ads' noise. The shape says it was a string: a run
    // that kept console text can't tell, and is left out.
    let frame_place = |frame: &str| -> String {
        let place = frame.trim().trim_start_matches("at ");
        place.rsplit_once(" (").map_or(place, |(_, p)| p.trim_end_matches(')')).to_string()
    };
    let own_place = |place: &str| site.is_some() && (place.starts_with("inline#") || site_of(place) == site);
    for c in of("console").filter(|c| c["level"] == "error") {
        let key = if let Some(thrown) = c["error_stack"].as_str().and_then(|s| s.lines().next()) {
            format!("logged {} thrown in {}", error_name(text(&c["message"])), frame_place(thrown))
        } else if text(&c["message"]).contains("<string len=") {
            let at = &c["location"];
            let located = (!at.is_null()).then(|| format!("{}:{}:{}", text(&at["url"]), at["line"], at["column"]));
            let callers = c["stack"].as_str().unwrap_or("").lines().map(frame_place);
            let Some(place) = located.into_iter().chain(callers).find(|p| own_place(p)) else { continue };
            format!("{LOGGED_STRING}{place}")
        } else {
            continue;
        };
        match errors.iter_mut().find(|(k, _, _)| *k == key) {
            Some((_, n, _)) => *n += 1,
            None => errors.push((key, 1, ms(&c["t_ms"]).unwrap_or(0.0))),
        }
    }
    // An error in the page's own site's script ranks first of the event
    // causes (design note R5): yahoo's chunk 7730 SyntaxError is the app
    // bundle failing, where an ad script's error is noise. Its line says when
    // it fell and how many fetch/XHR went to the page's own host either side:
    // an error late in a fixed window has few calls after it whatever it
    // broke (yahoo, T9: 2, 1 and 0 after, 8 to 10 before). The host, not the
    // site: yahoo's ad and analytics hosts share the site.
    let own_calls = |keep: &dyn Fn(f64) -> bool| {
        of("request")
            .filter(|r| matches!(r["resource_type"].as_str(), Some("fetch" | "xhr")))
            .filter(|r| ms(&r["t_ms"]).is_some_and(keep))
            .filter(|r| r["url"].as_str().and_then(host_of) == page_host)
            .count()
    };
    // A logged string in the page's inline script is the page's own, and an
    // own-site string never pushes an error off the list.
    let is_string = |key: &str| key.starts_with(LOGGED_STRING);
    let is_own = |key: &str| {
        site.is_some() && (site_of_error(key) == site || key.starts_with(&format!("{LOGGED_STRING}inline#")))
    };
    errors.sort_by_key(|(key, _, _)| (!is_own(key), is_string(key)));
    for (key, n, first) in errors.into_iter().take(PER_RULE) {
        let mut line = if n > 1 { format!("{key} (x{n})") } else { key };
        if is_own(&line) {
            let before = own_calls(&|at| at < first);
            let after = own_calls(&|at| at >= first);
            let host = page_host.as_deref().unwrap_or("-");
            line = format!("{OWN_SITE}{line}; at t={:.1} s; {before} fetch/XHR to {host} before, {after} after", first / 1000.0);
        }
        causes.push(Cause { rule: "R5", line });
    }

    // R7: load long after DOMContentLoaded, and what it waited for.
    if !has_lifecycle {
        not_checked.push("R7: no `lifecycle` records on this build (dev-tools T3)".to_string());
    }
    let at = |event: &str| of("lifecycle").find(|r| r["event"] == event).and_then(|r| ms(&r["t_ms"]));
    if let (Some(dcl), Some(load)) = (at("DOMContentLoaded"), at("load")) {
        if load - dcl >= LATE_LOAD_MS {
            let held = of("lifecycle")
                .find(|r| r["event"] == "load")
                .map(|r| {
                    r["held_by"]
                        .as_array()
                        .map(|a| a.iter().take(PER_RULE).map(|s| text(s).to_string()).collect::<Vec<_>>().join(", "))
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            causes.push(Cause {
                rule: "R7",
                line: format!(
                    "load fired {:.1} s after DOMContentLoaded; it waited for: {}",
                    (load - dcl) / 1000.0,
                    if held.is_empty() { "no script (see R8)".to_string() } else { held }
                ),
            });
        }
    }

    // R8: the log silent for a while before the load finished, or while a
    // script body was in and waiting to run (yahoo: nine inserted chunks in
    // at 11.3 s, nothing recorded until 15.4 s, well after load).
    let finished = of("load_finished").next().and_then(|r| ms(&r["t_ms"]));
    // A script running is not silence: its start is a point too, and its run
    // span is taken out of any gap it falls in.
    let spans: Vec<(f64, f64)> = scripts.iter().map(|s| run_span(s)).collect();
    let waits: Vec<(f64, f64)> = scripts
        .iter()
        .filter_map(|s| {
            let start = run_span(s).0;
            Some((start - ms(&s["waited_ms"])?, start))
        })
        .collect();
    let mut times: Vec<f64> = records.iter().filter_map(|r| ms(&r["t_ms"])).chain(spans.iter().map(|s| s.0)).collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut gaps: Vec<(f64, f64, usize)> = times
        .windows(2)
        .filter_map(|w| {
            let waiting = waits.iter().filter(|(a, b)| *a <= w[0] && *b >= w[1]).count();
            let pending = finished.is_none_or(|end| w[1] <= end);
            if !pending && waiting == 0 {
                return None;
            }
            let ran: f64 = spans.iter().map(|(a, b)| (b.min(w[1]) - a.max(w[0])).max(0.0)).sum();
            Some((w[0], (w[1] - w[0] - ran).max(0.0), if pending { 0 } else { waiting }))
        })
        .filter(|(_, gap, _)| *gap >= SILENT_MS)
        .collect();
    gaps.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (from, gap, waiting) in gaps.into_iter().take(PER_RULE) {
        let when = if waiting == 0 {
            "before the load finished".to_string()
        } else {
            format!("with {waiting} script(s) in and waiting to run")
        };
        causes.push(Cause {
            rule: "R8",
            line: format!(
                "event log silent {:.1} s from t={:.1} s {when}: no script ran, no request moved",
                gap / 1000.0,
                from / 1000.0
            ),
        });
    }
    (causes, not_checked)
}

/// Was the page moving when the window ended at `end` ms? Records after `end`
/// are the capture's teardown (it cancels what is open), not the page's.
fn settled(records: &[Value], end: f64) -> Settled {
    let of = |kinds: &'static [&'static str]| {
        records.iter().filter(move |r| kinds.contains(&r["type"].as_str().unwrap_or("")))
    };
    let in_window = |r: &&Value| ms(&r["t_ms"]).is_some_and(|t| t <= end);
    // Record times restart at a navigation, so they can't be read against `end`.
    let navigated = of(&["navigation_start"]).count() > 1;
    let answered: std::collections::HashSet<u64> =
        of(&["response", "failure"]).filter(in_window).filter_map(|r| r["id"].as_u64()).collect();
    let open_requests = of(&["request"])
        .filter(in_window)
        .filter(|r| r["id"].as_u64().is_some_and(|id| !answered.contains(&id)))
        .count() as u64;
    let last_relayout_ms_before = of(&["first_layout", "relayout"])
        .filter(in_window)
        .filter_map(|r| ms(&r["t_ms"]))
        .reduce(f64::max)
        .filter(|_| !navigated)
        .map(|t| end - t);
    Settled {
        changing: navigated || open_requests > 0 || last_relayout_ms_before.is_some_and(|ago| ago < SETTLED_MS),
        last_relayout_ms_before,
        open_requests,
        navigated,
    }
}

/// Add the event-log rules to a layout report: layout causes keep the top
/// (R1, R2), then an empty band (R9) or placeholders (R11), then the event
/// causes in the design note's order.
pub fn apply_events(report: &mut Report, records: &[Value], facts: CaptureFacts) {
    report.shield = facts.shield;
    report.settled = facts.capture_ms.map(|end| settled(records, end));
    if !facts.shield {
        report.not_checked.push("R6 shield: not installed, nothing could be blocked".to_string());
    }
    let (events, not_checked) = event_causes(records);
    // "R5*" is an R5 line about the page's own site. "R5!" is an uncaught
    // one when the first viewport is blank (R10): the cause goes above the
    // symptom (Atlas 10-26; yahoo's chunk 7730 SyntaxError under R10).
    let order = ["R5!", "R1", "R2", "R10", "R9", "R11", "R12", "R5*", "R4", "R3", "R6", "R5", "R7", "R8"];
    let blank = report.ranked.iter().any(|c| c.rule == "R10");
    let uncaught = format!("{OWN_SITE}uncaught ");
    report.ranked.extend(events);
    report.ranked.sort_by_key(|c| {
        let rule = match c.rule {
            "R5" if blank && c.line.starts_with(&uncaught) => "R5!",
            "R5" if c.line.starts_with(OWN_SITE) => "R5*",
            rule => rule,
        };
        order.iter().position(|r| *r == rule).unwrap_or(order.len())
    });
    report.not_checked.retain(|n| !n.starts_with("R3-R8"));
    report.not_checked.extend(not_checked);
}

/// The report as one page of text.
pub fn report_text(report: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "diagnose {}  viewport {:.0}x{:.0}  document {:.0}px tall  {} elements\n\n",
        report.url.as_deref().unwrap_or("-"),
        report.viewport.0,
        report.viewport.1,
        report.document_height,
        report.elements
    ));
    out.push_str("Most likely:\n");
    if report.ranked.is_empty() {
        out.push_str("  (no rule fired)\n");
    }
    for (i, cause) in report.ranked.iter().enumerate() {
        out.push_str(&format!("  {}. [{}] {}\n", i + 1, cause.rule, cause.line));
    }
    out.push_str("\nFacts:\n");
    if let Some(s) = &report.settled {
        let state = if s.changing { "still changing" } else { "settled" };
        let layout = match s.last_relayout_ms_before {
            Some(ago) => format!("last relayout {ago:.0} ms before"),
            None if s.navigated => "the page navigated during the capture".to_string(),
            None => "no layout recorded".to_string(),
        };
        out.push_str(&format!("  {state} at capture end: {layout}; {} requests open\n", s.open_requests));
    }
    for r in &report.root_styles {
        out.push_str(&format!(
            "  {}: overflow-y {}, position {}\n",
            r.element,
            r.overflow_y.as_deref().unwrap_or("-"),
            r.position.as_deref().unwrap_or("-")
        ));
    }
    if report.covering.is_empty() {
        out.push_str("  covering boxes: none\n");
    }
    for c in &report.covering {
        out.push_str(&format!(
            "  covering: {} {} z {} covers {}%{}\n",
            c.position,
            c.selector,
            c.z_index.as_deref().unwrap_or("auto"),
            c.covers_pct,
            if c.takes_centre { ", takes the centre" } else { "" }
        ));
    }
    for b in &report.bands {
        out.push_str(&format!(
            "  y {:.0}..{:.0}: text {}, images {}, empty painted {}, overlapping {}, past right edge {}\n",
            b.top, b.bottom, b.text, b.images, b.empty_painted, b.overlapping, b.past_right
        ));
    }
    if !report.not_checked.is_empty() {
        out.push_str("\nNot checked:\n");
    }
    for n in &report.not_checked {
        out.push_str(&format!("  {n}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lines(records: &[Value]) -> Vec<String> {
        event_causes(records).0.iter().map(|c| format!("{} {}", c.rule, c.line)).collect()
    }

    /// yahoo, 2026-10-24 (T3 binary): the ad loader arrived in 196 ms and
    /// waited 24.5 s for a turn; a page chunk's request never answered.
    #[test]
    fn a_starved_inserted_script_and_an_open_script_request_are_r4() {
        let records = vec![
            json!({"seq": 1, "t_ms": 5000.0, "type": "request", "id": 7, "url": "https://x.test/chunk.js", "resource_type": "script"}),
            json!({"seq": 2, "t_ms": 5766.0, "type": "lifecycle", "event": "DOMContentLoaded", "held_by": [], "held_by_count": 0}),
            json!({"seq": 3, "t_ms": 5766.0, "type": "lifecycle", "event": "load", "held_by": [], "held_by_count": 0}),
            json!({"seq": 4, "t_ms": 5881.0, "type": "load_finished"}),
            json!({"seq": 5, "t_ms": 5838.0, "type": "script", "script": "inline#34", "kind": "parser",
                   "outcome": "ran", "ran_ms": 1.0}),
            json!({"seq": 5, "t_ms": 32229.0, "type": "script", "script": "https://s.test/benji.js", "kind": "inserted",
                   "outcome": "ran", "reason": null, "fetch_ms": 196.0, "waited_ms": 24519.0, "held": null}),
        ];
        assert_eq!(
            lines(&records),
            vec![
                "R4 inserted script https://s.test/benji.js arrived in 196 ms and waited 24.5 s for a turn",
                "R4 1 script request(s) still open when the capture ended, first https://x.test/chunk.js at t=5.0 s",
            ]
        );
    }

    /// yahoo, 2026-10-24 (T3 + T4b): async scripts that waited behind parser
    /// scripts read as starved. A wait covered by other scripts running is a
    /// queue; only the idle part counts.
    #[test]
    fn an_async_script_queued_behind_running_scripts_is_not_starved() {
        let script = |name: &str, kind: &str, end: f64, waited: f64, ran: f64| {
            json!({"t_ms": end, "type": "script", "script": name, "kind": kind, "outcome": "ran",
                   "fetch_ms": 50.0, "waited_ms": waited, "ran_ms": ran})
        };
        let records = vec![
            // Parser scripts run 1.0..4.9 s and 9.0..11.0 s.
            script("https://x.test/p1.js", "parser", 4900.0, 0.0, 3900.0),
            script("https://x.test/p2.js", "parser", 11000.0, 0.0, 2000.0),
            // Waited 1.0..4.9 s, all of it behind p1: a queue.
            script("https://x.test/queued.js", "async", 5000.0, 3900.0, 100.0),
            // Waited 3.0..11.0 s: 4.0 s behind p1, queued.js and p2
            // (1.9 + 0.1 + 2.0), 4.0 s idle.
            script("https://x.test/idle.js", "async", 11100.0, 8000.0, 100.0),
        ];
        assert_eq!(
            lines(&records),
            vec![
                "R4 async script https://x.test/idle.js arrived in 50 ms and waited 8.0 s for a turn (4.0 s of it behind other scripts running)",
                // The idle part of that wait, once; p1's and p2's runs are not silence.
                "R8 event log silent 4.0 s from t=5.0 s before the load finished: no script ran, no request moved",
            ]
        );
    }

    /// yahoo, 2026-10-24 (LW + T3 + T4 stack): the async chunks' bodies were
    /// in by 1.7 s, but no page script ran until 5.8 s, after first layout
    /// and the last image. That is one line about the load, not one
    /// "starved" line per chunk.
    #[test]
    fn scripts_held_until_the_load_reaches_its_script_phase_are_one_r4_line() {
        let records = vec![
            json!({"t_ms": 900.0, "type": "request", "id": 8, "resource_type": "script", "url": "https://x.test/a.js"}),
            json!({"t_ms": 900.0, "type": "request", "id": 9, "resource_type": "image", "url": "https://x.test/i.png"}),
            json!({"t_ms": 1700.0, "type": "response", "id": 8}),
            json!({"t_ms": 4532.0, "type": "first_layout"}),
            json!({"t_ms": 5816.0, "type": "response", "id": 9}),
            json!({"t_ms": 5838.0, "type": "script", "script": "inline#34", "kind": "parser", "outcome": "ran", "ran_ms": 2.0}),
            json!({"t_ms": 6023.0, "type": "script", "script": "https://x.test/a.js", "kind": "async", "outcome": "ran",
                   "fetch_ms": 800.0, "waited_ms": 4153.0, "ran_ms": 170.0}),
        ];
        assert_eq!(
            lines(&records),
            vec![
                "R4 no page script ran until t=5.8 s: 1 script body(ies) had arrived 3.0 s or more before (first at t=1.7 s); first layout at t=4.5 s; last answer before it: image at t=5.8 s",
            ]
        );
    }

    #[test]
    fn scripts_that_never_finished_are_r3_but_nomodule_is_not() {
        let script = |name: &str, outcome: &str, reason: &str| {
            json!({"t_ms": 1.0, "type": "script", "script": name, "kind": "parser", "outcome": outcome, "reason": reason})
        };
        let records = vec![
            script("inline#1", "skipped", "nomodule"),
            script("https://x.test/big.js", "skipped", "too large to run inside the script budget"),
            script("inline#3", "stopped", "script budget"),
        ];
        assert_eq!(
            lines(&records),
            vec![
                "R3 parser script https://x.test/big.js skipped (too large to run inside the script budget)",
                "R3 parser script inline#3 stopped (script budget)",
            ]
        );
    }

    #[test]
    fn a_shield_block_comes_before_other_failures_and_cancels_are_left_out() {
        let records = vec![
            json!({"t_ms": 1.0, "type": "request", "id": 1, "url": "https://x.test/a.png", "resource_type": "image"}),
            json!({"t_ms": 1.0, "type": "request", "id": 2, "url": "https://ads.test/gpt.js", "resource_type": "script"}),
            json!({"t_ms": 1.0, "type": "request", "id": 3, "url": "https://x.test/b.js", "resource_type": "script"}),
            json!({"t_ms": 2.0, "type": "failure", "id": 1, "stage": "timeout", "reason": "timed out", "blocked_by": null}),
            json!({"t_ms": 2.0, "type": "failure", "id": 2, "stage": "policy", "reason": "blocked",
                   "blocked_by": {"list": "EasyList", "rule": "||ads.test^"}}),
            json!({"t_ms": 2.0, "type": "failure", "id": 3, "stage": "cancelled", "reason": "abandoned", "blocked_by": null}),
        ];
        assert_eq!(
            lines(&records),
            vec![
                "R6 shield blocked script https://ads.test/gpt.js (EasyList ||ads.test^)",
                "R6 image https://x.test/a.png failed: timeout (timed out)",
            ]
        );
    }

    /// cnn, 2026-10-24 (T3 binary): load 8.6 s after DOMContentLoaded, and
    /// the log silent from 11.85 s to 17.76 s.
    #[test]
    fn a_late_load_is_r7_and_a_silent_stretch_before_it_is_r8() {
        let records = vec![
            json!({"t_ms": 9751.0, "type": "lifecycle", "event": "DOMContentLoaded", "held_by": [], "held_by_count": 0}),
            json!({"t_ms": 11846.0, "type": "response", "id": 9}),
            json!({"t_ms": 17758.0, "type": "request", "id": 10, "url": "https://x.test/f", "resource_type": "fetch"}),
            json!({"t_ms": 18397.0, "type": "lifecycle", "event": "load",
                   "held_by": ["https://x.test/otSDKStub.js", "https://x.test/otBannerSdk.js"], "held_by_count": 2}),
            json!({"t_ms": 19151.0, "type": "load_finished"}),
            json!({"t_ms": 30000.0, "type": "relayout"}),
        ];
        assert_eq!(
            lines(&records),
            vec![
                "R7 load fired 8.6 s after DOMContentLoaded; it waited for: https://x.test/otSDKStub.js, https://x.test/otBannerSdk.js",
                "R8 event log silent 5.9 s from t=11.8 s before the load finished: no script ran, no request moved",
            ]
        );
    }

    /// yahoo, 2026-10-24 (LW + T3 + T4 stack): long after load, inserted
    /// chunks were in at 11.3 s and nothing was recorded until 15.4 s.
    #[test]
    fn a_silent_stretch_after_load_with_a_script_waiting_is_r8() {
        let records = vec![
            json!({"t_ms": 5000.0, "type": "script", "script": "inline#1", "kind": "parser", "outcome": "ran", "ran_ms": 10.0}),
            json!({"t_ms": 6602.0, "type": "load_finished"}),
            json!({"t_ms": 11318.0, "type": "response", "id": 145}),
            json!({"t_ms": 15364.0, "type": "request", "id": 152, "resource_type": "fetch", "url": "https://x.test/cfg"}),
            json!({"t_ms": 16745.0, "type": "script", "script": "https://x.test/67222.js", "kind": "inserted", "outcome": "ran",
                   "fetch_ms": 1853.0, "waited_ms": 5444.0, "ran_ms": 11.0}),
        ];
        assert_eq!(
            lines(&records),
            vec![
                "R4 inserted script https://x.test/67222.js arrived in 1853 ms and waited 5.4 s for a turn",
                "R8 event log silent 4.0 s from t=11.3 s with 1 script(s) in and waiting to run: no script ran, no request moved",
            ]
        );
    }

    /// T1's `script_error`: grouped by name and place, a rejection marked.
    #[test]
    fn uncaught_errors_are_r5_grouped_by_place() {
        let error = |line: u32, rejection: bool| {
            json!({"t_ms": 1.0, "type": "script_error", "name": "TypeError", "message": null,
                   "location": {"url": "https://x.test/7730.js", "line": line, "column": 12},
                   "unhandled_rejection": rejection, "script": "https://x.test/7730.js", "value": "error"})
        };
        assert_eq!(
            lines(&[error(3, false), error(3, false), error(9, true)]),
            vec![
                "R5 uncaught TypeError in https://x.test/7730.js:3:12 (x2)",
                "R5 uncaught TypeError (unhandled rejection) in https://x.test/7730.js:9:12",
            ]
        );
    }

    /// #802 review nit: a `file://` page has no host, so it has no site and
    /// no script error is "its own"; an IPv4 host is a site whole, not its
    /// last two octets.
    #[test]
    fn a_file_page_has_no_site_and_an_ip_host_is_whole() {
        assert_eq!(site_of("file:///tmp/fixture/index.html"), None);
        assert_eq!(site_of("http://192.168.1.20:8080/a.js"), Some("192.168.1.20".to_string()));
        assert_ne!(site_of("http://10.0.1.20/"), site_of("http://10.9.1.20/"));
        assert_eq!(site_of("https://www.yahoo.com/x.js"), Some("yahoo.com".to_string()));
    }

    /// yahoo, 2026-10-24: the site's own chunk 7730 throws a SyntaxError
    /// and the site makes no fetch of its own; that is ranked above the
    /// script waits (design note R5), an ad script's error is not.
    #[test]
    fn an_error_in_the_pages_own_site_ranks_before_r4() {
        let records = vec![
            json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"}),
            json!({"t_ms": 900.0, "type": "script_error", "name": "TypeError", "message": null,
                   "location": {"url": "https://ads.y.test/ad.js", "line": 1, "column": 5},
                   "unhandled_rejection": false, "script": "https://ads.y.test/ad.js", "value": "error"}),
            json!({"t_ms": 1000.0, "type": "script_error", "name": "SyntaxError", "message": null,
                   "location": {"url": "https://www.x.test/chunks/7730.js", "line": 1, "column": 6278},
                   "unhandled_rejection": false, "script": "https://www.x.test/chunks/7730.js", "value": "error"}),
            // Counted: only the www.x.test call after the error. Not: one
            // before it, another site's, and another host of the same site.
            json!({"t_ms": 800.0, "type": "request", "id": 3, "resource_type": "fetch", "url": "https://www.x.test/api/early"}),
            json!({"t_ms": 2000.0, "type": "request", "id": 4, "resource_type": "fetch", "url": "https://ads.y.test/bid"}),
            json!({"t_ms": 2100.0, "type": "request", "id": 5, "resource_type": "xhr", "url": "https://pbd.x.test/analytics"}),
            json!({"t_ms": 2200.0, "type": "request", "id": 6, "resource_type": "fetch", "url": "https://www.x.test/api/feed"}),
            json!({"t_ms": 5000.0, "type": "script", "script": "inline#1", "kind": "parser", "outcome": "ran", "ran_ms": 10.0}),
            json!({"t_ms": 16745.0, "type": "script", "script": "https://x.test/67222.js", "kind": "inserted", "outcome": "ran",
                   "fetch_ms": 1853.0, "waited_ms": 5444.0, "ran_ms": 11.0}),
            json!({"t_ms": 16800.0, "type": "load_finished"}),
        ];
        let mut report = page_report(&json!({"page": {"url": "https://www.x.test/", "viewport": [1280.0, 800.0]}}), None);
        apply_events(&mut report, &records, CaptureFacts::default());
        let ranked: Vec<String> = report.ranked.iter().map(|c| format!("{} {}", c.rule, c.line)).collect();
        // The dump has no elements, so R10 fires; the own-site uncaught error
        // is its cause and goes above it. R8 lines follow (the records are
        // sparse).
        assert!(ranked[1].starts_with("R10 "), "{ranked:?}");
        assert_eq!(
            [ranked[0].as_str(), ranked[2].as_str(), ranked[3].as_str()],
            [
                "R5 the page's own script: uncaught SyntaxError in https://www.x.test/chunks/7730.js:1:6278; at t=1.0 s; 1 fetch/XHR to www.x.test before, 1 after",
                "R4 inserted script https://x.test/67222.js arrived in 1853 ms and waited 5.4 s for a turn",
                "R5 uncaught TypeError in https://ads.y.test/ad.js:1:5",
            ]
        );
    }

    /// Atlas 10-26: an uncaught own-site error ranks above R10 (cause over
    /// symptom); an ad script's error, an own-site error the page caught and
    /// logged, and an own-site error on a page that painted do not.
    #[test]
    fn an_uncaught_own_site_error_ranks_above_a_blank_first_viewport() {
        let nav = json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"});
        let uncaught = |url: &str| {
            json!({"t_ms": 1000.0, "type": "script_error", "name": "SyntaxError", "message": null,
                   "location": {"url": url, "line": 1, "column": 6278},
                   "unhandled_rejection": false, "script": url, "value": "error"})
        };
        let logged = json!({"t_ms": 1200.0, "type": "console", "level": "error",
                            "message": "<TypeError message len=46>",
                            "error_stack": "    at f (https://www.x.test/app.js:3:9)"});
        let ranked = |records: &[Value], painted: bool| {
            let mut report = page_report(&json!({"page": {"url": "https://www.x.test/", "viewport": [1280.0, 800.0]}}), None);
            if painted {
                report.ranked.retain(|c| c.rule != "R10");
            }
            apply_events(&mut report, records, CaptureFacts::default());
            report.ranked.iter().map(|c| format!("{} {}", c.rule, c.line)).collect::<Vec<_>>()
        };
        let own = uncaught("https://www.x.test/chunks/7730.js");
        let own_line = "R5 the page's own script: uncaught SyntaxError in https://www.x.test/chunks/7730.js:1:6278; at t=1.0 s; 0 fetch/XHR to www.x.test before, 0 after";
        let blank = ranked(&[nav.clone(), own.clone()], false);
        assert_eq!(blank[0], own_line, "{blank:?}");
        assert!(blank[1].starts_with("R10 "), "{blank:?}");
        let ad = ranked(&[nav.clone(), uncaught("https://ads.y.test/ad.js")], false);
        assert!(ad[0].starts_with("R10 "), "{ad:?}");
        let caught = ranked(&[nav.clone(), logged], false);
        assert!(caught[0].starts_with("R10 "), "{caught:?}");
        // No R10: the own-site error keeps its R5* place.
        let painted = ranked(&[nav, own], true);
        assert!(painted.iter().all(|l| !l.starts_with("R10 ")), "{painted:?}");
        assert!(painted.contains(&own_line.to_string()), "{painted:?}");
    }

    /// yahoo with #777 merged: no uncaught error, but two TypeErrors caught
    /// and passed to console.error, then a blank page. Named by where they
    /// were thrown (the first `error_stack` frame), own site first.
    #[test]
    fn an_error_caught_and_logged_with_its_stack_is_r5() {
        let logged = |t: f64, stack: &str| {
            json!({"t_ms": t, "type": "console", "level": "error", "message": "<TypeError message len=46>",
                   "location": {"url": "https://www.x.test/7730.js", "line": 52, "column": 1849},
                   "stack": "    at https://www.x.test/7730.js:52:1854", "error_stack": stack})
        };
        let records = vec![
            json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"}),
            json!({"t_ms": 1.0, "type": "console", "level": "error", "message": "<string len=5>",
                   "location": {"url": "https://ads.y.test/a.js", "line": 1, "column": 1}}),
            logged(7808.0, "    at https://www.x.test/7730.js:1:6198\n    at https://www.x.test/7730.js:1:6074"),
            logged(9812.0, "    at d (https://www.x.test/7730.js:9:40)\n    at oQ (https://www.x.test/87c7.js:1:71835)"),
            logged(9900.0, "    at d (https://www.x.test/7730.js:9:40)"),
        ];
        assert_eq!(
            lines(&records)[..2],
            [
                "R5 the page's own script: logged TypeError thrown in https://www.x.test/7730.js:1:6198; at t=7.8 s; 0 fetch/XHR to www.x.test before, 0 after",
                "R5 the page's own script: logged TypeError thrown in https://www.x.test/7730.js:9:40 (x2); at t=9.8 s; 0 fetch/XHR to www.x.test before, 0 after",
            ]
        );
    }

    #[test]
    fn a_build_without_t2_or_t3_records_says_which_rules_it_skipped() {
        let (causes, skipped) = event_causes(&[json!({"t_ms": 1.0, "type": "load_finished"})]);
        assert!(causes.is_empty());
        assert_eq!(skipped.len(), 2, "{skipped:?}");
        assert!(skipped[0].starts_with("R3, R4") && skipped[1].starts_with("R7"));
    }

    /// T9 gap 2: yahoo's R5 read "then 2", "then 1", "then 0" for one error,
    /// because the error lands late in a fixed window; the same logs held 8
    /// to 10 own-host fetches before it. Both counts, and the time.
    #[test]
    fn an_own_site_error_says_when_it_fell_and_the_fetches_either_side() {
        let fetch = |t: f64, id: u64, url: &str, kind: &str| {
            json!({"t_ms": t, "type": "request", "id": id, "resource_type": kind, "url": url})
        };
        let records = vec![
            json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"}),
            fetch(100.0, 1, "https://www.x.test/a", "fetch"),
            fetch(200.0, 2, "https://www.x.test/b", "xhr"),
            fetch(300.0, 3, "https://www.x.test/c.js", "script"),
            fetch(400.0, 4, "https://cdn.x.test/d", "fetch"),
            json!({"t_ms": 24400.0, "type": "console", "level": "error", "message": "<Error message len=191>",
                   "error_stack": "    at f (https://www.x.test/app.js:1:2)"}),
            fetch(24500.0, 5, "https://www.x.test/e", "fetch"),
        ];
        // Other rules fire on the sparse records (an open script, a silent stretch).
        let all = lines(&records);
        assert!(
            all.iter().any(|l| l
                == "R5 the page's own script: logged Error thrown in https://www.x.test/app.js:1:2; at t=24.4 s; 2 fetch/XHR to www.x.test before, 1 after"),
            "{all:?}"
        );
    }

    fn settled_for(records: &[Value], capture_ms: Option<f64>) -> Report {
        let mut report = page_report(&json!({"page": {"url": "https://www.x.test/", "viewport": [1280.0, 800.0]}}), None);
        apply_events(&mut report, records, CaptureFacts { shield: false, capture_ms });
        report
    }

    /// T9 gap 4: yahoo's last relayout was 90 ms before the capture ended and
    /// one run had 20 requests open, and nothing said so. A record written
    /// after the window ended (a view's teardown cancels what is open) does
    /// not count as an answer.
    #[test]
    fn a_page_still_moving_at_capture_end_says_so() {
        let records = vec![
            json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"}),
            json!({"t_ms": 900.0, "type": "first_layout"}),
            json!({"t_ms": 1000.0, "type": "request", "id": 1, "resource_type": "fetch", "url": "https://www.x.test/a"}),
            json!({"t_ms": 1100.0, "type": "request", "id": 2, "resource_type": "image", "url": "https://www.x.test/b"}),
            json!({"t_ms": 1200.0, "type": "request", "id": 3, "resource_type": "script", "url": "https://www.x.test/c"}),
            json!({"t_ms": 1300.0, "type": "response", "id": 1}),
            json!({"t_ms": 24910.0, "type": "relayout", "cause": "", "took_ms": 1.0}),
            json!({"t_ms": 25010.0, "type": "failure", "id": 3, "stage": "cancelled", "reason": "abandoned", "blocked_by": null}),
        ];
        let report = settled_for(&records, Some(25000.0));
        assert_eq!(
            report.settled,
            Some(Settled { changing: true, last_relayout_ms_before: Some(90.0), open_requests: 2, navigated: false })
        );
        let text = report_text(&report);
        assert!(text.contains("  still changing at capture end: last relayout 90 ms before; 2 requests open\n"), "{text}");
    }

    #[test]
    fn a_quiet_page_is_settled_and_a_capture_without_a_window_says_nothing() {
        let records = vec![
            json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"}),
            json!({"t_ms": 900.0, "type": "first_layout"}),
            json!({"t_ms": 1000.0, "type": "request", "id": 1, "resource_type": "fetch", "url": "https://www.x.test/a"}),
            json!({"t_ms": 1300.0, "type": "failure", "id": 1, "stage": "timeout", "reason": "x", "blocked_by": null}),
        ];
        let report = settled_for(&records, Some(25000.0));
        assert_eq!(
            report.settled,
            Some(Settled { changing: false, last_relayout_ms_before: Some(24100.0), open_requests: 0, navigated: false })
        );
        let text = report_text(&report);
        assert!(text.contains("  settled at capture end: last relayout 24100 ms before; 0 requests open\n"), "{text}");
        let none = settled_for(&records, None);
        assert_eq!(none.settled, None);
        assert!(!report_text(&none).contains("capture end"));
    }

    /// Record times restart at a navigation, so the window's length says
    /// nothing about them: no figure, and the page was not finished.
    #[test]
    fn a_page_that_navigated_during_the_capture_gives_no_relayout_figure() {
        let records = vec![
            json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"}),
            json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/next"}),
            json!({"t_ms": 500.0, "type": "relayout", "cause": "", "took_ms": 1.0}),
        ];
        let report = settled_for(&records, Some(25000.0));
        assert_eq!(
            report.settled,
            Some(Settled { changing: true, last_relayout_ms_before: None, open_requests: 0, navigated: true })
        );
        let text = report_text(&report);
        assert!(text.contains("  still changing at capture end: the page navigated during the capture; 0 requests open\n"), "{text}");
    }

    /// T9 gap 5: `blocked_by` exists, as null, on every failure, so "no shield
    /// blocks" read as checked when no block was possible.
    #[test]
    fn a_capture_without_the_shield_says_nothing_could_be_blocked() {
        let failure = json!({"t_ms": 1.0, "type": "failure", "id": 1, "stage": "timeout", "reason": "x", "blocked_by": null});
        const LINE: &str = "R6 shield: not installed, nothing could be blocked";
        let without = settled_for(std::slice::from_ref(&failure), None);
        assert!(without.not_checked.iter().any(|n| n == LINE), "{:?}", without.not_checked);
        assert!(!without.shield);
        let mut with = page_report(&json!({"page": {"url": "https://www.x.test/", "viewport": [1280.0, 800.0]}}), None);
        apply_events(&mut with, &[failure], CaptureFacts { shield: true, capture_ms: None });
        assert!(with.not_checked.iter().all(|n| !n.starts_with("R6")), "{:?}", with.not_checked);
        assert!(with.shield);
    }

    /// T9: the logged Error was the top line and the feed's rows said nothing.
    /// The symptom on screen ranks with R9, above an own-site error.
    #[test]
    fn placeholders_rank_above_an_own_site_error() {
        let mut report = page_report(&json!({"page": {"url": "https://www.x.test/", "viewport": [1280.0, 800.0]}}), None);
        report.ranked = vec![Cause { rule: "R11", line: "placeholders".to_string() }];
        let records = vec![
            json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"}),
            json!({"t_ms": 1.0, "type": "console", "level": "error", "message": "<Error message len=1>",
                   "error_stack": "    at f (https://www.x.test/app.js:1:2)"}),
        ];
        apply_events(&mut report, &records, CaptureFacts::default());
        assert_eq!(report.ranked[0].rule, "R11", "{:?}", report.ranked);
        assert_eq!(report.ranked[1].rule, "R5", "{:?}", report.ranked);
    }

    #[test]
    fn an_empty_not_checked_section_prints_no_heading() {
        let mut report = settled_for(&[], None);
        assert!(report_text(&report).contains("\nNot checked:\n"));
        report.not_checked.clear();
        assert!(!report_text(&report).contains("Not checked"));
    }

    /// A dump's record, as `all_element_diagnostics` serialises it: a block
    /// with text in it, unless a test changes that.
    fn block(selector: &str, x: f64, y: f64, w: f64, h: f64) -> Value {
        let tag = selector.rsplit(" > ").next().unwrap_or("").split(':').next().unwrap_or("");
        json!({
            "identity": {"tag": tag, "selector": selector, "classes": []},
            "tree": {"child_count": 0, "text_length": 40},
            "style": {"position": "static", "display": "block"},
            "box": {"has_box": true, "border_box": {"x": x, "y": y, "w": w, "h": h}},
            "paint": {"clipped": false, "in_transform_layer": false, "effective_opacity": 1.0},
        })
    }

    fn with(mut element: Value, path: &[&str], value: Value) -> Value {
        let mut at = &mut element;
        for key in path {
            at = &mut at[*key];
        }
        *at = value;
        element
    }

    fn dump_of(elements: Vec<Value>) -> Value {
        json!({"page": {"url": "https://www.x.test/", "viewport": [1280.0, 800.0]}, "elements": elements})
    }

    fn band(report: &Report, top: f64) -> (u64, u64) {
        let band = report.bands.iter().find(|b| b.top == top).unwrap();
        (band.overlapping, band.past_right)
    }

    /// The top viewport has content, so no blank-band rule fires on these.
    /// Every band below is inside the document or past its end, so R9 is
    /// kept quiet by a text box in each.
    fn feed(extra: Vec<Value>) -> Value {
        let mut elements = vec![
            block("html > body > header", 0.0, 0.0, 1280.0, 100.0),
            block("html > body > nav", 0.0, 800.0, 1280.0, 10.0),
            block("html > body > footer", 0.0, 1600.0, 1280.0, 10.0),
        ];
        elements.extend(extra);
        dump_of(elements)
    }

    /// T9 gap 6: yahoo's footer and "Advertisement" labels painted over the
    /// feed, and the report had no word for it.
    #[test]
    fn siblings_lying_over_each_other_below_the_fold_are_r12() {
        let row = |i: usize, y: f64| block(&format!("html > body > main > div:nth-child({i})"), 0.0, y, 1200.0, 100.0);
        // Three rows each lie over the one before; the last clears them.
        let report = page_report(
            &feed(vec![row(1, 1700.0), row(2, 1710.0), row(3, 1720.0), row(4, 1730.0), row(5, 1900.0)]),
            None,
        );
        assert_eq!(band(&report, 1600.0), (3, 0), "{:?}", report.bands);
        let causes: Vec<String> = report.ranked.iter().map(|c| format!("{} {}", c.rule, c.line)).collect();
        assert_eq!(causes, vec!["R12 y 1600..2400: 3 boxes overlap other content, 0 run past the right edge"]);
        let text = report_text(&report);
        assert!(text.contains("  y 1600..2400: text 6, images 0, empty painted 0, overlapping 3, past right edge 0\n"), "{text}");
    }

    /// yahoo at 8f0488c6: no two siblings overlap, because a column's content
    /// runs out of its box and over the next column's. The boxes that lie over
    /// each other are 3 to 10 levels below their common ancestor.
    #[test]
    fn content_in_different_subtrees_overlapping_counts_and_its_wrappers_do_not() {
        let leaf = |column: usize, y: f64| block(&format!("html > body > main > div:nth-child({column}) > section > p"), 0.0, y, 1200.0, 100.0);
        let wrapper = |column: usize, y: f64| {
            with(block(&format!("html > body > main > div:nth-child({column})"), 0.0, y, 1200.0, 100.0), &["tree", "child_count"], json!(1))
        };
        let report = page_report(&feed(vec![leaf(1, 900.0), leaf(2, 900.0)]), None);
        assert_eq!(band(&report, 800.0), (1, 0), "{:?}", report.bands);
        let wrappers = page_report(&feed(vec![wrapper(1, 900.0), wrapper(2, 900.0)]), None);
        assert_eq!(band(&wrappers, 800.0), (0, 0), "{:?}", wrappers.bands);
    }

    #[test]
    fn half_an_overlap_or_a_box_that_paints_nothing_is_no_overlap() {
        let at = |i: usize, y: f64, h: f64| block(&format!("html > body > main > div:nth-child({i})"), 0.0, y, 1200.0, h);
        // Exactly half of the smaller box, and only touching.
        let half = page_report(&feed(vec![at(1, 900.0, 100.0), at(2, 950.0, 100.0), at(3, 1050.0, 100.0)]), None);
        assert_eq!(band(&half, 800.0), (0, 0), "{:?}", half.bands);
        // Inside its parent is not a sibling overlap.
        let parent = block("html > body > main", 0.0, 900.0, 1200.0, 300.0);
        let child = block("html > body > main > div", 0.0, 900.0, 1200.0, 300.0);
        assert_eq!(band(&page_report(&feed(vec![parent, child]), None), 800.0), (0, 0));
        // No text and no image: nothing to read under the other box. A coloured
        // card behind its own text is how pages are built, so it counts as none.
        let empty = with(at(1, 900.0, 100.0), &["tree", "text_length"], json!(0));
        assert_eq!(band(&page_report(&feed(vec![empty.clone(), at(2, 900.0, 100.0)]), None), 800.0), (0, 0));
        let tinted = with(empty, &["style", "background-color"], json!("rgb(1, 2, 3)"));
        assert_eq!(band(&page_report(&feed(vec![tinted, at(2, 900.0, 100.0)]), None), 800.0), (0, 0));
    }

    /// Stacked slides, a tooltip, a wrapped link's union rect: boxes whose
    /// layout rect does not say where they paint, or that overlap on purpose.
    #[test]
    fn out_of_flow_clipped_inline_and_transformed_boxes_are_not_counted() {
        let pair = |other: Value| {
            let first = block("html > body > main > div:nth-child(1)", 0.0, 900.0, 1200.0, 100.0);
            band(&page_report(&feed(vec![first, other]), None), 800.0)
        };
        let second = || block("html > body > main > div:nth-child(2)", 0.0, 900.0, 1200.0, 100.0);
        assert_eq!(pair(second()), (1, 0), "the control overlaps");
        for position in ["absolute", "fixed", "sticky"] {
            assert_eq!(pair(with(second(), &["style", "position"], json!(position))), (0, 0), "{position}");
        }
        assert_eq!(pair(with(second(), &["paint", "clipped"], json!(true))), (0, 0), "clipped");
        assert_eq!(pair(with(second(), &["paint", "in_transform_layer"], json!(true))), (0, 0), "transformed");
        assert_eq!(pair(with(second(), &["paint", "effective_opacity"], json!(0.0))), (0, 0), "faded by an ancestor");
        assert_eq!(pair(with(second(), &["style", "display"], json!("inline"))), (0, 0), "inline");
        // In-flow, but inside an out-of-flow box: not the document's flow.
        let drawer = with(block("html > body > aside", 0.0, 900.0, 1200.0, 100.0), &["style", "position"], json!("absolute"));
        let inside = |i: usize| block(&format!("html > body > aside > div:nth-child({i})"), 0.0, 900.0, 1200.0, 100.0);
        assert_eq!(band(&page_report(&feed(vec![drawer, inside(1), inside(2)]), None), 800.0), (0, 0));
    }

    /// An image in its own `picture`, or text in a box the fixture calls a
    /// leaf: one thing, not two.
    #[test]
    fn a_box_and_its_own_descendant_do_not_overlap() {
        let outer = block("html > body > main > picture", 0.0, 900.0, 1200.0, 100.0);
        let inner = block("html > body > main > picture > img", 0.0, 900.0, 1200.0, 100.0);
        assert_eq!(band(&page_report(&feed(vec![outer, inner]), None), 800.0), (0, 0));
    }

    /// T9 gap 6: hero and row content ran past the right edge. A wide box's
    /// children are one fact, and a carousel's clipped slides are not one.
    #[test]
    fn boxes_past_the_right_edge_count_outermost_and_unclipped() {
        let wide = |i: usize| block(&format!("html > body > main > section:nth-child({i})"), 100.0, 900.0 + i as f64 * 200.0, 1300.0, 100.0);
        let inner = block("html > body > main > section:nth-child(1) > div", 100.0, 1100.0, 1300.0, 50.0);
        let slide = with(block("html > body > main > ul > li", 1000.0, 1500.0, 400.0, 50.0), &["paint", "clipped"], json!(true));
        let fits = block("html > body > main > section:nth-child(9)", 0.0, 1210.0, 1280.0, 50.0);
        let report = page_report(&feed(vec![wide(1), inner, wide(2), wide(3), slide, fits]), None);
        assert_eq!(band(&report, 800.0), (0, 3), "{:?}", report.bands);
        let causes: Vec<String> = report.ranked.iter().map(|c| format!("{} {}", c.rule, c.line)).collect();
        assert_eq!(causes, vec!["R12 y 800..1600: 0 boxes overlap other content, 3 run past the right edge"]);
        // A body wider than the viewport does not hide the boxes in it.
        let body = block("html > body", 0.0, 0.0, 1400.0, 2000.0);
        let child = block("html > body > div", 0.0, 900.0, 1400.0, 100.0);
        assert_eq!(band(&page_report(&dump_of(vec![body, child]), None), 800.0), (0, 1));
    }

    #[test]
    fn a_few_stray_boxes_do_not_fire_r12_but_the_facts_count_them() {
        let row = |i: usize| block(&format!("html > body > main > div:nth-child({i})"), 0.0, 900.0, 1200.0, 100.0);
        let report = page_report(&feed(vec![row(1), row(2), row(3)]), None);
        assert_eq!(band(&report, 800.0), (2, 0), "{:?}", report.bands);
        assert!(report.ranked.is_empty(), "{:?}", report.ranked);
    }

    /// R12 follows R11 (the plan's order) and an own-site error follows both.
    #[test]
    fn overlap_ranks_after_placeholders_and_before_an_own_site_error() {
        let mut report = page_report(&json!({"page": {"url": "https://www.x.test/", "viewport": [1280.0, 800.0]}}), None);
        report.ranked = vec![
            Cause { rule: "R12", line: "overlap".to_string() },
            Cause { rule: "R11", line: "placeholders".to_string() },
        ];
        let records = vec![
            json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"}),
            json!({"t_ms": 1.0, "type": "console", "level": "error", "message": "<Error message len=1>",
                   "error_stack": "    at f (https://www.x.test/app.js:1:2)"}),
        ];
        apply_events(&mut report, &records, CaptureFacts::default());
        let order: Vec<&str> = report.ranked.iter().take(3).map(|c| c.rule).collect();
        assert_eq!(order, ["R11", "R12", "R5"], "{:?}", report.ranked);
    }

    fn logged_string(t: f64, location: Value, stack: Value) -> Value {
        json!({"t_ms": t, "type": "console", "level": "error", "message": "<string len=161>",
               "location": location, "stack": stack, "error_stack": null})
    }

    fn r5(records: &[Value]) -> Vec<String> {
        lines(records).into_iter().filter(|l| l.starts_with("R5 ")).collect()
    }

    /// T9 gap 7: yahoo's layout chunk logged `<string len=161>` at 23.9 s and
    /// R5 never showed it, because it reads only records with `error_stack`.
    /// Named by the script that logged it; never the text.
    #[test]
    fn an_own_site_string_console_error_is_r5_named_by_its_script() {
        let nav = json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"});
        let at = |url: &str, line: u32| json!({"url": url, "line": line, "column": 9});
        let records = vec![
            nav.clone(),
            json!({"t_ms": 10.0, "type": "request", "id": 1, "resource_type": "fetch", "url": "https://www.x.test/a"}),
            logged_string(23900.0, at("https://www.x.test/layout.js", 3), json!("    at https://www.x.test/layout.js:3:9")),
            logged_string(24000.0, at("https://www.x.test/layout.js", 3), Value::Null),
            logged_string(24100.0, at("https://ads.y.test/a.js", 1), Value::Null),
        ];
        assert_eq!(
            r5(&records),
            vec!["R5 the page's own script: logged string in https://www.x.test/layout.js:3:9 (x2); at t=23.9 s; 1 fetch/XHR to www.x.test before, 0 after"]
        );
        // The page's inline script is the page's own.
        assert_eq!(
            r5(&[nav.clone(), logged_string(5.0, at("inline#3", 1), Value::Null)]),
            vec!["R5 the page's own script: logged string in inline#3:1:9; at t=0.0 s; 0 fetch/XHR to www.x.test before, 0 after"]
        );
        // No location (eval, an `on*` attribute): the first caller frame; no
        // frame either: nothing to name it by.
        assert_eq!(
            r5(&[nav.clone(), logged_string(5.0, Value::Null, json!("    at f (https://www.x.test/b.js:7:1)\n    at https://www.y.test/c.js:1:1"))]),
            vec!["R5 the page's own script: logged string in https://www.x.test/b.js:7:1; at t=0.0 s; 0 fetch/XHR to www.x.test before, 0 after"]
        );
        assert_eq!(r5(&[nav.clone(), logged_string(5.0, Value::Null, Value::Null)]), Vec::<String>::new());
        // Text never reaches the line on a run that kept it, nor do warnings count.
        let kept = json!({"t_ms": 5.0, "type": "console", "level": "error", "message": "SECRET text",
                          "location": at("https://www.x.test/a.js", 1)});
        assert_eq!(r5(&[nav.clone(), kept]), Vec::<String>::new());
        let warning = with(logged_string(5.0, at("https://www.x.test/a.js", 1), Value::Null), &["level"], json!("warn"));
        assert_eq!(r5(&[nav.clone(), warning]), Vec::<String>::new());
        // A later string argument is still a string call.
        let second = with(logged_string(5.0, at("https://www.x.test/a.js", 1), Value::Null), &["message"], json!("<number> <string len=3>"));
        assert_eq!(r5(&[nav, second]).len(), 1);
    }

    /// yahoo at 8f0488c6: a script on s.yimg.com wraps `console.error`, so
    /// every call is placed in the wrapper. The own-site frame under it names
    /// the logger. A consent script's string under the same wrapper stays out.
    #[test]
    fn a_console_error_wrapper_does_not_hide_the_own_site_script_under_it() {
        let nav = json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"});
        let wrapper = json!({"url": "https://s.y.test/rum.js", "line": 1, "column": 2});
        let own = "    at w (https://s.y.test/rum.js:1:2)\n    at f (https://www.x.test/layout.js:3:9)\n    at https://www.x.test/app.js:1:1";
        assert_eq!(
            r5(&[nav.clone(), logged_string(5.0, wrapper.clone(), json!(own))]),
            vec!["R5 the page's own script: logged string in https://www.x.test/layout.js:3:9; at t=0.0 s; 0 fetch/XHR to www.x.test before, 0 after"]
        );
        let consent = "    at w (https://s.y.test/rum.js:1:2)\n    at c (https://consent.z.test/cmp.js:8:8)";
        assert_eq!(r5(&[nav, logged_string(5.0, wrapper, json!(consent))]), Vec::<String>::new());
    }

    /// yahoo at 2dc9a3b2: `console.error("text", err)` read "logged string
    /// thrown in", one line under the same error logged alone.
    #[test]
    fn an_error_logged_after_a_string_is_named_by_the_error() {
        let logged = |t: f64, message: &str| {
            json!({"t_ms": t, "type": "console", "level": "error", "message": message,
                   "error_stack": "    at f (https://www.x.test/app.js:1:2)"})
        };
        let records = vec![
            json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"}),
            logged(1000.0, "<TypeError message len=4>"),
            logged(1100.0, "<string len=9> <TypeError message len=4> <number>"),
        ];
        assert_eq!(
            r5(&records),
            vec!["R5 the page's own script: logged TypeError thrown in https://www.x.test/app.js:1:2 (x2); at t=1.0 s; 0 fetch/XHR to www.x.test before, 0 after"]
        );
    }

    /// R5 prints three lines. A chatty logger must not push a real error off.
    #[test]
    fn own_site_strings_do_not_push_an_own_site_error_off_the_list() {
        let at = |line: u32| json!({"url": "https://www.x.test/a.js", "line": line, "column": 1});
        let mut records = vec![json!({"t_ms": 0.0, "type": "navigation_start", "url": "https://www.x.test/"})];
        for line in 1..=3 {
            records.push(logged_string(line as f64, at(line), Value::Null));
        }
        records.push(json!({"t_ms": 900.0, "type": "console", "level": "error", "message": "<Error message len=1>",
                            "error_stack": "    at f (https://www.x.test/app.js:1:2)"}));
        records.push(json!({"t_ms": 950.0, "type": "script_error", "name": "TypeError", "message": null,
                            "location": {"url": "https://www.x.test/b.js", "line": 1, "column": 1},
                            "unhandled_rejection": false, "script": "https://www.x.test/b.js", "value": "error"}));
        let all = r5(&records);
        assert_eq!(all.len(), 3, "{all:?}");
        assert!(all[0].contains("logged string") == false, "{all:?}");
        assert!(all.iter().any(|l| l.contains("uncaught TypeError")), "{all:?}");
        assert!(all.iter().any(|l| l.contains("logged Error thrown")), "{all:?}");
        assert!(all[2].contains("logged string"), "{all:?}");
    }
}
