# Consent-overlay click fixtures

Reduced pages modelled on the cookie-consent dialogs news sites show: a
full-viewport overlay with a high `z-index` over an article, and an
"Agree" button inside it. Each page sets `window.__agree` to the element a
user would click and logs the events its own listeners hear in
`window.__heard`. `consent_click_tests.rs` (rustkit-engine) loads each one,
clicks the centre of `__agree` where it is painted (its layout box under
its ancestors' transforms, computed from the layout tree; not
`getBoundingClientRect()`, which leaves transforms out) through
`mouse_down_at_point` + `click_at_point` (the path the app's window click
takes) and checks the listener ran.

| File | Shape |
|------|-------|
| `plain_dom.html` | (c) plain DOM at the end of `<body>`, flex-centred dialog, listener on the button |
| `translate_centered.html` | (c) dialog centred with `top/left: 50%` and `transform: translate(-50%, -50%)` |
| `backdrop_pointer_events_none.html` | backdrop `pointer-events: none`, dialog `pointer-events: auto` |
| `delegated_on_document.html` | `<div role=button>`, one `click` listener on `document` (delegation) |
| `pointerup_only.html` | the button listens for `pointerup` only |
| `fixed_in_transformed_ancestor.html` | the fixed overlay sits inside a `transform`ed wrapper |
| `zero_size_fixed_wrapper.html` | a 0x0 `position: fixed` container holding the fixed overlay |
| `scrolled_page.html` | the page is scrolled before the dialog is clicked |
| `shadow_open.html` | (b) open shadow root, listener delegated on the shadow root |
| `shadow_closed.html` | (b) closed shadow root, listener on the button |
| `iframe_srcdoc.html` | (a) the buttons inside an `<iframe srcdoc>` |
