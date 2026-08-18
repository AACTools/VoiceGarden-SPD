//! SSML handling: strip tags while extracting `<mark name="..."/>`
//! positions.
//!
//! speech-dispatcher inserts `<mark name="__spd_N"/>` tags into the text it
//! sends (used for pause/resume positioning), and clients may send their own
//! SSML. The TTS engines we route to want plain text, so we strip every tag
//! and record where each mark landed in the cleaned text. Mark positions are
//! kept as **byte offsets** — the same coordinate system the engines'
//! boundary callbacks report.

/// A `<mark name="...">` recovered from the input text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsmlMark {
    /// Value of the mark's `name` attribute.
    pub name: String,
    /// Byte offset into the cleaned text where the mark was positioned.
    pub byte_offset: usize,
}

/// Fully-processed incoming message.
#[derive(Debug, Clone, Default)]
pub struct ProcessedText {
    /// Plain text with all tags stripped and whitespace collapsed — used
    /// for word timing and mark positioning.
    pub plain: String,
    /// Recovered `<mark>` entries, byte-offset into `plain`.
    pub marks: Vec<SsmlMark>,
    /// Input with **only** `<mark>` tags removed (all other SSML kept),
    /// when the input contained any tag; `None` for plain text. Engines
    /// that accept SSML (azure/edge/google) speak this verbatim — the
    /// crate detects the `<speak>` prefix and passes it through, and
    /// converts SpeechMarkdown inside `speak()` the same way.
    pub ssml: Option<String>,
}

/// Full processing: plain text + marks for timing, mark-stripped SSML for
/// passthrough-capable engines.
#[must_use]
pub fn process(input: &str) -> ProcessedText {
    let has_tags = input.contains('<');
    if !has_tags {
        return ProcessedText {
            plain: collapse_whitespace(input),
            marks: Vec::new(),
            ssml: None,
        };
    }

    let mut plain = String::with_capacity(input.len());
    let mut ssml = String::with_capacity(input.len());
    let mut marks = Vec::new();

    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let Some((tag_end, tag)) = scan_tag(input, i) else {
                plain.push('<');
                ssml.push('<');
                i += 1;
                continue;
            };
            if let Some(name) = mark_name(tag) {
                // Marks never reach the engine: we own their timing.
                marks.push(SsmlMark {
                    name,
                    byte_offset: plain.len(),
                });
            } else {
                ssml.push_str(tag);
            }
            i = tag_end;
        } else {
            let ch = input[i..].chars().next().expect("char at boundary");
            if ch.is_whitespace() {
                if !plain.ends_with(|c: char| c.is_whitespace()) {
                    plain.push(' ');
                }
                if !ssml.ends_with(|c: char| c.is_whitespace()) {
                    ssml.push(' ');
                }
            } else {
                plain.push(ch);
                ssml.push(ch);
            }
            i += ch.len_utf8();
        }
    }
    ProcessedText {
        plain,
        marks,
        ssml: Some(ssml),
    }
}

/// Collapse whitespace runs to single spaces.
fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_whitespace() {
            if !out.ends_on_space() {
                out.push(' ');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

trait EndsOnSpace {
    fn ends_on_space(&self) -> bool;
}
impl EndsOnSpace for String {
    fn ends_on_space(&self) -> bool {
        self.ends_with(|c: char| c.is_whitespace())
    }
}

/// Strip SSML/XML tags from `input`, collapsing whitespace runs to single
/// spaces, returning the cleaned text plus every `<mark>` found along the
/// way, positioned at its byte offset in the cleaned text.
pub fn strip_ssml_with_marks(input: &str) -> (String, Vec<SsmlMark>) {
    let p = process(input);
    (p.plain, p.marks)
}

/// Plain-text view with tags removed and no mark extraction.
pub fn strip_ssml(input: &str) -> String {
    strip_ssml_with_marks(input).0
}

/// Locate the tag starting at `start` (which must be `<`). Returns
/// `(index_after_tag, tag_text)` where `tag_text` includes the brackets.
fn scan_tag(input: &str, start: usize) -> Option<(usize, &str)> {
    let bytes = input.as_bytes();
    let mut i = start + 1;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => match b {
                b'"' | b'\'' => quote = Some(b),
                b'>' => return Some((i + 1, &input[start..i + 1])),
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// If `tag` is a `<mark ...>` element, extract its `name` attribute.
fn mark_name(tag: &str) -> Option<String> {
    let inner = tag
        .strip_prefix('<')
        .and_then(|t| t.strip_suffix('>'))
        .unwrap_or(tag);
    let mut words = inner.split_whitespace();
    let first = words.next()?;
    if !first.eq_ignore_ascii_case("mark") && first != "mark/" {
        return None;
    }
    for attr in words {
        if let Some(value) = attr.strip_prefix("name=") {
            return Some(unquote(value));
        }
    }
    None
}

fn unquote(v: &str) -> String {
    let v = v.trim_end_matches('/');
    if v.len() >= 2 && (v.starts_with('"') || v.starts_with('\'')) {
        let q = v.as_bytes()[0];
        if v.as_bytes()[v.len() - 1] == q {
            return v[1..v.len() - 1].to_string();
        }
    }
    v.to_string()
}

/// SSML namespace required on the `<speak>` envelope by the cloud
/// engines (Edge/Azure silently return zero audio without it).
pub const SSML_XMLNS: &str = "http://www.w3.org/2001/10/synthesis";

/// Upgrade SSML markup to a document the cloud engines accept.
///
/// speech-dispatcher wraps message text in a bare `<speak>` envelope for
/// index marking, and clients send their own SSML of all shapes. The
/// Edge/Azure endpoints require the envelope to carry `version`, `xmlns`
/// and `xml:lang` — a bare `<speak>` completes the turn with **zero
/// audio and no error** (issue #1). This adds any of those attributes
/// that are missing, keeping values already present.
///
/// * Input that already starts with `<speak` → opening tag normalised in
///   place.
/// * Tagged fragment with no envelope and no stray `<` → wrapped in a
///   full `<speak>` document.
/// * Markup with a literal `<` outside any tag (e.g. `"angle < bracket"`)
///   or with no tags at all → returned unchanged (the engine treats it
///   as plain text and escapes it correctly).
#[must_use]
pub fn ensure_ssml_document(markup: &str, lang: &str) -> String {
    let trimmed = markup.trim_start();
    if trimmed.is_empty() {
        return markup.to_string();
    }

    if starts_with_speak_tag(trimmed) {
        let lead = &markup[..markup.len() - trimmed.len()];
        return normalize_envelope(lead, trimmed, lang);
    }

    // No envelope: only safe to wrap when every '<' opens a complete tag
    // (otherwise the text is plain prose with angle brackets, which the
    // engine must see — and escape — verbatim).
    let mut has_tag = false;
    let mut i = 0usize;
    while i < markup.len() {
        if markup.as_bytes()[i] == b'<' {
            match scan_tag(markup, i) {
                Some((end, _)) => {
                    has_tag = true;
                    i = end;
                }
                None => return markup.to_string(), // stray '<' — plain text
            }
        } else {
            i += 1;
        }
    }
    if !has_tag {
        return markup.to_string();
    }
    format!(
        "<speak version='1.0' xmlns='{SSML_XMLNS}' xml:lang='{}'>{markup}</speak>",
        xml_lang_attr_safe(lang)
    )
}

/// True when `s` begins with a `<speak` opening tag (word boundary, any
/// case).
fn starts_with_speak_tag(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.starts_with("<speak")
        && s.len() > 6
        && matches!(s.as_bytes()[6], b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/')
}

/// Rebuild `rest` (which starts with the `<speak` tag) with a complete
/// attribute set. `lead` is preceding whitespace, preserved verbatim.
fn normalize_envelope(lead: &str, rest: &str, lang: &str) -> String {
    let Some((tag_end, tag)) = scan_tag(rest, 0) else {
        return format!("{lead}{rest}"); // unterminated — leave alone
    };
    let inner = tag
        .strip_prefix('<')
        .and_then(|t| t.strip_suffix('>'))
        .unwrap_or("");
    // Attribute names present in the tag (name only, up to `=`).
    let mut has_version = false;
    let mut has_xmlns = false;
    let mut has_lang = false;
    for attr in inner.split_whitespace() {
        let name = attr.split('=').next().unwrap_or("").to_ascii_lowercase();
        match name.as_str() {
            "version" => has_version = true,
            "xmlns" | "xmlns:xmlns" => has_xmlns = true,
            "xml:lang" => has_lang = true,
            _ => {}
        }
    }
    let mut words = inner.split_whitespace();
    // XML tag names are case-sensitive: keep the original spelling so the
    // opening tag still matches `</SPEAK>` etc.
    let first = words.next().unwrap_or("speak");
    let self_closing = first.ends_with('/');
    let name = first.trim_end_matches('/');
    let mut open = String::from("<");
    open.push_str(name);
    // Keep any custom attributes (e.g. a namespace the client declared),
    // dropping a trailing self-closing slash.
    let attrs = words.collect::<Vec<_>>().join(" ");
    let self_closing = self_closing || attrs.ends_with('/');
    let attrs = attrs.trim_end_matches('/');
    if !attrs.is_empty() {
        open.push(' ');
        open.push_str(attrs);
    }
    if !has_version {
        open.push_str(" version='1.0'");
    }
    if !has_xmlns {
        open.push_str(" xmlns='");
        open.push_str(SSML_XMLNS);
        open.push('\'');
    }
    if !has_lang {
        open.push_str(" xml:lang='");
        open.push_str(&xml_lang_attr_safe(lang));
        open.push('\'');
    }
    if self_closing {
        open.push('/');
    }
    open.push('>');
    format!("{lead}{open}{}", &rest[tag_end..])
}

/// Sanitise a BCP-47 tag for single-quoted attribute use (empty → "und").
#[must_use]
pub fn xml_lang_attr_safe(lang: &str) -> String {
    let clean: String = lang
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    if clean.is_empty() {
        "und".to_string()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passthrough() {
        let (text, marks) = strip_ssml_with_marks("Hello world");
        assert_eq!(text, "Hello world");
        assert!(marks.is_empty());
    }

    #[test]
    fn strips_speak_and_prosody() {
        let (text, marks) =
            strip_ssml_with_marks("<speak>Hello <prosody rate=\"fast\">world</prosody></speak>");
        assert_eq!(text, "Hello world");
        assert!(marks.is_empty());
    }

    #[test]
    fn extracts_marks_with_offsets() {
        let (text, marks) =
            strip_ssml_with_marks("One <mark name=\"a\"/> two <mark name=\"b\"/> three");
        assert_eq!(text, "One two three");
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].name, "a");
        assert_eq!(marks[0].byte_offset, text.find("two").unwrap());
        assert_eq!(marks[1].name, "b");
        assert_eq!(marks[1].byte_offset, text.find("three").unwrap());
    }

    #[test]
    fn spd_pause_marks() {
        let (text, marks) = strip_ssml_with_marks(
            "Hello <mark name=\"__spd_1\"/> bright <mark name=\"__spd_2\"/> world",
        );
        assert_eq!(text, "Hello bright world");
        assert_eq!(marks[0].name, "__spd_1");
        assert_eq!(marks[1].name, "__spd_2");
    }

    #[test]
    fn self_closing_variants() {
        for tag in [
            "<mark name=\"x\"/>",
            "<mark name='x'/>",
            "<mark  name=\"x\" >",
            "<MARK name=\"x\"/>",
        ] {
            let (_, marks) = strip_ssml_with_marks(&format!("a {tag} b"));
            assert_eq!(marks.len(), 1, "tag {tag}");
            assert_eq!(marks[0].name, "x");
        }
    }

    #[test]
    fn quoted_angle_bracket_inside_tag() {
        let (text, marks) =
            strip_ssml_with_marks("<voice name=\"we>ird\">hi</voice> <mark name=\"m\"/>");
        assert_eq!(text, "hi ");
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].byte_offset, 3);
    }

    #[test]
    fn unterminated_tag_emitted_literally() {
        let (text, marks) = strip_ssml_with_marks("a < b");
        assert_eq!(text, "a < b");
        assert!(marks.is_empty());
    }

    #[test]
    fn multibyte_offsets_are_bytes() {
        let (text, marks) = strip_ssml_with_marks("héllo wörld <mark name=\"x\"/> end");
        assert_eq!(marks[0].byte_offset, text.find("end").unwrap());
    }

    #[test]
    fn ensure_document_upgrades_bare_envelope() {
        // speech-dispatcher's envelope: what the daemon sends for index
        // marking (issue #1).
        let out = ensure_ssml_document("<speak>hello</speak>", "en-GB");
        assert_eq!(
            out,
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-GB'>hello</speak>"
        );
    }

    #[test]
    fn ensure_document_keeps_existing_attrs_adds_missing() {
        let out =
            ensure_ssml_document("<speak version=\"1.1\" xml:lang='fr'>bonjour</speak>", "en");
        assert_eq!(
            out,
            "<speak version=\"1.1\" xml:lang='fr' xmlns='http://www.w3.org/2001/10/synthesis'>bonjour</speak>"
        );
    }

    #[test]
    fn ensure_document_preserves_leading_whitespace_and_case() {
        let out = ensure_ssml_document("  <SPEAK>x</SPEAK>", "de");
        assert!(out.starts_with("  <SPEAK version='1.0'"), "{out}");
        assert!(out.contains("xmlns='http://www.w3.org/2001/10/synthesis'"));
        assert!(out.ends_with(">x</SPEAK>"), "{out}");
    }

    #[test]
    fn ensure_document_wraps_tag_fragment() {
        // speechd-style envelope after mark stripping, or client fragment
        // SSML with no envelope at all.
        let out = ensure_ssml_document("slow <prosody rate='slow'>down</prosody>", "en");
        assert!(out.starts_with("<speak version='1.0'"), "{out}");
        assert!(out.ends_with(">slow <prosody rate='slow'>down</prosody></speak>"));
    }

    #[test]
    fn ensure_document_leaves_plain_text_with_angle_bracket() {
        // A stray '<' outside a tag means plain prose — must not be
        // wrapped (the engine escapes and speaks it verbatim).
        assert_eq!(
            ensure_ssml_document("angle < bracket", "en"),
            "angle < bracket"
        );
    }

    #[test]
    fn ensure_document_leaves_untagged_and_empty_input() {
        assert_eq!(ensure_ssml_document("just words", "en"), "just words");
        assert_eq!(ensure_ssml_document("", "en"), "");
    }

    #[test]
    fn ensure_document_self_closing_speak() {
        let out = ensure_ssml_document("<speak/>", "en");
        assert_eq!(
            out,
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en'/>"
        );
    }

    #[test]
    fn ensure_document_speechd_envelope_with_marks_removed() {
        // End-to-end shape from the module pipeline: process() strips the
        // marks, ensure_ssml_document then upgrades the envelope.
        let p = process("<speak>Repeat <mark name=\"__spd_0\"/> test</speak>");
        let out = ensure_ssml_document(p.ssml.as_deref().unwrap_or_default(), "en-GB");
        assert_eq!(
            out,
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-GB'>Repeat test</speak>"
        );
    }
}
