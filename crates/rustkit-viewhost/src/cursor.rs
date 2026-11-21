//! The CSS `cursor` keyword the page reports (`PageEvent::Cursor`, CH2.E4)
//! as the AppKit cursor the content view shows. Plain data, so the mapping
//! and its tests build on every OS; `macos.rs` sets the cursor.

/// The `NSCursor` class method that returns the cursor for a used CSS
/// `cursor` keyword. AppKit has no public cursor for some keywords
/// (`help`, `wait`, `progress`, `cell`, the diagonal resizes, `zoom-in`,
/// `zoom-out`, `none`): they show the arrow.
pub fn ns_cursor_selector(css: &str) -> &'static str {
    match css {
        "pointer" => "pointingHandCursor",
        "text" => "IBeamCursor",
        "vertical-text" => "IBeamCursorForVerticalLayout",
        "ew-resize" | "col-resize" => "resizeLeftRightCursor",
        "ns-resize" | "row-resize" => "resizeUpDownCursor",
        "e-resize" => "resizeRightCursor",
        "w-resize" => "resizeLeftCursor",
        "n-resize" => "resizeUpCursor",
        "s-resize" => "resizeDownCursor",
        "not-allowed" | "no-drop" => "operationNotAllowedCursor",
        // AppKit has no public four-way move cursor.
        "grab" | "move" | "all-scroll" => "openHandCursor",
        "grabbing" => "closedHandCursor",
        "crosshair" => "crosshairCursor",
        "copy" => "dragCopyCursor",
        "alias" => "dragLinkCursor",
        "context-menu" => "contextualMenuCursor",
        _ => "arrowCursor",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chrome_and_page_keywords_map_to_appkit_cursors() {
        for (css, ns) in [
            ("default", "arrowCursor"),
            ("auto", "arrowCursor"),
            ("pointer", "pointingHandCursor"),
            ("text", "IBeamCursor"),
            ("vertical-text", "IBeamCursorForVerticalLayout"),
            ("ew-resize", "resizeLeftRightCursor"),
            ("col-resize", "resizeLeftRightCursor"),
            ("ns-resize", "resizeUpDownCursor"),
            ("row-resize", "resizeUpDownCursor"),
            ("e-resize", "resizeRightCursor"),
            ("w-resize", "resizeLeftCursor"),
            ("n-resize", "resizeUpCursor"),
            ("s-resize", "resizeDownCursor"),
            ("not-allowed", "operationNotAllowedCursor"),
            ("no-drop", "operationNotAllowedCursor"),
            ("grab", "openHandCursor"),
            ("grabbing", "closedHandCursor"),
            ("move", "openHandCursor"),
            ("all-scroll", "openHandCursor"),
            ("crosshair", "crosshairCursor"),
            ("copy", "dragCopyCursor"),
            ("alias", "dragLinkCursor"),
            ("context-menu", "contextualMenuCursor"),
            ("help", "arrowCursor"),
            ("wait", "arrowCursor"),
            ("nonsense", "arrowCursor"),
        ] {
            assert_eq!(ns_cursor_selector(css), ns, "{css}");
        }
    }
}
