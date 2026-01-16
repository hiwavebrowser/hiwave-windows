# CSS Engine Port - Detailed Progress Tracker

**Started**: January 16, 2026
**Status**: In Progress
**Goal**: Port complete CSS parsing infrastructure from macOS to Windows (4,000+ lines)

---

## Layout Algorithm Integration - VERIFIED ✅

**GOOD NEWS**: The layout algorithms ARE properly integrated and ready to use!

**Verification Results:**
```rust
// rustkit-layout/src/lib.rs lines 443-454
if self.style.display.is_flex() {
    flex::layout_flex_container(...)  // ✅ Grid algorithm IS called
} else if self.style.display.is_grid() {
    grid::layout_grid_container(...)  // ✅ Flex algorithm IS called
}

// rustkit-layout/src/lib.rs line 754
Position::Sticky => { ... }  // ✅ Sticky positioning IS handled
```

**The Problem**: compute_style_for_element() never sets these values because CSS parsing is missing.

**What Happens Currently:**
1. Engine calls layout() on all LayoutBox nodes ✅
2. layout() checks style.display for Grid/Flex ✅
3. layout() checks style.position for Sticky ✅
4. BUT style.display is always Block (default) ❌
5. So advanced layouts never trigger ❌

**Conclusion**: Once we add CSS parsing, Grid/Flex/Sticky will work immediately. The plumbing is all there.

---

## CRITICAL: Audit of "Completed" Phases 1-5

### Phase 1: Critical Bug Fixes - INCOMPLETE ⚠️

**What Was Done:**
- ✅ LineHeight enum added to rustkit-css (lines 196-201)
- ✅ Grid bug fixes ported to grid.rs
- ✅ Flexbox stretch fix ported to flex.rs

**What's MISSING:**
- ❌ LineHeight CSS parsing: `line-height: 1.5` not parsed from stylesheets
- ❌ LineHeight inheritance not implemented
- ❌ Grid properties not parsed from stylesheets (grid-template-columns, etc.)
- ❌ Flexbox properties not parsed from stylesheets (flex-direction, align-items, etc.)

**Impact**: Layout algorithms work, but can't be styled via CSS.

---

### Phase 2: Text Rendering - INCOMPLETE ⚠️

**What Was Done:**
- ✅ text-transform logic exists in rustkit-layout/src/text.rs

**What's MISSING:**
- ❌ text-transform CSS property not parsed from stylesheets
- ❌ No way to apply uppercase/lowercase/capitalize via CSS

**Impact**: Transform function exists but unreachable via CSS.

---

### Phase 3: Sticky Positioning - INCOMPLETE ⚠️

**What Was Done:**
- ✅ Position enum has Sticky variant (rustkit-css/src/lib.rs line 587)

**What's MISSING:**
- ❌ `position: sticky` not parsed from stylesheets
- ❌ Sticky offset properties (top, left, right, bottom) not parsed
- ❌ StickyState infrastructure not added to LayoutBox

**Impact**: Sticky enum exists but completely non-functional.

---

### Phase 4: CSS Grid Phase 7 - INCOMPLETE ⚠️

**What Was Done:**
- ✅ grid.rs replaced with macOS version (4,560 lines)
- ✅ All 78 grid tests passing in rustkit-layout
- ✅ GridTemplate, TrackSize, GridLine enums added to rustkit-css
- ✅ BoxSizing enum added today

**What's MISSING:**
- ❌ grid-template-columns not parsed from stylesheets
- ❌ grid-template-rows not parsed from stylesheets
- ❌ grid-template-areas not parsed from stylesheets
- ❌ grid-column-start/end not parsed from stylesheets
- ❌ grid-row-start/end not parsed from stylesheets
- ❌ grid-gap not parsed from stylesheets
- ❌ grid-auto-flow not parsed from stylesheets
- ❌ justify-items not parsed from stylesheets
- ❌ align-items not parsed from stylesheets
- ❌ justify-content not parsed from stylesheets
- ❌ align-content not parsed from stylesheets

**Impact**: Complete grid layout algorithm ported, but ZERO grid styling works.

---

### Phase 5: HSL/HSLA Colors - INCOMPLETE ⚠️

**What Was Done:**
- ✅ hsl_to_rgb() added to rustkit-css (lines 1147-1188)
- ✅ HSL parsing added to parse_color() (lines 1121-1142)
- ✅ Test added for HSL colors

**What's MISSING:**
- ❌ HSL colors only work in parse_color() helper
- ❌ parse_color() only called from inline style parser
- ❌ HSL colors in stylesheets won't be parsed

**Impact**: HSL parsing exists but only works for inline styles.

---

## Root Cause Analysis

**The Problem**: All migration work assumed CSS property parsing existed. It doesn't.

**Current Windows Engine Capability:**
```rust
// rustkit-engine/src/lib.rs lines 1011-1069
fn apply_inline_style(&self, style: &mut ComputedStyle, style_attr: &str) {
    match property.as_str() {
        "color" => { ... }
        "background-color" | "background" => { ... }
        "font-size" => { ... }
        "font-weight" => { ... }
        "margin" => { ... }
        "padding" => { ... }
        "box-sizing" => { ... }  // Added today
        _ => {}  // EVERYTHING ELSE IGNORED
    }
}
```

**Missing Infrastructure:**
1. `<style>` tag parsing
2. `<link rel="stylesheet">` parsing
3. Selector matching (element, class, ID, attribute, pseudo-class)
4. Combinator support (descendant, child, adjacent, sibling)
5. Cascade and specificity calculation
6. Style inheritance
7. User-agent stylesheet
8. Computed value resolution
9. 90+ CSS properties

---

## CSS Engine Port Plan - Option 1 (COMPLETE)

### Overview
Port 4,000+ lines of CSS infrastructure from macOS to Windows in logical phases.

**Total Estimate**: 2-3 weeks
**Testing Strategy**: Test each phase before moving to next

---

### Phase A: Foundation - Selector Matching (~800 lines)

**Goal**: Enable matching CSS selectors to DOM elements

**Files to Modify:**
- `hiwave-windows/crates/rustkit-engine/src/lib.rs`

**What to Port from macOS:**
- Selector parsing (element, class, ID)
- Attribute selector matching
- Pseudo-class support (:hover, :first-child, :nth-child)
- Combinator support (descendant, >, +, ~)
- Selector specificity calculation

**macOS Source**: Lines ~1500-2300

**Implementation Steps:**
1. Add `Selector` struct and parsing
2. Add `matches_selector()` function
3. Add specificity calculation
4. Add combinator traversal
5. Test with simple selectors

**Test Cases to Create:**
```html
<style>
  div { color: red; }
  .class { color: blue; }
  #id { color: green; }
  div > span { color: purple; }
</style>
```

**Success Criteria:**
- ✅ Element selectors work (div, span, p)
- ✅ Class selectors work (.container, .header)
- ✅ ID selectors work (#main, #footer)
- ✅ Descendant combinators work (div span)
- ✅ Child combinators work (div > span)

---

### Phase B: Style Tag Parsing (~300 lines)

**Goal**: Extract CSS rules from `<style>` tags

**Files to Modify:**
- `hiwave-windows/crates/rustkit-engine/src/lib.rs`

**What to Port from macOS:**
- `<style>` tag detection during DOM traversal
- CSS rule extraction
- Rule storage (selector → declarations)

**macOS Source**: Lines ~800-1100

**Implementation Steps:**
1. Detect `<style>` elements during DOM parse
2. Extract text content from style elements
3. Parse CSS rules (selector { property: value; })
4. Store rules in stylesheet structure
5. Test with embedded styles

**Test Cases to Create:**
```html
<style>
  body { background-color: #fafafa; }
  h1 { font-size: 2em; }
</style>
<body>
  <h1>Test</h1>
</body>
```

**Success Criteria:**
- ✅ `<style>` tags detected
- ✅ CSS rules extracted
- ✅ Rules stored in accessible structure

---

### Phase C: Cascade and Specificity (~400 lines)

**Goal**: Resolve conflicting rules using cascade and specificity

**Files to Modify:**
- `hiwave-windows/crates/rustkit-engine/src/lib.rs`

**What to Port from macOS:**
- Rule matching for each element
- Specificity comparison
- Source order resolution
- Cascade algorithm

**macOS Source**: Lines ~2300-2700

**Implementation Steps:**
1. Collect all matching rules for element
2. Sort by specificity
3. Sort by source order
4. Apply declarations in order
5. Test with conflicting rules

**Test Cases to Create:**
```html
<style>
  div { color: red; }
  .class { color: blue; }  /* More specific */
  #id { color: green; }    /* Most specific */
</style>
<div id="id" class="class">Test</div>
```

**Success Criteria:**
- ✅ Specificity calculated correctly
- ✅ Higher specificity wins
- ✅ Source order breaks ties
- ✅ Inline styles have highest specificity

---

### Phase D: Style Inheritance (~300 lines)

**Goal**: Propagate inheritable properties from parent to child

**Files to Modify:**
- `hiwave-windows/crates/rustkit-engine/src/lib.rs`

**What to Port from macOS:**
- Inheritance logic
- Inheritable property list
- Computed value resolution

**macOS Source**: Lines ~900-1200

**Implementation Steps:**
1. Define inheritable properties (color, font-size, font-family, etc.)
2. Copy values from parent to child during style computation
3. Handle 'inherit' keyword
4. Test inheritance chain

**Test Cases to Create:**
```html
<style>
  body { color: blue; font-size: 16px; }
</style>
<body>
  <div>
    <span>Should be blue 16px</span>
  </div>
</body>
```

**Success Criteria:**
- ✅ color inherits
- ✅ font-size inherits
- ✅ font-family inherits
- ✅ margin does NOT inherit (correctly)

---

### Phase E: Core Property Parsing (~1,500 lines)

**Goal**: Parse 50+ most common CSS properties

**Files to Modify:**
- `hiwave-windows/crates/rustkit-engine/src/lib.rs`

**What to Port from macOS:**
- Property parsing for 50+ properties
- Length parsing (px, em, rem, %, vw, vh)
- Color parsing (hex, rgb, rgba, hsl, hsla, named)
- Keyword parsing

**macOS Source**: Lines ~2700-4200

**Properties to Implement (Priority Order):**

**Box Model (12 properties):**
- display
- position
- width, height
- min-width, min-height, max-width, max-height
- box-sizing
- top, right, bottom, left

**Spacing (8 properties):**
- margin (4 sides)
- padding (4 sides)

**Border (12 properties):**
- border-width (4 sides)
- border-style (4 sides)
- border-color (4 sides)

**Typography (10 properties):**
- font-family
- font-size
- font-weight
- font-style
- line-height
- text-align
- text-decoration
- text-transform
- letter-spacing
- color

**Background (5 properties):**
- background-color
- background-image
- background-size
- background-position
- background-repeat

**Flexbox (8 properties):**
- flex-direction
- flex-wrap
- justify-content
- align-items
- align-content
- flex-grow
- flex-shrink
- flex-basis

**Grid (15 properties):**
- grid-template-columns
- grid-template-rows
- grid-template-areas
- grid-column-start, grid-column-end
- grid-row-start, grid-row-end
- grid-column, grid-row
- grid-gap, column-gap, row-gap
- grid-auto-flow
- justify-items, align-items
- justify-content, align-content

**Implementation Steps:**
1. Start with box model properties
2. Add spacing properties
3. Add typography properties
4. Add background properties
5. Add flexbox properties
6. Add grid properties
7. Test each group before moving on

**Success Criteria:**
- ✅ All 70 properties parse correctly
- ✅ Invalid values ignored gracefully
- ✅ Shorthand properties expand correctly (margin, padding, border)

---

### Phase F: Advanced Selectors (~300 lines)

**Goal**: Support advanced selector features

**Files to Modify:**
- `hiwave-windows/crates/rustkit-engine/src/lib.rs`

**What to Port from macOS:**
- Pseudo-class support (:hover, :active, :focus)
- Pseudo-element support (::before, ::after)
- Attribute selectors ([type="text"])
- :nth-child() / :nth-of-type()

**macOS Source**: Lines ~2000-2300

**Success Criteria:**
- ✅ :hover works
- ✅ :first-child works
- ✅ :nth-child(n) works
- ✅ [attribute] selectors work

---

### Phase G: External Stylesheets (~200 lines)

**Goal**: Load and parse `<link rel="stylesheet">` files

**Files to Modify:**
- `hiwave-windows/crates/rustkit-engine/src/lib.rs`

**What to Port from macOS:**
- `<link>` tag detection
- External file loading
- CSS parsing of external files

**macOS Source**: Lines ~1100-1300

**Success Criteria:**
- ✅ <link rel="stylesheet"> detected
- ✅ CSS files loaded
- ✅ External rules applied

---

### Phase H: User-Agent Stylesheet (~100 lines)

**Goal**: Provide default browser styles

**Files to Create:**
- `hiwave-windows/crates/rustkit-engine/src/ua_stylesheet.rs`

**What to Port from macOS:**
- Default styles for HTML elements
- Form element styles
- Heading styles

**Success Criteria:**
- ✅ h1 larger than p by default
- ✅ Links blue and underlined by default
- ✅ Lists have default margins

---

## Testing Strategy

### Test File Structure
```
hiwave-windows/
  tests/
    css/
      selectors.html           - Element, class, ID selectors
      cascade.html             - Specificity tests
      inheritance.html         - Property inheritance
      box_model.html           - Width, height, margin, padding
      typography.html          - Font properties, text-align
      flexbox.html             - Flex layout with CSS
      grid.html                - Grid layout with CSS
      colors.html              - RGB, HSL, named colors
      line_height.html         - LineHeight enum via CSS
      sticky.html              - Sticky positioning via CSS
      text_transform.html      - Uppercase/lowercase via CSS
```

### Test After Each Phase

**Phase A Complete:**
```bash
cargo test -p rustkit-engine test_selector_matching
cargo run -p hiwave-smoke -- --html-file tests/css/selectors.html
```

**Phase B Complete:**
```bash
cargo test -p rustkit-engine test_style_tag_parsing
cargo run -p hiwave-smoke -- --html-file tests/css/selectors.html
```

**Phase C Complete:**
```bash
cargo test -p rustkit-engine test_cascade
cargo run -p hiwave-smoke -- --html-file tests/css/cascade.html
```

And so on for each phase.

---

## Progress Tracking

### Phase A: Selector Matching
- [ ] Add Selector struct
- [ ] Implement element selector matching
- [ ] Implement class selector matching
- [ ] Implement ID selector matching
- [ ] Implement descendant combinator
- [ ] Implement child combinator (>)
- [ ] Implement adjacent sibling (+)
- [ ] Implement general sibling (~)
- [ ] Add specificity calculation
- [ ] Create test_selector_matching test
- [ ] Test with selectors.html

### Phase B: Style Tag Parsing
- [ ] Detect `<style>` elements in DOM
- [ ] Extract text content
- [ ] Parse CSS rules
- [ ] Store rules in stylesheet
- [ ] Test with embedded styles

### Phase C: Cascade and Specificity
- [ ] Collect matching rules per element
- [ ] Sort by specificity
- [ ] Handle source order
- [ ] Test with conflicting rules

### Phase D: Style Inheritance
- [ ] Define inheritable properties
- [ ] Implement inheritance logic
- [ ] Handle 'inherit' keyword
- [ ] Test inheritance chain

### Phase E: Core Property Parsing
- [ ] Box model properties (12)
- [ ] Spacing properties (8)
- [ ] Border properties (12)
- [ ] Typography properties (10)
- [ ] Background properties (5)
- [ ] Flexbox properties (8)
- [ ] Grid properties (15)

### Phase F: Advanced Selectors
- [ ] Pseudo-classes
- [ ] Pseudo-elements
- [ ] Attribute selectors
- [ ] :nth-child()

### Phase G: External Stylesheets
- [ ] Detect <link> tags
- [ ] Load external files
- [ ] Parse external CSS

### Phase H: User-Agent Stylesheet
- [ ] Create default styles
- [ ] Apply UA styles first

---

## Context Continuity Notes

### If Context Runs Out - Resume From:

**What's Been Done:**
1. Phases 1-5 migration completed but non-functional (CSS parsing missing)
2. Critical gap identified and documented
3. BoxSizing enum added to rustkit-css
4. This progress tracker created
5. Ready to start Phase A: Selector Matching

**Where macOS Source Code Is:**
- File: `hiwave-macos/crates/rustkit-engine/src/lib.rs` (6,215 lines)
- Selector matching: Lines ~1500-2300
- Style tag parsing: Lines ~800-1100
- Cascade: Lines ~2300-2700
- Inheritance: Lines ~900-1200
- Property parsing: Lines ~2700-4200
- Advanced selectors: Lines ~2000-2300
- External stylesheets: Lines ~1100-1300

**Where Windows Target Is:**
- File: `hiwave-windows/crates/rustkit-engine/src/lib.rs` (2,025 lines)
- Current inline parser: Lines 1011-1069
- Need to add everything else

**Next Action:**
Start Phase A - port selector matching from macOS lines 1500-2300

---

## Risk Management

### Risks and Mitigations

**Risk**: Breaking existing inline style functionality
**Mitigation**: Keep inline parser working, add stylesheet parsing alongside

**Risk**: Performance regression with stylesheet parsing
**Mitigation**: Profile before/after, optimize if needed

**Risk**: Missing edge cases in CSS parsing
**Mitigation**: Comprehensive test suite, compare output to macOS

**Risk**: Running out of context during long port
**Mitigation**: This progress tracker, detailed notes, commit after each phase

---

**Last Updated**: January 16, 2026 - Initial planning complete, ready to start Phase A
