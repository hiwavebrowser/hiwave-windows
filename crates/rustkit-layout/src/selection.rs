//! Text selection geometry (CH2.E2): which character of which DOM text node
//! is under a point, and how a text run's [`TextSource`]s follow the run
//! when the box builder joins runs and strips their edge spaces.
//!
//! Painting the selection lives with the text it paints behind
//! (`DisplayList::build_with_selection`, `render_text`).

use crate::{
    apply_text_transform, measure_text_advanced, run_line_height, shape_line_advances, BoxType,
    LayoutBox, Rect, TextSource,
};
use rustkit_css::Length;

/// A caret position in a text run: the DOM text node and the index into
/// its white-space-collapsed characters (see [`crate::collapse_map`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPosition {
    pub node: usize,
    pub offset: usize,
    /// Whether the node's white space collapses (see [`TextSource`]).
    pub collapsible: bool,
}

/// One painted line of a text run: its text, its rect, and the index of its
/// first character in the run.
struct RunLine {
    text: String,
    rect: Rect,
    start: usize,
}

/// The lines of a text run where `render_text` paints them.
fn run_lines(layout_box: &LayoutBox, raw: &str) -> Vec<RunLine> {
    let style = &layout_box.style;
    let font_size = match style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    };
    if font_size <= 0.0 {
        return Vec::new();
    }
    let text = apply_text_transform(raw, style.text_transform);
    let metrics = measure_text_advanced(&text, &style.font_family, font_size, style.font_weight, style.font_style);
    let line_height = run_line_height(style, font_size, &metrics);
    let content = layout_box.dimensions.content;
    match &layout_box.text_lines {
        Some(lines) => {
            let mut cursor = 0usize;
            lines
                .iter()
                .enumerate()
                .filter(|(_, l)| !l.text.is_empty())
                .map(|(i, l)| {
                    let rest: String = raw.chars().skip(cursor).collect();
                    let found = rest.find(l.text.as_str()).map(|b| rest[..b].chars().count()).unwrap_or(0);
                    let start = cursor + found;
                    cursor = start + l.text.chars().count();
                    RunLine {
                        text: apply_text_transform(&l.text, style.text_transform),
                        rect: l.fragment_rect(i, content.x, content.y, line_height),
                        start,
                    }
                })
                .collect()
        }
        None => vec![RunLine {
            text,
            rect: Rect::new(content.x, content.y, content.width, content.height.max(line_height)),
            start: 0,
        }],
    }
}

/// How far a point is from a rect: vertical distance first (the line the
/// point is on, or the nearest line), then horizontal.
fn distance(rect: &Rect, x: f32, y: f32) -> (f32, f32) {
    let dy = if y < rect.y {
        rect.y - y
    } else if y > rect.y + rect.height {
        y - (rect.y + rect.height)
    } else {
        0.0
    };
    let dx = if x < rect.x {
        rect.x - x
    } else if x > rect.x + rect.width {
        x - (rect.x + rect.width)
    } else {
        0.0
    };
    (dy, dx)
}

impl LayoutBox {
    /// The text position nearest to a point in document coordinates: on
    /// the text line under the point, else on the nearest line (above or
    /// below, then left or right), the character boundary nearest to `x`.
    /// `None` when the tree has no text that came from a DOM text node.
    ///
    /// Positions are where layout put the runs: element scrolls, sticky
    /// shifts and transforms are not followed.
    pub fn text_position_at(&self, x: f32, y: f32) -> Option<TextPosition> {
        let mut best: Option<((f32, f32), &LayoutBox, RunLine)> = None;
        self.visit_text_runs(&mut |run, raw| {
            for line in run_lines(run, raw) {
                let d = distance(&line.rect, x, y);
                if best.as_ref().is_none_or(|(bd, _, _)| d < *bd) {
                    best = Some((d, run, line));
                }
            }
        });
        let (_, run, line) = best?;
        let style = &run.style;
        let font_size = match style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };
        let advances = shape_line_advances(&line.text, style, font_size).unwrap_or_else(|| {
            line.text
                .chars()
                .map(|c| {
                    measure_text_advanced(&c.to_string(), &style.font_family, font_size, style.font_weight, style.font_style)
                        .width
                })
                .collect()
        });
        let mut index = 0usize;
        let mut left = line.rect.x;
        for adv in &advances {
            if x < left + adv / 2.0 {
                break;
            }
            left += adv;
            index += 1;
        }
        run.position_of_run_char(line.start + index)
    }

    /// Every text run with a DOM source, in tree order.
    fn visit_text_runs<'a>(&'a self, f: &mut dyn FnMut(&'a LayoutBox, &'a str)) {
        if let BoxType::Text(ref raw) = self.box_type {
            if !self.text_sources.is_empty() {
                f(self, raw);
            }
        }
        for child in &self.children {
            child.visit_text_runs(f);
        }
    }

    /// The DOM position of the run's character boundary `index`.
    pub fn position_of_run_char(&self, index: usize) -> Option<TextPosition> {
        let src = self
            .text_sources
            .iter()
            .find(|s| index >= s.at && index < s.at + s.len)
            .or_else(|| self.text_sources.iter().rev().find(|s| index >= s.at))
            .or_else(|| self.text_sources.first())?;
        let within = index.saturating_sub(src.at).min(src.len);
        Some(TextPosition { node: src.node, offset: src.skip + within, collapsible: src.collapsible })
    }

    /// The run kept its characters `[from, from + len)` (the builder
    /// stripped the rest): its sources follow.
    pub fn keep_text_sources(&mut self, from: usize, len: usize) {
        self.text_sources = keep_sources(&self.text_sources, from, len);
    }

    /// `next`'s characters were appended to this run, its first `dropped`
    /// of them left out (a collapsed space the two runs shared): its
    /// sources join this run's.
    pub fn append_text_sources(&mut self, next: &[TextSource], at: usize, next_len: usize, dropped: usize) {
        let kept = keep_sources(next, dropped, next_len.saturating_sub(dropped));
        self.text_sources.extend(kept.into_iter().map(|s| TextSource { at: s.at + at, ..s }));
    }
}

fn keep_sources(sources: &[TextSource], from: usize, len: usize) -> Vec<TextSource> {
    let to = from + len;
    sources
        .iter()
        .filter_map(|s| {
            let start = s.at.max(from);
            let end = (s.at + s.len).min(to);
            (start < end).then(|| TextSource {
                at: start - from,
                skip: s.skip + (start - s.at),
                len: end - start,
                ..*s
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collapse_map;

    fn src(node: usize, at: usize, skip: usize, len: usize) -> TextSource {
        TextSource { node, at, skip, len, collapsible: true }
    }

    #[test]
    fn collapse_map_follows_the_builder() {
        // "  a \n b " collapses to " a b ".
        assert_eq!(collapse_map("  a \n b ", true), vec![0, 0, 1, 2, 2, 2, 3, 4, 5]);
        assert_eq!(collapse_map("ab", false), vec![0, 1, 2]);
        assert_eq!(collapse_map("x\u{a0}y", true), vec![0, 1, 2, 3], "nbsp is content");
    }

    #[test]
    fn sources_follow_trims_and_joins() {
        let mut run = LayoutBox::new(BoxType::Text(" ab ".into()), rustkit_css::ComputedStyle::default());
        run.text_sources = vec![src(7, 0, 0, 4)];
        run.keep_text_sources(1, 3);
        assert_eq!(run.text_sources, vec![src(7, 0, 1, 3)]);
        // "ab " + " cd" -> "ab cd": the second run's space is dropped.
        run.append_text_sources(&[src(9, 0, 0, 3)], 3, 3, 1);
        assert_eq!(run.text_sources, vec![src(7, 0, 1, 3), src(9, 3, 1, 2)]);
        assert_eq!(run.position_of_run_char(4), Some(TextPosition { node: 9, offset: 2, collapsible: true }));
        assert_eq!(run.position_of_run_char(5), Some(TextPosition { node: 9, offset: 3, collapsible: true }));
        assert_eq!(run.position_of_run_char(1), Some(TextPosition { node: 7, offset: 2, collapsible: true }));
    }
}
