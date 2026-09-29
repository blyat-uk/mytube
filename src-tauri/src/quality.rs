//! What quality a download is fetched at, and how that becomes yt-dlp flags.
//!
//! Pure: no I/O. One [`Quality`] lives in `Settings` (the default for every
//! download) and, per video, in `videos.download_quality` (a one-off from the
//! card's "Download (custom)…"). Both are snake_case, since `Settings` is.
//!
//! The fields are strings rather than enums on purpose. settings.json is
//! hand-editable, and an enum would turn one typo into a failed parse of the
//! whole block; a string is repaired field by field in [`Quality::sanitize`],
//! so a bad `vcodec` costs the codec preference and nothing else.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Quality {
    /// `video` or `audio`.
    pub mode: String,
    /// 0 is "best"; otherwise one of [`HEIGHTS`]. A ceiling by preference,
    /// not a filter: see [`format_args`].
    pub max_height: u32,
    /// `any`, `av1`, `vp9` or `h264`. A preference.
    pub vcodec: String,
    /// `any`, `prefer60` or `max30`.
    pub fps: String,
    /// `mkv` or `mp4`: what the video and audio streams are merged into.
    pub container: String,
    /// `original` (no conversion), `m4a`, `mp3` or `opus`. Audio mode only.
    pub audio_format: String,
    /// A raw yt-dlp `-f` string. Non-empty replaces the selects above for the
    /// choice of streams; `container` and `audio_format` still apply.
    pub format: String,
}

pub const HEIGHTS: [u32; 6] = [2160, 1440, 1080, 720, 480, 360];
const MODES: [&str; 2] = ["video", "audio"];
const VCODECS: [&str; 4] = ["any", "av1", "vp9", "h264"];
const FPS: [&str; 3] = ["any", "prefer60", "max30"];
const CONTAINERS: [&str; 2] = ["mkv", "mp4"];
const AUDIO_FORMATS: [&str; 4] = ["original", "m4a", "mp3", "opus"];

impl Default for Quality {
    fn default() -> Self {
        Self {
            mode: "video".into(),
            max_height: 0,
            vcodec: "any".into(),
            fps: "any".into(),
            container: "mkv".into(),
            audio_format: "original".into(),
            format: String::new(),
        }
    }
}

fn repair(value: &mut String, allowed: &[&str]) {
    if !allowed.contains(&value.as_str()) {
        *value = allowed[0].to_string();
    }
}

impl Quality {
    /// Repairs every value the UI can never produce to its default, trims the
    /// raw format, and empties one carrying a control character -- a newline
    /// in an argv element is never a format a person meant.
    pub fn sanitize(&mut self) {
        repair(&mut self.mode, &MODES);
        repair(&mut self.vcodec, &VCODECS);
        repair(&mut self.fps, &FPS);
        repair(&mut self.container, &CONTAINERS);
        repair(&mut self.audio_format, &AUDIO_FORMATS);
        if self.max_height != 0 && !HEIGHTS.contains(&self.max_height) {
            self.max_height = 0;
        }
        let trimmed = self.format.trim();
        self.format = if trimmed.chars().any(char::is_control) {
            String::new()
        } else {
            trimmed.to_string()
        };
    }

    pub fn sanitized(mut self) -> Self {
        self.sanitize();
        self
    }

    fn is_audio(&self) -> bool {
        self.mode == "audio"
    }

    fn converts_audio(&self) -> bool {
        self.is_audio() && self.audio_format != "original"
    }
}

/// For `#[serde(deserialize_with)]`: a block of the wrong shape reads as the
/// default rather than failing the file it sits in, and whatever does parse
/// is sanitised.
pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Quality, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<Quality>(v).unwrap_or_default().sanitized())
}

/// [`lenient`] for an optional block: absent or null is `None`, malformed is
/// the default.
pub fn lenient_opt<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Quality>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.filter(|v| !v.is_null())
        .map(|v| serde_json::from_value::<Quality>(v).unwrap_or_default().sanitized()))
}

fn s(x: &str) -> String {
    x.to_string()
}

/// The format-selection flags for `q`. `Quality::default()` gives exactly the
/// argv every download had before this setting existed: `-f bv*+ba/b
/// --merge-output-format mkv`.
///
/// Height, frame rate and codec go through `-S` (sort) rather than `-f`
/// filters, so they are *preferences*: a video with no 1080p or no h264 still
/// downloads, at the nearest thing it has. The one filter is `max30`, and even
/// that falls back to the unfiltered pick.
///
/// `-S` fields the user names outrank everything yt-dlp sorts by itself, in
/// the order given -- so the order below is the priority. Verified 2026-09-29
/// against dQw4w9WgXcQ (yt-dlp 2026.09.27, which offers 144p-2160p in av01, vp9
/// and h264 up to 1080p): with `-f bv*+ba/b`, no `-S` picks 401 (2160p av01);
/// `res:1080` 399 (1080p av01); `res:1080,vcodec:h264` 137 (1080p avc1);
/// `res:1080,vcodec:vp9` 248; `res:1080,vcodec:av01` 399; `res:720,vcodec:h264`
/// 136; `res:2160,vcodec:h264` 313 (2160p vp9 -- height outranks codec, and
/// there is no 2160p h264). `vcodec:h264` alone picks 137, *not* the 2160p:
/// with no height set the codec is the top priority, which is what "prefer
/// h264" is asked to mean. Audio was 774 (opus) throughout.
pub fn format_args(q: &Quality) -> Vec<String> {
    let selector = if !q.format.is_empty() {
        q.format.clone()
    } else if q.is_audio() {
        s("ba/b")
    } else if q.fps == "max30" {
        s("bv*[fps<=30]+ba/b[fps<=30]/bv*+ba/b")
    } else {
        s("bv*+ba/b")
    };
    let mut a = vec![s("-f"), selector];

    if q.is_audio() {
        if q.converts_audio() {
            a.extend([s("-x"), s("--audio-format"), q.audio_format.clone()]);
        }
        return a;
    }

    // A raw format overrides the selects -- the form disables them while it
    // is set -- so their sorts are left out too, rather than quietly
    // reordering whatever the raw string lets yt-dlp choose between.
    if q.format.is_empty() {
        let mut sort: Vec<String> = Vec::new();
        if q.max_height != 0 {
            sort.push(format!("res:{}", q.max_height));
        }
        if q.fps == "prefer60" {
            sort.push(s("fps"));
        }
        match q.vcodec.as_str() {
            "av1" => sort.push(s("vcodec:av01")),
            "vp9" => sort.push(s("vcodec:vp9")),
            "h264" => sort.push(s("vcodec:h264")),
            _ => {}
        }
        if !sort.is_empty() {
            a.extend([s("-S"), sort.join(",")]);
        }
    }
    a.extend([s("--merge-output-format"), container(q).to_string()]);
    a
}

fn container(q: &Quality) -> &'static str {
    if q.container == "mp4" { "mp4" } else { "mkv" }
}

/// The extension the finished file will have, when it is not the one the
/// probe's `--print filename` reports -- or `None` to keep the probe's.
///
/// The probe prints the name *before* post-processing. Verified 2026-09-29:
/// with `-x --audio-format mp3` it prints `.m4a`, and the file ends up `.mp3`.
/// For a merge it already prints the merged extension (`.mkv` with
/// `--merge-output-format mkv`, `.mp4` with `mp4` -- checked on jNQXAC9IVRw and
/// dQw4w9WgXcQ), so for video this is normally a no-op; it matters when the
/// pick falls back to one pre-muxed stream, which is never remuxed and keeps
/// its own extension (the real path is read back from yt-dlp either way).
///
/// A raw format with no `+` picks one stream, which is neither merged nor
/// converted, so its extension is whatever that stream is.
///
/// Also the switch for `--embed-thumbnail`: `Some` means a container yt-dlp can
/// embed into (mkv, mp4, m4a, mp3, opus). `None` may well be webm, and embedding
/// into webm fails the whole download -- see `ytdlp::download_args`.
pub fn final_ext(q: &Quality) -> Option<&'static str> {
    if q.is_audio() {
        return match q.audio_format.as_str() {
            "m4a" => Some("m4a"),
            "mp3" => Some("mp3"),
            "opus" => Some("opus"),
            _ => None,
        };
    }
    if q.format.is_empty() || q.format.contains('+') {
        Some(container(q))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(f: impl FnOnce(&mut Quality)) -> Quality {
        let mut q = Quality::default();
        f(&mut q);
        q
    }

    fn argv(q: &Quality) -> Vec<String> {
        format_args(q)
    }

    fn value_of(a: &[String], flag: &str) -> Option<String> {
        a.iter().position(|x| x == flag).map(|i| a[i + 1].clone())
    }

    /// Pinned: the default must never change what an existing user downloads.
    #[test]
    fn the_default_is_exactly_the_argv_before_quality_existed() {
        assert_eq!(argv(&Quality::default()),
                   vec!["-f", "bv*+ba/b", "--merge-output-format", "mkv"]);
        assert_eq!(final_ext(&Quality::default()), Some("mkv"));
    }

    #[test]
    fn a_height_is_a_sort_preference_not_a_filter() {
        let a = argv(&q(|q| q.max_height = 1080));
        assert_eq!(value_of(&a, "-f").as_deref(), Some("bv*+ba/b"));
        assert_eq!(value_of(&a, "-S").as_deref(), Some("res:1080"));
    }

    #[test]
    fn each_codec_maps_to_yt_dlps_name_for_it() {
        for (ours, theirs) in [("av1", "vcodec:av01"), ("vp9", "vcodec:vp9"), ("h264", "vcodec:h264")] {
            let a = argv(&q(|q| q.vcodec = ours.into()));
            assert_eq!(value_of(&a, "-S").as_deref(), Some(theirs), "{ours}");
        }
        assert_eq!(value_of(&argv(&q(|q| q.vcodec = "any".into())), "-S"), None);
    }

    #[test]
    fn sort_fields_come_in_priority_order_height_then_fps_then_codec() {
        let a = argv(&q(|q| {
            q.max_height = 720;
            q.fps = "prefer60".into();
            q.vcodec = "h264".into();
        }));
        assert_eq!(value_of(&a, "-S").as_deref(), Some("res:720,fps,vcodec:h264"));
    }

    #[test]
    fn max30_filters_with_an_unfiltered_fallback() {
        let a = argv(&q(|q| q.fps = "max30".into()));
        assert_eq!(value_of(&a, "-f").as_deref(), Some("bv*[fps<=30]+ba/b[fps<=30]/bv*+ba/b"));
        assert_eq!(value_of(&a, "-S"), None, "max30 is a filter, not a sort");
    }

    #[test]
    fn the_container_is_the_merge_format() {
        let a = argv(&q(|q| q.container = "mp4".into()));
        assert_eq!(value_of(&a, "--merge-output-format").as_deref(), Some("mp4"));
        assert_eq!(final_ext(&q(|q| q.container = "mp4".into())), Some("mp4"));
    }

    #[test]
    fn a_raw_format_replaces_the_selector_but_keeps_the_container() {
        let a = argv(&q(|q| {
            q.format = "399+251".into();
            q.max_height = 720;
            q.fps = "max30".into();
        }));
        assert_eq!(value_of(&a, "-f").as_deref(), Some("399+251"));
        assert_eq!(value_of(&a, "--merge-output-format").as_deref(), Some("mkv"));
        assert_eq!(value_of(&a, "-S"), None, "the selects a raw format overrides add nothing");
    }

    #[test]
    fn audio_mode_picks_the_best_audio_and_converts_only_when_asked() {
        let orig = argv(&q(|q| q.mode = "audio".into()));
        assert_eq!(orig, vec!["-f", "ba/b"]);
        for f in ["m4a", "mp3", "opus"] {
            let a = argv(&q(|q| {
                q.mode = "audio".into();
                q.audio_format = f.into();
                q.max_height = 1080;
                q.vcodec = "h264".into();
            }));
            assert_eq!(a, vec!["-f", "ba/b", "-x", "--audio-format", f], "{f}");
        }
    }

    #[test]
    fn audio_mode_with_a_raw_format_uses_it() {
        let a = argv(&q(|q| {
            q.mode = "audio".into();
            q.format = "251".into();
            q.audio_format = "mp3".into();
        }));
        assert_eq!(a, vec!["-f", "251", "-x", "--audio-format", "mp3"]);
    }

    #[test]
    fn final_ext_follows_the_merge_or_the_conversion_and_nothing_else() {
        assert_eq!(final_ext(&q(|q| q.format = "137+140".into())), Some("mkv"));
        assert_eq!(final_ext(&q(|q| q.format = "18".into())), None, "one stream, no merge");
        assert_eq!(final_ext(&q(|q| q.format = "bv*+ba/b".into())), Some("mkv"));
        assert_eq!(final_ext(&q(|q| q.mode = "audio".into())), None, "original may be webm");
        for f in ["m4a", "mp3", "opus"] {
            assert_eq!(final_ext(&q(|q| {
                q.mode = "audio".into();
                q.audio_format = f.into();
            })), Some(f));
        }
    }

    #[test]
    fn sanitize_repairs_every_unknown_value_to_its_default() {
        let mut bad = Quality {
            mode: "both".into(),
            max_height: 1234,
            vcodec: "hevc".into(),
            fps: "120".into(),
            container: "avi".into(),
            audio_format: "flac".into(),
            format: "  137+140  ".into(),
        };
        bad.sanitize();
        assert_eq!(bad, Quality { format: "137+140".into(), ..Quality::default() });

        let mut good = q(|q| {
            q.mode = "audio".into();
            q.max_height = 480;
            q.vcodec = "vp9".into();
            q.fps = "prefer60".into();
            q.container = "mp4".into();
            q.audio_format = "opus".into();
        });
        let before = good.clone();
        good.sanitize();
        assert_eq!(good, before, "valid values are left alone");
    }

    #[test]
    fn a_format_with_a_control_character_is_emptied() {
        let mut x = q(|q| q.format = "137\n--exec rm".into());
        x.sanitize();
        assert_eq!(x.format, "");
        let mut tab = q(|q| q.format = "137\t+140".into());
        tab.sanitize();
        assert_eq!(tab.format, "");
    }

    #[test]
    fn missing_keys_take_their_defaults() {
        let x: Quality = serde_json::from_str(r#"{"mode":"audio"}"#).unwrap();
        assert_eq!(x, Quality { mode: "audio".into(), ..Quality::default() });
    }
}
