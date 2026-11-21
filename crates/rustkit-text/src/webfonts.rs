//! Web fonts (`@font-face`): the document-scoped face registry.
//!
//! Every font-creation path in this crate and in `rustkit-layout` resolves a
//! family NAME through the platform (Core Text `new_from_name`). A web font is
//! a family name that exists only because the document declared it, so those
//! paths need one extra lookup ahead of the platform: "did the current
//! document register this family?" That lookup lives here.
//!
//! SCOPE, STATED: the registry holds the faces of ONE document at a time —
//! the engine installs a view's partition slice of its (partitioned)
//! `FontLoader` immediately before laying out or painting that view. It is a
//! slot, not a cache: two documents never see each other's faces because the
//! engine swaps the slot per view, and the loader it swaps from is keyed by
//! top-level site. Handing every `create_font` call a partition parameter
//! would have been the pure design; it also touches a dozen call sites across
//! four crates for the same guarantee, which this gives by construction.
//!
//! The slot is PER THREAD (E56). An engine lays out and paints on the one
//! thread that owns it, and the app runs one engine per tab on its own
//! content thread plus the chrome's engine on the window thread. While the
//! slot was process-wide, another thread's install could land between this
//! thread's install and its shaping, and this page's lines were shaped in
//! the fallback face on that frame only: on ebay with nine tabs open the
//! text switched face from frame to frame once the page's faces had arrived.
//!
//! Formats: TrueType/OpenType sfnt (`.ttf`/`.otf`/`.ttc`), WOFF and WOFF2.
//! On macOS `CGFontCreateWithDataProvider` decodes all of them, so this
//! workspace carries no decompressor of its own. What it does carry is
//! [`inspect`]: every face's container header and table directory are checked
//! against the bytes actually received, and the decoded size a WOFF/WOFF2
//! header declares is capped, BEFORE the bytes reach the platform decoder.
//! A face that fails is dropped and counted as rejected.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Largest font file accepted, as received.
pub const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;

/// Largest decoded (sfnt) size a WOFF/WOFF2 directory may add up to or its
/// header may declare. The platform decoder allocates from these numbers.
pub const MAX_DECODED_FONT_BYTES: u64 = 128 * 1024 * 1024;

/// The container a font file is in, told from its bytes, never from a URL
/// extension, a `format()` hint or a Content-Type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontContainer {
    /// A single TrueType/OpenType font.
    Sfnt,
    /// A TrueType collection (`ttcf`).
    Collection,
    Woff,
    Woff2,
}

/// Why [`inspect`] refused a font file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontRejection {
    /// Shorter than its container's fixed header.
    TooShort,
    /// More than [`MAX_FONT_BYTES`] as received.
    TooLarge,
    /// The first four bytes are no font container this crate knows.
    UnknownContainer,
    /// The header or table directory contradicts the bytes received.
    Malformed(&'static str),
    /// Would decode to more than [`MAX_DECODED_FONT_BYTES`].
    DecodedTooLarge,
}

fn be16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// WOFF2 `UIntBase128`: at most five bytes, no leading zero group, no
/// overflow past 32 bits.
fn base128(data: &[u8], pos: &mut usize) -> Option<u32> {
    let mut value: u32 = 0;
    for i in 0..5 {
        let byte = *data.get(*pos)?;
        *pos += 1;
        if i == 0 && byte == 0x80 {
            return None;
        }
        if value & 0xFE00_0000 != 0 {
            return None;
        }
        value = (value << 7) | u32::from(byte & 0x7F);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn padded(len: u32) -> u64 {
    (u64::from(len) + 3) & !3
}

fn inspect_sfnt(data: &[u8]) -> Result<FontContainer, FontRejection> {
    const BAD: FontRejection = FontRejection::Malformed("sfnt table directory");
    let tables = usize::from(be16(data, 4).ok_or(FontRejection::TooShort)?);
    if tables == 0 || data.len() < 12 + 16 * tables {
        return Err(BAD);
    }
    for i in 0..tables {
        let record = 12 + 16 * i;
        let offset = u64::from(be32(data, record + 8).ok_or(BAD)?);
        let length = u64::from(be32(data, record + 12).ok_or(BAD)?);
        if offset + length > data.len() as u64 {
            return Err(FontRejection::Malformed(
                "sfnt table past the end of the file",
            ));
        }
    }
    Ok(FontContainer::Sfnt)
}

fn inspect_woff(data: &[u8]) -> Result<FontContainer, FontRejection> {
    const HEADER: usize = 44;
    const BAD: FontRejection = FontRejection::Malformed("woff table directory");
    if data.len() < HEADER {
        return Err(FontRejection::TooShort);
    }
    if be32(data, 8) != Some(data.len() as u32) {
        return Err(FontRejection::Malformed(
            "woff length field is not the file size",
        ));
    }
    let tables = usize::from(be16(data, 12).ok_or(BAD)?);
    let directory_end = HEADER + 20 * tables;
    if tables == 0 || data.len() < directory_end {
        return Err(BAD);
    }
    let mut decoded = 12 + 16 * tables as u64;
    for i in 0..tables {
        let entry = HEADER + 20 * i;
        let offset = u64::from(be32(data, entry + 4).ok_or(BAD)?);
        let compressed = be32(data, entry + 8).ok_or(BAD)?;
        let original = be32(data, entry + 12).ok_or(BAD)?;
        if offset < directory_end as u64 || offset + u64::from(compressed) > data.len() as u64 {
            return Err(FontRejection::Malformed("woff table outside the file"));
        }
        if compressed > original {
            return Err(FontRejection::Malformed(
                "woff table larger compressed than decoded",
            ));
        }
        decoded += padded(original);
    }
    let declared = u64::from(be32(data, 16).ok_or(BAD)?);
    if decoded.max(declared) > MAX_DECODED_FONT_BYTES {
        return Err(FontRejection::DecodedTooLarge);
    }
    Ok(FontContainer::Woff)
}

fn inspect_woff2(data: &[u8]) -> Result<FontContainer, FontRejection> {
    const HEADER: usize = 48;
    const BAD: FontRejection = FontRejection::Malformed("woff2 table directory");
    if data.len() < HEADER {
        return Err(FontRejection::TooShort);
    }
    if be32(data, 8) != Some(data.len() as u32) {
        return Err(FontRejection::Malformed(
            "woff2 length field is not the file size",
        ));
    }
    let tables = usize::from(be16(data, 12).ok_or(BAD)?);
    if tables == 0 {
        return Err(BAD);
    }
    let compressed = u64::from(be32(data, 20).ok_or(BAD)?);
    let mut decoded = 12 + 16 * tables as u64;
    let mut pos = HEADER;
    for _ in 0..tables {
        let flags = *data.get(pos).ok_or(BAD)?;
        pos += 1;
        let tag = flags & 0x3F;
        if tag == 0x3F {
            // An arbitrary four-byte tag follows.
            pos += 4;
        }
        let original = base128(data, &mut pos).ok_or(BAD)?;
        // `glyf` (10) and `loca` (11) are transformed unless version 3;
        // every other table is transformed only when the version is not 0.
        let version = flags >> 6;
        let transformed = if tag == 10 || tag == 11 {
            version != 3
        } else {
            version != 0
        };
        if transformed {
            base128(data, &mut pos).ok_or(BAD)?;
        }
        decoded += padded(original);
    }
    if pos as u64 + compressed > data.len() as u64 {
        return Err(FontRejection::Malformed(
            "woff2 compressed stream outside the file",
        ));
    }
    let declared = u64::from(be32(data, 16).ok_or(BAD)?);
    if decoded.max(declared) > MAX_DECODED_FONT_BYTES {
        return Err(FontRejection::DecodedTooLarge);
    }
    Ok(FontContainer::Woff2)
}

/// Identify a font file's container and check its header against the bytes
/// received. This reads headers and table directories only: it decompresses
/// nothing and does not validate table contents.
pub fn inspect(data: &[u8]) -> Result<FontContainer, FontRejection> {
    // The shortest header any container has is the sfnt's 12 bytes.
    if data.len() < 12 {
        return Err(FontRejection::TooShort);
    }
    if data.len() > MAX_FONT_BYTES {
        return Err(FontRejection::TooLarge);
    }
    match &data[..4] {
        b"wOFF" => inspect_woff(data),
        b"wOF2" => inspect_woff2(data),
        b"ttcf" => Ok(FontContainer::Collection),
        [0, 1, 0, 0] | b"OTTO" | b"true" | b"typ1" => inspect_sfnt(data),
        _ => Err(FontRejection::UnknownContainer),
    }
}

/// Identity of a face's bytes: equal for the same file however often it is
/// installed, different for different files. Never 0 (0 means "no web face").
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn content_id(data: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish().max(1)
}

/// Bumped for every thread: something process-wide that family-name
/// resolutions depend on changed (the shaped-face table started over).
static EPOCH: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Bumped when THIS thread's installed face set changes.
    static LOCAL_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Changes whenever this thread's installed face set does (an install of a
/// different set, or a clear), or when process-wide face state is reset.
/// Anything that caches a family-name resolution keys it on this, because
/// the same name can resolve to a different face after. Another thread's
/// install does not move it: that thread has a slot of its own.
pub fn generation() -> u64 {
    // Both parts only grow, so the sum changes whenever either does.
    EPOCH.load(Ordering::Acquire) + LOCAL_GENERATION.with(std::cell::Cell::get)
}

/// Invalidate every thread's family-name resolutions.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn bump_generation() {
    EPOCH.fetch_add(1, Ordering::AcqRel);
}

/// Invalidate this thread's family-name resolutions (its slot changed).
#[cfg_attr(not(any(target_os = "macos", target_os = "linux", windows)), allow(dead_code))]
fn bump_local_generation() {
    LOCAL_GENERATION.with(|g| g.set(g.get() + 1));
}

/// One face as the engine hands it over: raw bytes plus the descriptors the
/// `@font-face` rule declared for them.
#[derive(Debug, Clone)]
pub struct WebFontFace {
    pub family: String,
    /// CSS weight (100..900).
    pub weight: u16,
    pub italic: bool,
    pub data: Arc<Vec<u8>>,
}

#[cfg(target_os = "macos")]
mod imp {
    use super::WebFontFace;
    use core_graphics::data_provider::CGDataProvider;
    use core_graphics::font::CGFont;
    use std::cell::RefCell;
    use std::collections::HashMap;

    struct Face {
        weight: u16,
        italic: bool,
        /// `content_id` of the bytes `cgfont` was built from.
        id: u64,
        cgfont: CGFont,
    }

    struct Active {
        /// Identifies WHICH face set is installed so a re-install of the
        /// same set is a no-op rather than a re-parse of every font file.
        tag: String,
        families: HashMap<String, Vec<Face>>,
    }

    thread_local! {
        /// This thread's slot (see the module docs: one per thread, E56).
        /// Dropped with the thread, so a closed tab's faces go with it.
        static SLOT: RefCell<Active> = RefCell::new(Active {
            tag: String::new(),
            families: HashMap::new(),
        });
    }

    fn with_slot<R>(f: impl FnOnce(&mut Active) -> R) -> R {
        SLOT.with(|slot| f(&mut slot.borrow_mut()))
    }

    fn key(family: &str) -> String {
        family.trim().to_ascii_lowercase()
    }

    /// Install `faces` as the active document font set, replacing whatever
    /// was there. Returns how many faces were accepted; a face that fails
    /// `inspect` or that Core Graphics cannot decode is dropped and counted
    /// against that number, never silently kept as a name with no glyphs.
    pub fn install(tag: &str, faces: &[WebFontFace]) -> usize {
        let installed = with_slot(|active| {
            (!tag.is_empty() && active.tag == tag)
                .then(|| active.families.values().map(Vec::len).sum())
        });
        if let Some(count) = installed {
            return count;
        }
        let mut families: HashMap<String, Vec<Face>> = HashMap::new();
        let mut accepted = 0usize;
        for face in faces {
            if super::inspect(&face.data).is_err() {
                continue;
            }
            let provider = CGDataProvider::from_buffer(face.data.clone());
            let Ok(cgfont) = CGFont::from_data_provider(provider) else {
                continue;
            };
            accepted += 1;
            families.entry(key(&face.family)).or_default().push(Face {
                weight: face.weight,
                italic: face.italic,
                id: super::content_id(&face.data),
                cgfont,
            });
        }
        with_slot(|active| {
            active.tag = tag.to_string();
            active.families = families;
        });
        super::bump_local_generation();
        accepted
    }

    /// Drop the active set. After this no family resolves through the registry.
    pub fn clear() {
        let changed = with_slot(|active| {
            let changed = !active.tag.is_empty() || !active.families.is_empty();
            active.tag.clear();
            active.families.clear();
            changed
        });
        if changed {
            super::bump_local_generation();
        }
    }

    /// Does the active document declare `family`? Case-insensitive, as CSS
    /// family matching is.
    pub fn is_installed(family: &str) -> bool {
        with_slot(|active| active.families.contains_key(&key(family)))
    }

    /// CSS Fonts 4 §5.2 reduced to the two axes the loader records: an exact
    /// italic match beats a mismatched one, then the nearest weight.
    fn select<'a>(faces: &'a [Face], weight: u16, italic: bool) -> Option<&'a Face> {
        faces.iter().min_by_key(|f| {
            let style_penalty: u32 = if f.italic == italic { 0 } else { 10_000 };
            style_penalty + (f.weight as i32 - weight as i32).unsigned_abs()
        })
    }

    /// The registered face of `family` closest to the requested style.
    pub fn lookup(family: &str, weight: u16, italic: bool) -> Option<CGFont> {
        with_slot(|active| {
            let faces = active.families.get(&key(family))?;
            select(faces, weight, italic).map(|f| f.cgfont.clone())
        })
    }

    /// Identity of the registered face a CSS family LIST resolves to for this
    /// style: the first family in the list the document registered, or 0
    /// when it registered none of them. Anything that caches per-family
    /// output (glyph bitmaps) keys on this, because a family name alone says
    /// neither whether the face has arrived yet nor which document's file it
    /// is.
    pub fn face_id(family_list: &str, weight: u16, italic: bool) -> u64 {
        with_slot(|active| {
            if active.families.is_empty() {
                return 0;
            }
            for family in family_list.split(',') {
                let family = family.trim().trim_matches('"').trim_matches('\'');
                if let Some(faces) = active.families.get(&key(family)) {
                    return select(faces, weight, italic).map_or(0, |f| f.id);
                }
            }
            0
        })
    }

    /// The `(weight, italic)` descriptors of the face `lookup` would return.
    /// Same selection code, exposed so the rule is testable and debuggable
    /// without comparing font objects.
    pub fn lookup_descriptor(family: &str, weight: u16, italic: bool) -> Option<(u16, bool)> {
        with_slot(|active| {
            let faces = active.families.get(&key(family))?;
            select(faces, weight, italic).map(|f| (f.weight, f.italic))
        })
    }
}

#[cfg(windows)]
mod imp {
    //! The Windows registry: the same per-thread slot and selection as macOS, holding DirectWrite
    //! faces built from the bytes (a WOFF or WOFF2 file is unpacked by DirectWrite first).
    //!
    //! The bytes are served to DirectWrite by this module's own font file loader, keyed by the
    //! content id of the bytes, from a process-wide table that counts the registry faces using
    //! them. A face that leaves its slot (a new set, `clear`, the thread ending) gives its count
    //! back, and the bytes go with the last one; a stream DirectWrite already opened holds its own
    //! reference until DirectWrite releases it. The key is the content id, so two threads' faces
    //! name their files by content, never by an ordinal one thread's loader handed out.
    use super::{FontContainer, WebFontFace};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use windows::core::implement;
    use windows::Win32::Graphics::DirectWrite::{
        DWriteCreateFactory, IDWriteFactory5, IDWriteFontFace, IDWriteFontFile, IDWriteFontFileLoader,
        IDWriteFontFileLoader_Impl, IDWriteFontFileStream, IDWriteFontFileStream_Impl, DWRITE_CONTAINER_TYPE,
        DWRITE_CONTAINER_TYPE_WOFF, DWRITE_CONTAINER_TYPE_WOFF2, DWRITE_FACTORY_TYPE_SHARED,
        DWRITE_FONT_FACE_TYPE, DWRITE_FONT_FILE_TYPE, DWRITE_FONT_SIMULATIONS_NONE,
    };

    /// The bytes registry faces use, by content id, with how many faces use each.
    static BYTES: Mutex<Option<HashMap<u64, (Arc<Vec<u8>>, usize)>>> = Mutex::new(None);

    fn bytes_table<R>(f: impl FnOnce(&mut HashMap<u64, (Arc<Vec<u8>>, usize)>) -> R) -> R {
        let mut table = BYTES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        f(table.get_or_insert_with(HashMap::new))
    }

    fn retain(id: u64, bytes: Vec<u8>) {
        bytes_table(|table| table.entry(id).or_insert_with(|| (Arc::new(bytes), 0)).1 += 1);
    }

    fn release(id: u64) {
        bytes_table(|table| {
            if let Some(entry) = table.get_mut(&id) {
                entry.1 -= 1;
                if entry.1 == 0 {
                    table.remove(&id);
                }
            }
        });
    }

    fn bytes_of(id: u64) -> Option<Arc<Vec<u8>>> {
        bytes_table(|table| table.get(&id).map(|entry| entry.0.clone()))
    }

    /// Font bytes the registry holds for live faces, in bytes and files (for the tests).
    pub(crate) fn held_bytes() -> (usize, usize) {
        bytes_table(|table| (table.values().map(|entry| entry.0.len()).sum(), table.len()))
    }

    /// One font file's bytes, as DirectWrite reads them.
    #[implement(IDWriteFontFileStream)]
    struct Stream {
        bytes: Arc<Vec<u8>>,
    }

    impl IDWriteFontFileStream_Impl for Stream_Impl {
        fn ReadFileFragment(
            &self,
            fragmentstart: *mut *mut core::ffi::c_void,
            fileoffset: u64,
            fragmentsize: u64,
            fragmentcontext: *mut *mut core::ffi::c_void,
        ) -> windows::core::Result<()> {
            let fits = fileoffset.checked_add(fragmentsize).is_some_and(|end| end <= self.bytes.len() as u64);
            if !fits || fragmentstart.is_null() || fragmentcontext.is_null() {
                return Err(windows::Win32::Foundation::E_INVALIDARG.into());
            }
            unsafe {
                *fragmentstart = self.bytes.as_ptr().add(fileoffset as usize) as *mut core::ffi::c_void;
                *fragmentcontext = std::ptr::null_mut();
            }
            Ok(())
        }

        fn ReleaseFileFragment(&self, _fragmentcontext: *mut core::ffi::c_void) {}

        fn GetFileSize(&self) -> windows::core::Result<u64> {
            Ok(self.bytes.len() as u64)
        }

        fn GetLastWriteTime(&self) -> windows::core::Result<u64> {
            Ok(0)
        }
    }

    /// Serves the bytes `BYTES` holds for a content id (the 8-byte key).
    #[implement(IDWriteFontFileLoader)]
    struct Loader;

    impl IDWriteFontFileLoader_Impl for Loader_Impl {
        fn CreateStreamFromKey(
            &self,
            key: *const core::ffi::c_void,
            key_size: u32,
        ) -> windows::core::Result<IDWriteFontFileStream> {
            if key.is_null() || key_size as usize != std::mem::size_of::<u64>() {
                return Err(windows::Win32::Foundation::E_INVALIDARG.into());
            }
            let id = unsafe { std::ptr::read_unaligned(key as *const u64) };
            let bytes = bytes_of(id).ok_or(windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG))?;
            Ok(Stream { bytes }.into())
        }
    }

    struct Face {
        weight: u16,
        italic: bool,
        /// `content_id` of the bytes `face` was built from: its `BYTES` entry.
        id: u64,
        face: IDWriteFontFace,
    }

    impl Drop for Face {
        fn drop(&mut self) {
            release(self.id);
        }
    }

    struct Active {
        tag: String,
        families: HashMap<String, Vec<Face>>,
    }

    /// This thread's factory and its registration of the loader, undone when the thread ends.
    struct Registration {
        factory: IDWriteFactory5,
        loader: IDWriteFontFileLoader,
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            let _ = unsafe { self.factory.UnregisterFontFileLoader(&self.loader) };
            LOADERS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Loaders registered and not yet unregistered (one per thread that installed faces).
    static LOADERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Registered loaders (for the tests).
    pub(crate) fn registered_loaders() -> usize {
        LOADERS.load(std::sync::atomic::Ordering::Relaxed)
    }

    thread_local! {
        // Declared before SLOT: thread-locals drop in reverse order, so the slot's faces give
        // their bytes back before the loader is unregistered.
        static REGISTRATION: Option<Registration> = register();
        static SLOT: RefCell<Active> = RefCell::new(Active { tag: String::new(), families: HashMap::new() });
    }

    fn register() -> Option<Registration> {
        unsafe {
            let factory: IDWriteFactory5 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let loader: IDWriteFontFileLoader = Loader.into();
            factory.RegisterFontFileLoader(&loader).ok()?;
            LOADERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(Registration { factory, loader })
        }
    }

    fn with_slot<R>(f: impl FnOnce(&mut Active) -> R) -> R {
        SLOT.with(|slot| f(&mut slot.borrow_mut()))
    }

    fn key(family: &str) -> String {
        family.trim().to_ascii_lowercase()
    }

    /// The sfnt bytes of a WOFF or WOFF2 file, as DirectWrite unpacks them.
    fn unpacked(factory: &IDWriteFactory5, kind: DWRITE_CONTAINER_TYPE, data: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let stream: IDWriteFontFileStream =
                factory.UnpackFontFile(kind, data.as_ptr().cast(), u32::try_from(data.len()).ok()?).ok()?;
            let size = stream.GetFileSize().ok()?;
            let mut start = std::ptr::null_mut();
            let mut context = std::ptr::null_mut();
            stream.ReadFileFragment(&mut start, 0, size, &mut context).ok()?;
            let bytes = std::slice::from_raw_parts(start as *const u8, usize::try_from(size).ok()?).to_vec();
            stream.ReleaseFileFragment(context);
            Some(bytes)
        }
    }

    /// A DirectWrite face for `data` (sfnt, WOFF or WOFF2) whose bytes `BYTES` holds under `id`,
    /// or `None` when DirectWrite rejects it (and nothing is held).
    fn build(registration: &Registration, container: FontContainer, data: &[u8], id: u64) -> Option<Face> {
        let sfnt = match container {
            FontContainer::Woff => unpacked(&registration.factory, DWRITE_CONTAINER_TYPE_WOFF, data)?,
            FontContainer::Woff2 => unpacked(&registration.factory, DWRITE_CONTAINER_TYPE_WOFF2, data)?,
            FontContainer::Sfnt | FontContainer::Collection => data.to_vec(),
        };
        retain(id, sfnt);
        // Held from here: a `Face` gives it back when dropped, and every early return below
        // builds none, so it gives it back itself.
        let face = unsafe {
            let built = (|| {
                let key = id.to_ne_bytes();
                let file: IDWriteFontFile = registration
                    .factory
                    .CreateCustomFontFileReference(key.as_ptr().cast(), key.len() as u32, &registration.loader)
                    .ok()?;
                let mut supported = windows::core::BOOL(0);
                let mut file_type = DWRITE_FONT_FILE_TYPE::default();
                let mut face_type = DWRITE_FONT_FACE_TYPE::default();
                let mut faces = 0u32;
                file.Analyze(&mut supported, &mut file_type, Some(&mut face_type), &mut faces).ok()?;
                if !supported.as_bool() || faces == 0 {
                    return None;
                }
                registration.factory.CreateFontFace(face_type, &[Some(file)], 0, DWRITE_FONT_SIMULATIONS_NONE).ok()
            })();
            built
        };
        match face {
            Some(face) => Some(Face { weight: 0, italic: false, id, face }),
            None => {
                release(id);
                None
            }
        }
    }

    /// Install `faces` as this thread's document font set (as on macOS). Returns how many faces
    /// were accepted; one that fails `inspect` or that DirectWrite rejects is dropped and counted
    /// against that number.
    pub fn install(tag: &str, faces: &[WebFontFace]) -> usize {
        let installed = with_slot(|active| {
            (!tag.is_empty() && active.tag == tag).then(|| active.families.values().map(Vec::len).sum())
        });
        if let Some(count) = installed {
            return count;
        }
        let mut families: HashMap<String, Vec<Face>> = HashMap::new();
        let mut accepted = 0usize;
        REGISTRATION.with(|registration| {
            let Some(registration) = registration else { return };
            for face in faces {
                let Ok(container) = super::inspect(&face.data) else { continue };
                let id = super::content_id(&face.data);
                let Some(mut built) = build(registration, container, &face.data, id) else { continue };
                built.weight = face.weight;
                built.italic = face.italic;
                accepted += 1;
                families.entry(key(&face.family)).or_default().push(built);
            }
        });
        with_slot(|active| {
            active.tag = tag.to_string();
            active.families = families;
        });
        super::bump_local_generation();
        accepted
    }

    pub fn clear() {
        let changed = with_slot(|active| {
            let changed = !active.tag.is_empty() || !active.families.is_empty();
            active.tag.clear();
            active.families.clear();
            changed
        });
        if changed {
            super::bump_local_generation();
        }
    }

    pub fn is_installed(family: &str) -> bool {
        with_slot(|active| active.families.contains_key(&key(family)))
    }

    fn select<'a>(faces: &'a [Face], weight: u16, italic: bool) -> Option<&'a Face> {
        faces.iter().min_by_key(|f| {
            let style_penalty: u32 = if f.italic == italic { 0 } else { 10_000 };
            style_penalty + (f.weight as i32 - weight as i32).unsigned_abs()
        })
    }

    /// The registered face of `family` closest to the requested style.
    pub fn lookup(family: &str, weight: u16, italic: bool) -> Option<IDWriteFontFace> {
        with_slot(|active| {
            let faces = active.families.get(&key(family))?;
            select(faces, weight, italic).map(|f| f.face.clone())
        })
    }

    /// The registered faces of the first family in a CSS family LIST the document registered.
    fn first_registered<'a>(active: &'a Active, family_list: &str) -> Option<&'a Vec<Face>> {
        family_list
            .split(',')
            .map(|family| family.trim().trim_matches('"').trim_matches('\''))
            .find_map(|family| active.families.get(&key(family)))
    }

    pub fn face_id(family_list: &str, weight: u16, italic: bool) -> u64 {
        with_slot(|active| {
            first_registered(active, family_list).and_then(|faces| select(faces, weight, italic)).map_or(0, |f| f.id)
        })
    }

    /// The face `face_id` names for a CSS family LIST: the same family, the same selection.
    pub fn lookup_list(family_list: &str, weight: u16, italic: bool) -> Option<IDWriteFontFace> {
        with_slot(|active| {
            first_registered(active, family_list).and_then(|faces| select(faces, weight, italic)).map(|f| f.face.clone())
        })
    }

    pub fn lookup_descriptor(family: &str, weight: u16, italic: bool) -> Option<(u16, bool)> {
        with_slot(|active| {
            let faces = active.families.get(&key(family))?;
            select(faces, weight, italic).map(|f| (f.weight, f.italic))
        })
    }
}

/// LINUX (declared divergence): the registry holds the raw bytes and
/// `LinuxTextBackend::get_face` opens them as FreeType memory faces, so the
/// same face measures (layout) and paints (renderer). `inspect` vets the
/// container (size caps, header and table directory) before FreeType sees the
/// bytes, as on macOS; FreeType then validates the face, and one it rejects is
/// dropped and counted, never kept as a name with no glyphs. FreeType reads
/// TrueType, OpenType and WOFF; WOFF2 needs a brotli-enabled FreeType and
/// installs as nothing otherwise.
#[cfg(target_os = "linux")]
mod imp {
    use super::WebFontFace;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;

    struct Face {
        weight: u16,
        italic: bool,
        id: u64,
        data: Arc<Vec<u8>>,
    }

    struct Active {
        tag: String,
        families: HashMap<String, Vec<Face>>,
    }

    // Per thread, as on macOS: a view's content thread lays out and paints with the set it
    // installed, and another thread's install neither replaces it nor moves its generation.
    thread_local! {
        static SLOT: RefCell<Active> = RefCell::new(Active {
            tag: String::new(),
            families: HashMap::new(),
        });
    }

    fn with_slot<R>(f: impl FnOnce(&mut Active) -> R) -> R {
        SLOT.with(|slot| f(&mut slot.borrow_mut()))
    }

    fn key(family: &str) -> String {
        family.trim().to_ascii_lowercase()
    }

    pub fn install(tag: &str, faces: &[WebFontFace]) -> usize {
        let installed = with_slot(|active| {
            (!tag.is_empty() && active.tag == tag)
                .then(|| active.families.values().map(Vec::len).sum())
        });
        if let Some(count) = installed {
            return count;
        }
        let library = freetype::Library::init().ok();
        let mut families: HashMap<String, Vec<Face>> = HashMap::new();
        let mut accepted = 0usize;
        for face in faces {
            if super::inspect(&face.data).is_err() {
                continue;
            }
            // FreeType validates the face (its own copy of the bytes, dropped here); one it
            // rejects (a WOFF2 without brotli, a broken table) is dropped, not kept as a name.
            let parses = library
                .as_ref()
                .map(|lib| lib.new_memory_face((*face.data).clone(), 0).is_ok())
                .unwrap_or(false);
            if !parses {
                continue;
            }
            accepted += 1;
            families.entry(key(&face.family)).or_default().push(Face {
                weight: face.weight,
                italic: face.italic,
                id: super::content_id(&face.data),
                data: face.data.clone(),
            });
        }
        with_slot(|active| {
            active.tag = tag.to_string();
            active.families = families;
        });
        super::bump_local_generation();
        accepted
    }

    pub fn clear() {
        let changed = with_slot(|active| {
            let changed = !active.tag.is_empty() || !active.families.is_empty();
            active.tag.clear();
            active.families.clear();
            changed
        });
        if changed {
            super::bump_local_generation();
        }
    }

    pub fn is_installed(family: &str) -> bool {
        with_slot(|active| active.families.contains_key(&key(family)))
    }

    fn select<'a>(faces: &'a [Face], weight: u16, italic: bool) -> Option<&'a Face> {
        faces.iter().min_by_key(|f| {
            let style_penalty: u32 = if f.italic == italic { 0 } else { 10_000 };
            style_penalty + (f.weight as i32 - weight as i32).unsigned_abs()
        })
    }

    /// The bytes of this thread's registered face of `family` closest to the requested style.
    /// hiwave-linux's `LinuxTextBackend` opens them as a FreeType memory face.
    pub fn lookup_data(family: &str, weight: u16, italic: bool) -> Option<Arc<Vec<u8>>> {
        with_slot(|active| {
            let faces = active.families.get(&key(family))?;
            select(faces, weight, italic).map(|f| f.data.clone())
        })
    }

    /// Identity of the registered face a CSS family LIST resolves to for this
    /// style, or 0 when the document registered none of them.
    pub fn face_id(family_list: &str, weight: u16, italic: bool) -> u64 {
        with_slot(|active| {
            if active.families.is_empty() {
                return 0;
            }
            for family in family_list.split(',') {
                let family = family.trim().trim_matches('"').trim_matches('\'');
                if let Some(faces) = active.families.get(&key(family)) {
                    return select(faces, weight, italic).map_or(0, |f| f.id);
                }
            }
            0
        })
    }

    pub fn lookup_descriptor(family: &str, weight: u16, italic: bool) -> Option<(u16, bool)> {
        with_slot(|active| {
            let faces = active.families.get(&key(family))?;
            select(faces, weight, italic).map(|f| (f.weight, f.italic))
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    use super::WebFontFace;

    /// No platform registry here yet: web fonts install as nothing, and the
    /// count says so instead of pretending.
    pub fn install(_tag: &str, _faces: &[WebFontFace]) -> usize {
        0
    }

    pub fn clear() {}

    pub fn is_installed(_family: &str) -> bool {
        false
    }

    pub fn face_id(_family_list: &str, _weight: u16, _italic: bool) -> u64 {
        0
    }

    pub fn lookup_descriptor(_family: &str, _weight: u16, _italic: bool) -> Option<(u16, bool)> {
        None
    }
}

pub use imp::{face_id, is_installed, lookup_descriptor};

/// A thread's installed face set as it was handed over (tag and faces), so
/// that a frame built on one thread can be drawn on another with the same
/// faces: the app lays a page out on its content thread and draws the
/// committed frame on the window thread, whose slot holds other faces or
/// none. Empty: no web faces.
#[derive(Debug, Clone, Default)]
pub struct FaceSet(Option<Arc<(String, Vec<WebFontFace>)>>);

impl FaceSet {
    pub fn is_empty(&self) -> bool {
        self.0.as_ref().map_or(true, |set| set.1.is_empty())
    }
}

thread_local! {
    /// What this thread installed last, for [`current`].
    static CURRENT: std::cell::RefCell<FaceSet> = std::cell::RefCell::new(FaceSet::default());
}

/// Install `faces` as this thread's document font set, replacing whatever
/// was there. Returns how many faces were accepted; a face that fails
/// `inspect` or that the platform cannot decode is dropped and counted
/// against that number, never silently kept as a name with no glyphs.
/// Installing the set already installed (same non-empty `tag`) is a no-op.
pub fn install(tag: &str, faces: &[WebFontFace]) -> usize {
    let accepted = imp::install(tag, faces);
    CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        let same = current.0.as_ref().is_some_and(|set| !tag.is_empty() && set.0 == tag);
        if !same {
            *current = FaceSet(Some(Arc::new((tag.to_string(), faces.to_vec()))));
        }
    });
    accepted
}

/// Drop this thread's set. After this no family resolves through the registry.
pub fn clear() {
    imp::clear();
    CURRENT.with(|current| *current.borrow_mut() = FaceSet::default());
}

/// The set this thread has installed, to hand to the thread that draws
/// what this one built ([`install_set`]).
pub fn current() -> FaceSet {
    CURRENT.with(|current| current.borrow().clone())
}

/// Install a set another thread handed over ([`current`]); an empty set
/// clears this thread's, so no family resolves to a face another document
/// installed here. Returns how many faces were accepted.
pub fn install_set(set: &FaceSet) -> usize {
    match &set.0 {
        Some(set) => install(&set.0, &set.1),
        None => {
            clear();
            0
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
pub use imp::lookup;
#[cfg(target_os = "linux")]
pub use imp::lookup_data;
#[cfg(windows)]
pub use imp::lookup_list;

#[cfg(test)]
mod container_tests {
    use super::*;

    const AHEM: &[u8] = include_bytes!("../tests/fixtures/Ahem.ttf");
    const AHEM_WOFF: &[u8] = include_bytes!("../tests/fixtures/Ahem.woff");
    const AHEM_WOFF2: &[u8] = include_bytes!("../tests/fixtures/Ahem.woff2");

    fn patched(data: &[u8], at: usize, value: u32) -> Vec<u8> {
        let mut out = data.to_vec();
        out[at..at + 4].copy_from_slice(&value.to_be_bytes());
        out
    }

    #[test]
    fn the_container_is_told_from_the_bytes() {
        assert_eq!(inspect(AHEM), Ok(FontContainer::Sfnt));
        assert_eq!(inspect(AHEM_WOFF), Ok(FontContainer::Woff));
        assert_eq!(inspect(AHEM_WOFF2), Ok(FontContainer::Woff2));
        assert_eq!(inspect(&[0u8; 64]), Err(FontRejection::UnknownContainer));
        assert_eq!(
            inspect(b"<!DOCTYPE html><html>"),
            Err(FontRejection::UnknownContainer)
        );
        assert_eq!(inspect(&AHEM[..11]), Err(FontRejection::TooShort));
    }

    #[test]
    fn a_truncated_file_is_rejected_in_every_container() {
        // The last table of each file now ends past the bytes received.
        for file in [AHEM, AHEM_WOFF, AHEM_WOFF2] {
            let cut = &file[..file.len() - 40];
            assert!(
                matches!(inspect(cut), Err(FontRejection::Malformed(_))),
                "{:?} for a file cut by 40 bytes",
                inspect(cut)
            );
        }
        assert_eq!(inspect(&AHEM_WOFF[..20]), Err(FontRejection::TooShort));
        assert_eq!(inspect(&AHEM_WOFF2[..20]), Err(FontRejection::TooShort));
    }

    #[test]
    fn a_header_that_declares_a_huge_decoded_size_is_rejected() {
        // totalSfntSize sits at byte 16 of both headers.
        for file in [AHEM_WOFF, AHEM_WOFF2] {
            assert_eq!(
                inspect(&patched(file, 16, u32::MAX)),
                Err(FontRejection::DecodedTooLarge)
            );
        }
    }

    #[test]
    fn a_woff_table_that_expands_past_the_cap_is_rejected() {
        // First directory entry's origLength (header 44 + tag, offset,
        // compLength = byte 56). The header's own total is left honest.
        assert_eq!(
            inspect(&patched(AHEM_WOFF, 56, u32::MAX)),
            Err(FontRejection::DecodedTooLarge)
        );
    }

    #[test]
    fn a_woff_table_that_points_outside_the_file_is_rejected() {
        // First directory entry's offset (byte 48).
        assert_eq!(
            inspect(&patched(AHEM_WOFF, 48, AHEM_WOFF.len() as u32)),
            Err(FontRejection::Malformed("woff table outside the file"))
        );
        // Offset 0 would alias the header.
        assert_eq!(
            inspect(&patched(AHEM_WOFF, 48, 0)),
            Err(FontRejection::Malformed("woff table outside the file"))
        );
    }

    #[test]
    fn a_woff2_stream_longer_than_the_file_is_rejected() {
        // totalCompressedSize sits at byte 20.
        assert_eq!(
            inspect(&patched(AHEM_WOFF2, 20, AHEM_WOFF2.len() as u32)),
            Err(FontRejection::Malformed(
                "woff2 compressed stream outside the file"
            ))
        );
    }

    #[test]
    fn an_sfnt_table_past_the_end_is_rejected() {
        // First table record's length (12 + tag, checksum, offset = byte 24).
        assert_eq!(
            inspect(&patched(AHEM, 24, u32::MAX)),
            Err(FontRejection::Malformed(
                "sfnt table past the end of the file"
            ))
        );
    }

    #[test]
    fn an_oversized_file_is_rejected_before_its_header_is_read() {
        let mut big = AHEM.to_vec();
        big.resize(MAX_FONT_BYTES + 1, 0);
        assert_eq!(inspect(&big), Err(FontRejection::TooLarge));
    }

    #[test]
    fn base128_refuses_leading_zeros_overflow_and_overlong_numbers() {
        let read = |bytes: &[u8]| base128(bytes, &mut 0);
        assert_eq!(read(&[0x3F]), Some(63));
        assert_eq!(read(&[0x81, 0x00]), Some(128));
        assert_eq!(read(&[0x8F, 0xFF, 0xFF, 0xFF, 0x7F]), Some(u32::MAX));
        assert_eq!(read(&[0x80, 0x01]), None, "leading zero group");
        assert_eq!(read(&[0x9F, 0xFF, 0xFF, 0xFF, 0x7F]), None, "past 32 bits");
        assert_eq!(
            read(&[0x81, 0x81, 0x81, 0x81, 0x81, 0x01]),
            None,
            "six bytes"
        );
        assert_eq!(read(&[0x81]), None, "ends mid-number");
    }
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) fn slot_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;
    use std::sync::MutexGuard;

    const AHEM: &[u8] = include_bytes!("../tests/fixtures/Ahem.ttf");

    fn ahem_face(family: &str, weight: u16, italic: bool) -> WebFontFace {
        WebFontFace {
            family: family.to_string(),
            weight,
            italic,
            data: Arc::new(AHEM.to_vec()),
        }
    }

    fn slot_guard() -> MutexGuard<'static, ()> {
        slot_test_guard()
    }

    /// Another thread's install neither replaces this thread's faces nor moves its generation:
    /// a tab's content thread keeps laying out with its own set while another tab installs.
    #[test]
    fn another_threads_install_leaves_this_threads_faces_and_generation() {
        let _slot = slot_guard();
        assert_eq!(install("mine", &[ahem_face("WebfontsTestMine", 400, false)]), 1);
        let my_generation = generation();
        let my_id = face_id("WebfontsTestMine", 400, false);
        std::thread::spawn(|| {
            assert_eq!(install("theirs", &[ahem_face("WebfontsTestTheirs", 400, false)]), 1);
            assert!(is_installed("WebfontsTestTheirs"));
            assert!(!is_installed("WebfontsTestMine"), "a thread sees only its own set");
        })
        .join()
        .unwrap();
        assert!(is_installed("WebfontsTestMine"), "this thread's set is still installed");
        assert!(!is_installed("WebfontsTestTheirs"), "the other thread's set did not land here");
        assert_eq!(face_id("WebfontsTestMine", 400, false), my_id);
        assert!(lookup_data("WebfontsTestMine", 400, false).is_some());
        assert_eq!(generation(), my_generation, "another thread's install does not move this one");
        clear();
    }

    #[test]
    fn an_installed_face_resolves_by_family_case_insensitively() {
        let _slot = slot_guard();
        let faces = [ahem_face("WebfontsTestAhem", 400, false)];
        assert_eq!(install("l1", &faces), 1, "FreeType accepts the TrueType Ahem");
        let data = lookup_data("WEBFONTSTESTAHEM", 400, false).expect("registered family resolves");
        assert_eq!(data.len(), AHEM.len());
        assert!(!is_installed("DejaVu Sans"), "system fonts are not web fonts");
    }

    #[test]
    fn garbage_bytes_are_rejected_not_registered() {
        let _slot = slot_guard();
        let junk = WebFontFace {
            family: "WebfontsTestJunk".to_string(),
            weight: 400,
            italic: false,
            data: Arc::new(vec![0u8; 64]),
        };
        assert_eq!(install("l2", &[junk]), 0);
        assert!(!is_installed("WebfontsTestJunk"));
    }

    #[test]
    fn a_face_the_container_check_rejects_never_reaches_freetype() {
        let _slot = slot_guard();
        let woff = include_bytes!("../tests/fixtures/Ahem.woff");
        let mut huge = woff.to_vec();
        huge[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        let mut bad_length = woff.to_vec();
        bad_length[8..12].copy_from_slice(&((woff.len() as u32) + 1).to_be_bytes());
        let cut_ttf = &AHEM[..AHEM.len() - 40];
        for (name, bytes) in [
            ("WebfontsTestHuge", huge.as_slice()),
            ("WebfontsTestBadLength", bad_length.as_slice()),
            ("WebfontsTestCutTtf", cut_ttf),
        ] {
            assert!(inspect(bytes).is_err(), "{name}: premise, inspect rejects it");
            let face = WebFontFace {
                family: name.to_string(),
                weight: 400,
                italic: false,
                data: Arc::new(bytes.to_vec()),
            };
            assert_eq!(install(name, &[face]), 0, "{name}");
            assert!(!is_installed(name), "{name}");
        }
    }

    #[test]
    fn installing_a_different_set_bumps_the_generation() {
        let _slot = slot_guard();
        install("l3a", &[ahem_face("WebfontsTestGen", 400, false)]);
        let before = generation();
        install("l3a", &[ahem_face("WebfontsTestGen", 400, false)]);
        assert_eq!(generation(), before, "re-installing the same tag is a no-op");
        install("l3b", &[ahem_face("WebfontsTestGen", 700, false)]);
        assert!(generation() > before);
        assert_eq!(lookup_descriptor("WebfontsTestGen", 400, false), Some((700, false)));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use core_text::font as ct_font;

    const AHEM: &[u8] = include_bytes!("../tests/fixtures/Ahem.ttf");
    const AHEM_WOFF: &[u8] = include_bytes!("../tests/fixtures/Ahem.woff");
    const AHEM_WOFF2: &[u8] = include_bytes!("../tests/fixtures/Ahem.woff2");

    fn ahem_face(family: &str, weight: u16, italic: bool) -> WebFontFace {
        face_from(family, weight, italic, AHEM)
    }

    fn face_from(family: &str, weight: u16, italic: bool, data: &[u8]) -> WebFontFace {
        WebFontFace {
            family: family.to_string(),
            weight,
            italic,
            data: Arc::new(data.to_vec()),
        }
    }

    // History: the slot was process-wide until E56 made it per thread, so
    // the guard below is now belt and braces (the generation's epoch part
    // is still process-wide). While it was shared:
    // "Re-install before every positive check" was not enough: another
    // test's install() can land BETWEEN this test's install() and its
    // lookup(), replacing the set, and the positive assertion fails — seen
    // as `an_installed_face_resolves_by_family_case_insensitively` going red
    // about one full-suite run in eight on #163/#164 while passing alone and
    // under --test-threads=1. Every test that touches the slot holds this
    // guard for its whole body, so the slot is single-writer per test.
    use std::sync::{Mutex, MutexGuard};

    fn slot_guard() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        // A panicking test must not poison the rest of the suite.
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn an_installed_face_resolves_by_family_case_insensitively() {
        let _slot = slot_guard();
        let faces = [ahem_face("WebfontsTestAhem", 400, false)];
        let n = install("t1", &faces);
        assert_eq!(n, 1, "Core Graphics accepts the TrueType Ahem");
        let ct = {
            install("t1", &faces);
            let cg = lookup("WEBFONTSTESTAHEM", 400, false).expect("registered family resolves");
            ct_font::new_from_CGFont(&cg, 25.0)
        };
        // Not a fallback: the CTFont built from those bytes reports Ahem's
        // own family name.
        assert_eq!(ct.family_name(), "Ahem");
    }

    #[test]
    fn a_family_nobody_declared_does_not_resolve() {
        let _slot = slot_guard();
        install("t2", &[ahem_face("WebfontsTestOther", 400, false)]);
        assert!(lookup("WebfontsTestNeverDeclared", 400, false).is_none());
        assert!(!is_installed("Helvetica"), "system fonts are not web fonts");
    }

    #[test]
    fn garbage_bytes_are_rejected_not_registered() {
        let _slot = slot_guard();
        let junk = WebFontFace {
            family: "WebfontsTestJunk".to_string(),
            weight: 400,
            italic: false,
            data: Arc::new(vec![0u8; 64]),
        };
        let n = install("t3", &[junk]);
        assert_eq!(n, 0);
        assert!(
            !is_installed("WebfontsTestJunk"),
            "a name with no glyphs behind it must not shadow the platform lookup"
        );
    }

    #[test]
    fn an_ahem_square_rasterizes_with_no_partial_coverage_fringe() {
        // Ahem's glyphs are exact em squares. Rasterized at an integer size
        // on an integer origin, every bitmap pixel must be fully inside the
        // square (255) or fully outside (0); any intermediate value is the
        // rasterizer adding ink the outline does not have. n33 measured that
        // ink on the WPT board: a ~30% fringe one column either side of every
        // Ahem square and ~60% on the row above, which is exactly the
        // difference between a reftest PASS and FAIL for every overlap case.
        let _slot = slot_guard();
        let faces = [ahem_face("WebfontsRasterProbe", 400, false)];
        install("t5", &faces);
        let r = crate::macos::GlyphRasterizer::new("WebfontsRasterProbe", 20.0)
            .expect("registered family rasterizes");
        let (bitmap, w, h, advance, bx, by) = r.rasterize_char('X', 0.0).expect("glyph");
        assert_eq!(advance, 20.0, "Ahem advance is exactly 1em");
        let mut partial = Vec::new();
        let mut ink_cols = std::collections::BTreeSet::new();
        let mut ink_rows = std::collections::BTreeSet::new();
        for row in 0..h as usize {
            for col in 0..w as usize {
                let v = bitmap[row * w as usize + col];
                if v != 0 && v != 255 {
                    partial.push((col, row, v));
                }
                if v != 0 {
                    ink_cols.insert(col);
                    ink_rows.insert(row);
                }
            }
        }
        assert_eq!(
            ink_cols.len(),
            20,
            "ink spans {} columns, expected exactly 20 (bitmap {w}x{h}, bearing {bx},{by}); \
             partial pixels: {:?}",
            ink_cols.len(),
            &partial[..partial.len().min(12)]
        );
        assert_eq!(ink_rows.len(), 20, "ink spans {} rows, expected exactly 20", ink_rows.len());
        assert!(
            partial.is_empty(),
            "{} partially-covered pixels in an integer-aligned em square, e.g. {:?} — \
             the rasterizer is dilating the outline",
            partial.len(),
            &partial[..partial.len().min(12)]
        );
    }

    #[test]
    fn woff_and_woff2_faces_install_and_draw_their_own_glyphs() {
        // Ahem's "X" is an exact em square and no fallback font's is, so a
        // 20x20 block of full ink says the glyph came out of THESE bytes:
        // the compressed tables were decoded, not just the header accepted.
        let _slot = slot_guard();
        for (format, bytes) in [("ttf", AHEM), ("woff", AHEM_WOFF), ("woff2", AHEM_WOFF2)] {
            let family = format!("WebfontsFormat{format}");
            let tag = format!("t7-{format}");
            assert_eq!(
                install(&tag, &[face_from(&family, 400, false, bytes)]),
                1,
                "{format} is accepted"
            );
            let cg = lookup(&family, 400, false).expect("registered family resolves");
            assert_eq!(
                ct_font::new_from_CGFont(&cg, 20.0).family_name(),
                "Ahem",
                "{format}"
            );
            let r = crate::macos::GlyphRasterizer::new(&family, 20.0).expect("rasterizer");
            let (bitmap, w, h, advance, _, _) = r.rasterize_char('X', 0.0).expect("glyph");
            assert_eq!(advance, 20.0, "{format}: Ahem advance is exactly 1em");
            let full = bitmap.iter().filter(|&&v| v == 255).count();
            let partial = bitmap.iter().filter(|&&v| v != 0 && v != 255).count();
            assert_eq!(
                (full, partial),
                (400, 0),
                "{format}: ink in a {w}x{h} bitmap"
            );
        }
    }

    #[test]
    fn a_malformed_compressed_face_never_reaches_the_platform_decoder() {
        let _slot = slot_guard();
        let cut = &AHEM_WOFF2[..AHEM_WOFF2.len() - 40];
        let mut huge = AHEM_WOFF.to_vec();
        huge[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        let faces = [
            face_from("WebfontsTestCut", 400, false, cut),
            face_from("WebfontsTestHuge", 400, false, &huge),
        ];
        assert_eq!(install("t8", &faces), 0);
        assert!(!is_installed("WebfontsTestCut"));
        assert!(!is_installed("WebfontsTestHuge"));
    }

    #[test]
    fn a_face_id_names_the_file_not_the_family() {
        let _slot = slot_guard();
        clear();
        assert_eq!(
            face_id("WebfontsTestId", 400, false),
            0,
            "nothing registered"
        );

        install("t9-a", &[face_from("WebfontsTestId", 400, false, AHEM)]);
        let ttf = face_id("WebfontsTestId", 400, false);
        assert_ne!(ttf, 0, "a registered family has an identity");
        assert_eq!(
            face_id("Nowhere Sans, 'webfontstestid', serif", 400, false),
            ttf,
            "found through a family list, quoted, in any case"
        );
        assert_eq!(
            face_id("Helvetica, serif", 400, false),
            0,
            "platform families have none"
        );

        // Another document declares the SAME family name with another file.
        install(
            "t9-b",
            &[face_from("WebfontsTestId", 400, false, AHEM_WOFF)],
        );
        let woff = face_id("WebfontsTestId", 400, false);
        assert_ne!(woff, 0);
        assert_ne!(woff, ttf, "same name, different file: a different face");

        // Back to the first document: the same file is the same face again.
        install("t9-c", &[face_from("WebfontsTestId", 400, false, AHEM)]);
        assert_eq!(face_id("WebfontsTestId", 400, false), ttf);

        clear();
        assert_eq!(face_id("WebfontsTestId", 400, false), 0);
    }

    #[test]
    fn a_face_shorter_than_a_table_directory_is_rejected() {
        let _slot = slot_guard();
        let short = |n: usize| WebFontFace {
            family: "WebfontsTestShort".to_string(),
            weight: 400,
            italic: false,
            data: Arc::new(vec![0u8; n]),
        };
        assert_eq!(install("t6", &[short(0), short(11)]), 0);
        assert!(!is_installed("WebfontsTestShort"));
    }

    #[test]
    fn the_nearest_style_wins_and_italic_outranks_weight() {
        let _slot = slot_guard();
        let faces = [
            ahem_face("WebfontsTestStyled", 400, false),
            ahem_face("WebfontsTestStyled", 700, true),
        ];
        let pick = |w, i| {
            install("t4", &faces);
            lookup_descriptor("WebfontsTestStyled", w, i).expect("family installed")
        };
        assert_eq!(pick(400, false), (400, false));
        assert_eq!(pick(900, false), (400, false), "upright 900 prefers the upright face");
        assert_eq!(pick(400, true), (700, true), "italic 400 prefers the italic face");
    }
}

/// The Windows registry (DirectWrite faces from the bytes): the macOS registry tests' behaviour,
/// read through DirectWrite.
#[cfg(all(test, windows))]
mod windows_registry_tests {
    use super::*;
    use std::sync::Arc;
    use windows::Win32::Graphics::DirectWrite::IDWriteFontFace;

    const AHEM: &[u8] = include_bytes!("../tests/fixtures/Ahem.ttf");
    const AHEM_WOFF: &[u8] = include_bytes!("../tests/fixtures/Ahem.woff");
    const AHEM_WOFF2: &[u8] = include_bytes!("../tests/fixtures/Ahem.woff2");

    fn face_from(family: &str, weight: u16, italic: bool, data: &[u8]) -> WebFontFace {
        WebFontFace { family: family.into(), weight, italic, data: Arc::new(data.to_vec()) }
    }

    /// The registry's byte table and loader count are process-wide: one test at a time.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The glyph `face` maps `c` to (0: the font has no glyph for it).
    fn glyph(face: &IDWriteFontFace, c: char) -> u16 {
        let mut index = [0u16];
        unsafe { face.GetGlyphIndices([c as u32].as_ptr(), 1, index.as_mut_ptr()) }.expect("glyph indices");
        index[0]
    }

    #[test]
    fn an_installed_face_resolves_by_family_case_insensitively() {
        let _serial = serial();
        assert_eq!(install("win-a", &[face_from("WinWebfontsAhem", 400, false, AHEM)]), 1);
        let face = lookup("WINWEBFONTSAHEM", 400, false).expect("registered family resolves");
        assert_ne!(glyph(&face, 'X'), 0, "the face is Ahem's, which maps X");
        assert!(is_installed("winwebfontsahem"));
        clear();
        assert!(lookup("WinWebfontsAhem", 400, false).is_none());
    }

    #[test]
    fn a_family_nobody_declared_does_not_resolve() {
        let _serial = serial();
        install("win-b", &[face_from("WinWebfontsAhem", 400, false, AHEM)]);
        assert!(lookup("WinWebfontsNeverDeclared", 400, false).is_none());
        clear();
    }

    #[test]
    fn garbage_bytes_are_rejected_not_registered() {
        let _serial = serial();
        let garbage = vec![0x00, 0x01, 0x00, 0x00, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        assert_eq!(install("win-c", &[face_from("WinWebfontsGarbage", 400, false, &garbage)]), 0);
        assert!(lookup("WinWebfontsGarbage", 400, false).is_none());
        clear();
    }

    #[test]
    fn ttf_woff_and_woff2_faces_install_with_their_own_glyphs() {
        let _serial = serial();
        let faces = [
            face_from("WinWebfontsTtf", 400, false, AHEM),
            face_from("WinWebfontsWoff", 400, false, AHEM_WOFF),
            face_from("WinWebfontsWoff2", 400, false, AHEM_WOFF2),
        ];
        assert_eq!(install("win-d", &faces), 3, "every container decodes");
        for family in ["WinWebfontsTtf", "WinWebfontsWoff", "WinWebfontsWoff2"] {
            let face = lookup(family, 400, false).unwrap_or_else(|| panic!("{family} resolves"));
            assert_ne!(glyph(&face, 'X'), 0, "{family} maps X");
        }
        clear();
    }

    #[test]
    fn a_face_id_names_the_file_and_the_nearest_style_wins() {
        let _serial = serial();
        install(
            "win-e",
            &[
                face_from("WinWebfontsStyled", 400, false, AHEM),
                face_from("WinWebfontsStyled", 700, false, AHEM_WOFF),
                face_from("WinWebfontsStyled", 400, true, AHEM_WOFF2),
            ],
        );
        assert_eq!(lookup_descriptor("WinWebfontsStyled", 650, false), Some((700, false)));
        assert_eq!(lookup_descriptor("WinWebfontsStyled", 700, true), Some((400, true)), "italic outranks weight");
        let regular = face_id("'WinWebfontsStyled', serif", 400, false);
        let bold = face_id("WinWebfontsStyled", 700, false);
        assert!(regular != 0 && bold != 0 && regular != bold, "{regular} {bold}");
        assert_eq!(face_id("serif", 400, false), 0);
        clear();
    }

    /// #1343 R1 item 1: font bytes go with the faces. Installing set after set, on threads that end
    /// and on one thread, leaves no bytes and no loader behind once the faces are gone.
    #[test]
    fn installs_and_teardowns_leave_no_bytes_or_loaders_behind() {
        let _serial = serial();
        clear();
        let base = imp::held_bytes();
        for round in 0..20 {
            std::thread::spawn(move || {
                let faces = [
                    face_from("WinWebfontsRoundTtf", 400, false, AHEM),
                    face_from("WinWebfontsRoundWoff2", 400, false, AHEM_WOFF2),
                ];
                assert_eq!(install(&format!("round-{round}"), &faces), 2);
                assert!(imp::held_bytes().0 > base.0, "the set's bytes are held while it is installed");
                assert!(imp::registered_loaders() >= 1);
            })
            .join()
            .expect("round thread");
        }
        assert_eq!(imp::held_bytes(), base, "20 threads' sets are gone with their threads");
        assert_eq!(imp::registered_loaders(), 0, "each thread's loader is unregistered as it ends");

        for round in 0..20 {
            let data = if round % 2 == 0 { AHEM } else { AHEM_WOFF };
            assert_eq!(install(&format!("same-thread-{round}"), &[face_from("WinWebfontsSame", 400, false, data)]), 1);
            assert_eq!(imp::held_bytes().1, base.1 + 1, "one set's file is held, not every set's so far");
        }
        clear();
        assert_eq!(imp::held_bytes(), base, "clear gives the last set's bytes back");
    }

    /// #1343 R1 item 2: two views on two threads that install different font files never share a
    /// face id, so the process-wide face table cannot paint one view's runs with the other's font.
    #[test]
    fn two_threads_web_faces_get_different_face_ids() {
        let _serial = serial();
        let interned = |tag: &'static str, data: &'static [u8]| {
            std::thread::spawn(move || {
                assert_eq!(install(tag, &[face_from("WinWebfontsView", 400, false, data)]), 1);
                let face = crate::web_font_face("WinWebfontsView", 400, false).expect("the view's face");
                crate::intern_face(&face)
            })
            .join()
            .expect("view thread")
        };
        let first = interned("view-a", AHEM);
        let second = interned("view-b", AHEM_WOFF2);
        assert_ne!(first, second, "two views' different fonts share a face id");
    }
}
