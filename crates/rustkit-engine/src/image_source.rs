//! E54b: which image an `<img>` loads (HTML "select an image source", §4.8.4.3): the `<picture>`
//! sources, `srcset` with `x` and `w` descriptors, `sizes`, `type` and `media`, and the density of
//! the pick (so the natural size can be corrected by it).
//!
//! Pure functions over attribute strings, so the whole selection is unit-tested against what
//! Chrome answers on a matrix page (`docs/diagnostics/2026-11-10/e54b_srcset_matrix.md`).

/// What the selection reads from one source of images: an `<img>` or a `<picture>`'s `<source>`.
#[derive(Debug, Default, Clone)]
pub struct SourceAttrs<'a> {
    pub srcset: Option<&'a str>,
    pub sizes: Option<&'a str>,
    pub media: Option<&'a str>,
    pub mime: Option<&'a str>,
    /// Only the `<img>`'s own `src` (a `<source>` has none).
    pub src: Option<&'a str>,
}

/// The environment a selection is made in.
#[derive(Debug, Clone, Copy)]
pub struct Env {
    pub viewport_w: f32,
    pub viewport_h: f32,
    pub dpr: f32,
}

/// The chosen image and its density: a decoded image of N pixels is N / `density` CSS px wide.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub url: String,
    pub density: f32,
}

/// A parsed `srcset` candidate: a URL and either a pixel density (`x`) or a width (`w`).
#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    url: String,
    kind: Descriptor,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Descriptor {
    X(f32),
    W(f32),
}

/// Parse a `srcset` value (HTML "parse a srcset attribute"): a URL is a run of non-whitespace
/// characters and MAY contain commas (`data:image/png;base64,...`); trailing commas are stripped
/// from it and end the candidate; otherwise the descriptors run to the next comma. No descriptor
/// is `1x`. A candidate with a descriptor that is not a positive `Nx` or `Nw` (or two descriptors)
/// is dropped.
fn parse_srcset(srcset: &str) -> Vec<Candidate> {
    let chars: Vec<char> = srcset.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && (chars[i].is_whitespace() || chars[i] == ',') {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        let mut url: String = chars[start..i].iter().collect();
        let mut descriptors = String::new();
        if url.ends_with(',') {
            url = url.trim_end_matches(',').to_string();
        } else {
            let dstart = i;
            let mut depth = 0i32;
            while i < chars.len() {
                match chars[i] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    ',' if depth <= 0 => break,
                    _ => {}
                }
                i += 1;
            }
            descriptors = chars[dstart..i].iter().collect();
        }
        if url.is_empty() {
            continue;
        }
        let mut parts = descriptors.split_whitespace();
        let kind = match (parts.next(), parts.next()) {
            (None, _) => Descriptor::X(1.0),
            (Some(d), None) => {
                let (num, unit) = d.split_at(d.len().saturating_sub(1));
                match (num.parse::<f32>(), unit) {
                    (Ok(n), "x") if n > 0.0 => Descriptor::X(n),
                    (Ok(n), "w") if n > 0.0 => Descriptor::W(n),
                    _ => continue,
                }
            }
            _ => continue,
        };
        out.push(Candidate { url, kind });
    }
    out
}

/// The image types RustKit decodes. A `<source type>` outside this list is skipped, so a page
/// offering AVIF first still gets its fallback.
fn type_supported(mime: &str) -> bool {
    let mime = mime.trim().to_ascii_lowercase();
    matches!(
        mime.as_str(),
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp" | "image/bmp" | "image/svg+xml" | "image/x-icon" | "image/vnd.microsoft.icon"
    )
}

/// One `sizes` length in CSS px: `px`, `vw`, `vh`, `em`/`rem` (16 px), a bare 0, or a math
/// function (`calc()`, `min()`, `max()`, `clamp()`) over those, never negative. Anything else is
/// unknown (`None`).
fn size_px(token: &str, env: &Env) -> Option<f32> {
    let t = token.trim();
    if t == "0" {
        return Some(0.0);
    }
    // IMG1: `sizes="calc(50vw - 100px)"` was unknown, so the slot fell back to 100vw and the
    // image was sized 800 wide where Chrome 148 has 300. A percentage has no meaning in `sizes`.
    if t.contains('(') {
        return match rustkit_css::parse_length(t)? {
            length @ (rustkit_css::Length::Calc(_)
            | rustkit_css::Length::Min(..)
            | rustkit_css::Length::Max(..)
            | rustkit_css::Length::Clamp(..)) => {
                let px = length.to_px_with_viewport(16.0, 16.0, 0.0, env.viewport_w, env.viewport_h);
                px.is_finite().then_some(px.max(0.0))
            }
            _ => None,
        };
    }
    let split = t.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = t.split_at(split);
    let n: f32 = num.parse().ok()?;
    match unit.to_ascii_lowercase().as_str() {
        "px" => Some(n),
        "vw" => Some(n * env.viewport_w / 100.0),
        "vh" => Some(n * env.viewport_h / 100.0),
        "em" | "rem" => Some(n * 16.0),
        _ => None,
    }
}

/// The source size (HTML "parse a sizes attribute"): the first entry whose media condition
/// matches, else the last entry (the one with no condition), else `100vw`.
fn source_size(sizes: Option<&str>, env: &Env) -> f32 {
    let default = env.viewport_w;
    let Some(sizes) = sizes else { return default };
    // Split at top-level commas (a media condition may hold parentheses, not commas).
    let mut entries: Vec<&str> = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in sizes.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                entries.push(&sizes[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    entries.push(&sizes[start..]);
    for entry in entries {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        // `(media condition) length`, or just `length`: the condition runs to its matching paren.
        let (condition, length) = if entry.starts_with('(') {
            let mut depth = 0i32;
            let mut end = entry.len();
            for (i, c) in entry.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            (entry[..end].trim(), entry[end..].trim())
        } else {
            ("", entry)
        };
        let Some(px) = size_px(length, env) else { continue };
        if condition.is_empty() || rustkit_css::media::media_query_list_matches(condition, env.viewport_w, env.viewport_h) {
            return px;
        }
    }
    default
}

/// Pick one candidate of a set for `dpr` (HTML "select an image source from a source set"): the
/// smallest density that covers the DPR, else the largest. `w` candidates have density
/// `w / slot`. Returns the URL and its density.
fn pick_candidate(candidates: &[Candidate], slot: f32, dpr: f32) -> Option<Pick> {
    let mut with_density: Vec<(f32, &Candidate)> = candidates
        .iter()
        .filter_map(|c| match c.kind {
            Descriptor::X(x) => Some((x, c)),
            Descriptor::W(w) if slot > 0.0 => Some((w / slot, c)),
            Descriptor::W(_) => None,
        })
        .collect();
    if with_density.is_empty() {
        return None;
    }
    // Stable by density, so equal densities keep document order (the first wins).
    with_density.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let chosen = with_density
        .iter()
        .find(|(d, _)| *d >= dpr)
        .or_else(|| with_density.last())?;
    Some(Pick { url: chosen.1.url.clone(), density: chosen.0 })
}

/// The candidates and size slot of one source of images.
fn pick_from(set: &SourceAttrs<'_>, env: &Env, with_src: bool) -> Option<Pick> {
    let mut candidates = parse_srcset(set.srcset.unwrap_or(""));
    let has_width = candidates.iter().any(|c| matches!(c.kind, Descriptor::W(_)));
    if with_src && !has_width {
        if let Some(src) = set.src.map(str::trim).filter(|s| !s.is_empty()) {
            // `src` is a 1x candidate unless one already is.
            let has_1x = candidates.iter().any(|c| c.kind == Descriptor::X(1.0));
            if !has_1x {
                candidates.push(Candidate { url: src.to_string(), kind: Descriptor::X(1.0) });
            }
        }
    }
    let slot = source_size(set.sizes, env);
    pick_candidate(&candidates, slot, env.dpr)
}

/// Select the image for an `<img>` whose `<picture>` sources (those before it) are `sources`:
/// the first `<source>` whose `media` matches, whose `type` is supported and that offers a
/// usable `srcset` decides; otherwise the `<img>`'s own `srcset`, else its `src`.
pub fn select_source(img: &SourceAttrs<'_>, sources: &[SourceAttrs<'_>], env: &Env) -> Option<Pick> {
    for source in sources {
        if let Some(mime) = source.mime {
            if !type_supported(mime) {
                continue;
            }
        }
        if let Some(media) = source.media {
            if !media.trim().is_empty()
                && !rustkit_css::media::media_query_list_matches(media, env.viewport_w, env.viewport_h)
            {
                continue;
            }
        }
        if source.srcset.map(str::trim).filter(|s| !s.is_empty()).is_none() {
            continue;
        }
        if let Some(pick) = pick_from(source, env, false) {
            return Some(pick);
        }
    }
    if let Some(pick) = pick_from(img, env, true) {
        return Some(pick);
    }
    img.src.map(str::trim).filter(|s| !s.is_empty()).map(|s| Pick { url: s.to_string(), density: 1.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENVS: [(&str, Env); 4] = [
        ("dpr1_vw500", Env { viewport_w: 500.0, viewport_h: 900.0, dpr: 1.0 }),
        ("dpr1_vw784", Env { viewport_w: 784.0, viewport_h: 900.0, dpr: 1.0 }),
        ("dpr2_vw492", Env { viewport_w: 492.0, viewport_h: 900.0, dpr: 2.0 }),
        ("dpr2_vw784", Env { viewport_w: 784.0, viewport_h: 900.0, dpr: 2.0 }),
    ];

    /// (case, image, picture sources, expected per environment: (url, decoded px width of that
    /// url, natural width Chrome reported)). Chrome 157 on Windows, `--force-device-scale-factor`
    /// and `--window-size`, via `img.currentSrc` / `img.naturalWidth` (see the matrix page).
    type Expected = [(&'static str, f32, f32); 4];

    fn check(name: &str, img: SourceAttrs<'_>, sources: Vec<SourceAttrs<'_>>, expected: Expected) {
        for ((env_name, env), (url, px, natural)) in ENVS.iter().zip(expected.iter()) {
            let pick = select_source(&img, &sources, env)
                .unwrap_or_else(|| panic!("{name} @ {env_name}: nothing picked"));
            assert_eq!(pick.url, *url, "{name} @ {env_name}: which image");
            let got_natural = px / pick.density;
            assert!(
                (got_natural - natural).abs() < 1.0,
                "{name} @ {env_name}: natural width {got_natural} (density {}), Chrome {natural}",
                pick.density
            );
        }
    }

    fn img<'a>() -> SourceAttrs<'a> {
        SourceAttrs::default()
    }

    #[test]
    fn x_descriptors_pick_the_smallest_density_that_covers_the_dpr() {
        let a = SourceAttrs { srcset: Some("i100.png 1x, i200.png 2x"), ..img() };
        check("c1", a.clone(), vec![], [("i100.png", 100.0, 100.0), ("i100.png", 100.0, 100.0), ("i200.png", 200.0, 100.0), ("i200.png", 200.0, 100.0)]);
        let b = SourceAttrs { srcset: Some("i100.png 1x, i200.png 2x, i400.png 4x"), ..img() };
        check("c2", b, vec![], [("i100.png", 100.0, 100.0), ("i100.png", 100.0, 100.0), ("i200.png", 200.0, 100.0), ("i200.png", 200.0, 100.0)]);
        let only2x = SourceAttrs { srcset: Some("i200.png 2x"), ..img() };
        check("c3", only2x, vec![], [("i200.png", 200.0, 100.0); 4]);
    }

    #[test]
    fn src_is_the_1x_candidate_when_no_candidate_is_1x() {
        let c4 = SourceAttrs { src: Some("i50.png"), srcset: Some("i100.png 1.5x, i300.png 3x"), ..img() };
        check("c4", c4, vec![], [("i50.png", 50.0, 50.0), ("i50.png", 50.0, 50.0), ("i300.png", 300.0, 100.0), ("i300.png", 300.0, 100.0)]);
        let c14 = SourceAttrs { src: Some("i50.png"), srcset: Some(""), ..img() };
        check("c14", c14, vec![], [("i50.png", 50.0, 50.0); 4]);
    }

    #[test]
    fn a_candidate_with_no_descriptor_is_1x() {
        let c15 = SourceAttrs { srcset: Some("i100.png, i200.png 2x"), ..img() };
        check("c15", c15, vec![], [("i100.png", 100.0, 100.0), ("i100.png", 100.0, 100.0), ("i200.png", 200.0, 100.0), ("i200.png", 200.0, 100.0)]);
    }

    #[test]
    fn w_descriptors_with_sizes_pick_by_the_slot_width_and_the_natural_size_is_the_slot() {
        // No sizes means 100vw; the natural width is the slot (Chrome: 500, 784, 492, 784).
        let w = "i100.png 100w, i200.png 200w, i400.png 400w";
        check("c5", SourceAttrs { srcset: Some(w), sizes: Some("100vw"), ..img() }, vec![], [("i400.png", 400.0, 500.0), ("i400.png", 400.0, 784.0), ("i400.png", 400.0, 492.0), ("i400.png", 400.0, 784.0)]);
        check("c7", SourceAttrs { srcset: Some(w), ..img() }, vec![], [("i400.png", 400.0, 500.0), ("i400.png", 400.0, 784.0), ("i400.png", 400.0, 492.0), ("i400.png", 400.0, 784.0)]);
        check(
            "c6",
            SourceAttrs { srcset: Some(w), sizes: Some("(min-width: 600px) 300px, 100px"), ..img() },
            vec![],
            [("i100.png", 100.0, 100.0), ("i400.png", 400.0, 300.0), ("i200.png", 200.0, 100.0), ("i400.png", 400.0, 300.0)],
        );
        check("c9", SourceAttrs { srcset: Some(w), sizes: Some("150px"), ..img() }, vec![], [("i200.png", 200.0, 150.0), ("i200.png", 200.0, 150.0), ("i400.png", 400.0, 150.0), ("i400.png", 400.0, 150.0)]);
    }

    #[test]
    fn picture_sources_win_in_order_by_media_and_type() {
        let base = SourceAttrs { src: Some("i50.png"), ..img() };
        // c10: media min-width 600 first, then an unconditional source.
        let c10 = vec![
            SourceAttrs { media: Some("(min-width: 600px)"), srcset: Some("i400.png"), ..img() },
            SourceAttrs { srcset: Some("i100.png"), ..img() },
        ];
        check("c10", base.clone(), c10, [("i100.png", 100.0, 100.0), ("i400.png", 400.0, 400.0), ("i100.png", 100.0, 100.0), ("i400.png", 400.0, 400.0)]);
        // c11: an unsupported type is skipped; the img's own srcset decides.
        let c11 = vec![SourceAttrs { mime: Some("image/foo"), srcset: Some("i400.png"), ..img() }];
        let own = SourceAttrs { src: Some("i50.png"), srcset: Some("i100.png 1x, i200.png 2x"), ..img() };
        check("c11", own, c11, [("i100.png", 100.0, 100.0), ("i100.png", 100.0, 100.0), ("i200.png", 200.0, 100.0), ("i200.png", 200.0, 100.0)]);
        // c12: a supported type with x descriptors.
        let c12 = vec![SourceAttrs { mime: Some("image/png"), srcset: Some("i200.png 1x, i400.png 2x"), ..img() }];
        check("c12", base.clone(), c12, [("i200.png", 200.0, 200.0), ("i200.png", 200.0, 200.0), ("i400.png", 400.0, 200.0), ("i400.png", 400.0, 200.0)]);
        // c13: a source with w descriptors and its own sizes.
        let c13 = vec![SourceAttrs { srcset: Some("i100.png 100w, i300.png 300w"), sizes: Some("200px"), ..img() }];
        check("c13", base, c13, [("i300.png", 300.0, 200.0); 4]);
    }

    #[test]
    fn a_url_may_contain_commas_and_trailing_commas_end_the_candidate() {
        let c = parse_srcset("data:image/png;base64,AAAA 2x, b.png, c.png 3x,");
        let urls: Vec<&str> = c.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(urls, ["data:image/png;base64,AAAA", "b.png", "c.png"]);
        assert_eq!(c[0].kind, Descriptor::X(2.0));
        assert_eq!(c[1].kind, Descriptor::X(1.0));
        assert_eq!(c[2].kind, Descriptor::X(3.0));
        // A bad descriptor drops that candidate only.
        let d = parse_srcset("a.png 0x, b.png 2q, c.png 1.5x");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].url, "c.png");
    }

    #[test]
    fn nothing_to_load_picks_nothing() {
        assert_eq!(select_source(&img(), &[], &ENVS[0].1), None);
    }
}
