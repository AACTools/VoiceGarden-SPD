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

/// Strip SSML/XML tags from `input`, collapsing whitespace runs to single
/// spaces, returning the cleaned text plus every `<mark>` found along the
/// way, positioned at its byte offset in the cleaned text.
pub fn strip_ssml_with_marks(input: &str) -> (String, Vec<SsmlMark>) {
    let mut out = String::with_capacity(input.len());
    let mut marks = Vec::new();

    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Find the end of the tag, tolerating quoted '>' characters.
            let Some((tag_end, tag)) = scan_tag(input, i) else {
                // Unterminated '<' — emit it literally.
                out.push('<');
                i += 1;
                continue;
            };
            if let Some(name) = mark_name(tag) {
                marks.push(SsmlMark {
                    name,
                    byte_offset: out.len(),
                });
            }
            i = tag_end;
        } else {
            let ch = input[i..].chars().next().expect("char at boundary");
            if ch.is_whitespace() {
                // Collapse whitespace runs so tag removal doesn't leave
                // double spaces; offsets stay consistent with the output.
                if !out.ends_with(|c: char| c.is_whitespace()) {
                    out.push(' ');
                }
            } else {
                out.push(ch);
            }
            i += ch.len_utf8();
        }
    }
    (out, marks)
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
}
