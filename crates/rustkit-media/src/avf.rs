//! AVFoundation glue for MEDIA-AVF (`docs/design/MEDIA_AVF.md`). Milestone 1 step 1 adds
//! the dependency and the one question the element asks before anything plays:
//! whether AVFoundation can play a MIME type with its codecs. Step 2 answers
//! `canPlayType` with it (`""`, `"maybe"` or `"probably"` per §3.5).

use objc2_av_foundation::AVURLAsset;
use objc2_foundation::NSString;

/// Whether AVFoundation can play `mime`, an extended MIME type (`video/mp4;
/// codecs="avc1.42E01E"`): `+[AVURLAsset isPlayableExtendedMIMEType:]`.
pub fn is_playable_extended_mime_type(mime: &str) -> bool {
    let mime = NSString::from_str(mime);
    unsafe { AVURLAsset::isPlayableExtendedMIMEType(&mime) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codecs of the MEDIA-AVF fixtures play; an unknown type does not.
    #[test]
    fn h264_and_aac_in_mp4_are_playable() {
        assert!(is_playable_extended_mime_type(
            "video/mp4; codecs=\"avc1.42E01E, mp4a.40.2\""
        ));
        assert!(is_playable_extended_mime_type(
            "audio/mp4; codecs=\"mp4a.40.2\""
        ));
        assert!(!is_playable_extended_mime_type("video/x-unknown"));
    }
}
