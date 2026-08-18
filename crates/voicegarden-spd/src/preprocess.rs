//! Accessibility text preprocessing: punctuation announcement, spelling,
//! and capital-letter recognition.
//!
//! speech-dispatcher forwards these settings to modules; engines don't
//! implement them, so the module expands the text before synthesis
//! (the same approach espeak's module takes via espeak's own modes —
//! we do it in plain text instead).

/// Punctuation announcement mode (SSIP `punctuation_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PunctMode {
    #[default]
    None,
    Some,
    Most,
    All,
}

impl PunctMode {
    #[must_use]
    pub fn parse(v: &str) -> Option<Self> {
        match v {
            "none" => Some(Self::None),
            "some" => Some(Self::Some),
            "most" => Some(Self::Most),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

/// Capital-letter recognition mode (SSIP `cap_let_recogn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapMode {
    #[default]
    None,
    Spell,
    Icon,
}

impl CapMode {
    #[must_use]
    pub fn parse(v: &str) -> Option<Self> {
        match v {
            "none" => Some(Self::None),
            "spell" => Some(Self::Spell),
            "icon" => Some(Self::Icon),
            _ => None,
        }
    }
}

/// All preprocessing switches in one place.
#[derive(Debug, Clone, Copy, Default)]
pub struct Preprocess {
    pub punctuation: PunctMode,
    pub spelling: bool,
    pub capitals: CapMode,
}

impl Preprocess {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.punctuation != PunctMode::None || self.spelling || self.capitals != CapMode::None
    }
}

/// Expand punctuation marks into spoken words per the mode. Word-adjacent
/// punctuation (don't, 3.14) is left alone — only boundary/standalone
/// punctuation is announced, matching screen-reader conventions.
#[must_use]
pub fn expand_punctuation(text: &str, mode: PunctMode) -> String {
    if mode == PunctMode::None || text.is_empty() {
        return text.to_string();
    }
    let announce = |c: char| -> Option<&'static str> {
        let set = match mode {
            PunctMode::None => return None,
            PunctMode::Some => matches!(c, '.' | '?' | '!'),
            PunctMode::Most => matches!(c, '.' | '?' | '!' | ',' | ';' | ':'),
            PunctMode::All => matches!(
                c,
                '.' | '?'
                    | '!'
                    | ','
                    | ';'
                    | ':'
                    | '-'
                    | '—'
                    | '('
                    | ')'
                    | '"'
                    | '/'
                    | '@'
                    | '#'
                    | '*'
                    | '+'
                    | '='
                    | '_'
            ),
        };
        if !set {
            return None;
        }
        Some(match c {
            '.' => "period",
            '?' => "question mark",
            '!' => "exclamation mark",
            ',' => "comma",
            ';' => "semicolon",
            ':' => "colon",
            '-' => "dash",
            '—' => "em dash",
            '(' => "open paren",
            ')' => "close paren",
            '"' => "quote",
            '/' => "slash",
            '@' => "at",
            '#' => "hash",
            '*' => "star",
            '+' => "plus",
            '=' => "equals",
            _ => "underscore",
        })
    };

    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 32);
    for (i, &c) in chars.iter().enumerate() {
        let word_before = i > 0 && chars[i - 1].is_alphanumeric();
        let word_after = i + 1 < chars.len() && chars[i + 1].is_alphanumeric();
        let at_start = i == 0 || chars[i - 1].is_whitespace();
        let at_end = i + 1 >= chars.len() || chars[i + 1].is_whitespace();

        // Which positions get announced:
        //  - standalone: " - " surrounded by space/edges
        //  - trailing: "word."  — attached after a word, at end/space
        //  - leading:  "(word"  — attached before a word, at start/space
        //  - embedded: "don't", "3.14", "1,000" — never announced
        let standalone = !word_before && !word_after;
        let trailing = word_before && at_end;
        let leading = word_after && at_start;
        if standalone || trailing || leading {
            if let Some(word) = announce(c) {
                out.push_str(&format!(" {word} "));
                continue;
            }
        }
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Spell out each word: letters separated by commas so engines pause
/// between them ("hello" → "h, e, l, l, o").
#[must_use]
pub fn expand_spelling(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 4);
    for word in text.split_whitespace() {
        let mut first = true;
        for ch in word.chars() {
            if ch.is_alphanumeric() {
                if !first {
                    out.push_str(", ");
                }
                match ch {
                    '*' => out.push_str("star"),
                    _ => out.push(ch),
                }
                first = false;
            }
        }
        out.push(' ');
    }
    out.trim_end().to_string()
}

/// Prefix capitalized words with "capital" so they're distinguished
/// audibly (screen-reader convention). Single letters are spelled loudly
/// anyway; only words with an uppercase initial get the prefix.
#[must_use]
pub fn expand_capitals(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for word in text.split_whitespace() {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            if first.is_uppercase() && chars.any(char::is_alphanumeric) {
                out.push_str("capital ");
            }
        }
        out.push_str(word);
        out.push(' ');
    }
    out.trim_end().to_string()
}

/// Apply the active expansions. Spelling short-circuits everything else —
/// spelling mode reads item-by-item, so announcing the inserted words
/// ("period" spelled letter-by-letter) would be wrong.
#[must_use]
pub fn apply(text: &str, pp: Preprocess) -> String {
    if pp.spelling {
        return expand_spelling(text);
    }
    let mut t = text.to_string();
    if pp.capitals != CapMode::None {
        t = expand_capitals(&t);
    }
    if pp.punctuation != PunctMode::None {
        t = expand_punctuation(&t, pp.punctuation);
    }
    t
}

/// XML-escape text for embedding in generated SSML.
#[must_use]
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// SSML spelling for engines with native support (Azure, Edge, Google):
/// each word wrapped in `<say-as interpret-as="characters">`, which the
/// engine spells with its own prosody — better than our comma-separated
/// letter approximation. Only used when the voice is SSML-capable and the
/// client didn't send SSML of its own.
///
/// The envelope carries `version`/`xmlns`/`xml:lang` — Edge/Azure return
/// zero audio for a bare `<speak>` (issue #1).
#[must_use]
pub fn spelling_ssml(text: &str, lang: &str) -> String {
    use crate::ssml::{xml_lang_attr_safe, SSML_XMLNS};
    let mut out = format!(
        "<speak version='1.0' xmlns='{SSML_XMLNS}' xml:lang='{}'>",
        xml_lang_attr_safe(lang)
    );
    for word in text.split_whitespace() {
        out.push_str("<say-as interpret-as=\"characters\">");
        out.push_str(&xml_escape(word));
        out.push_str("</say-as> ");
    }
    if out.ends_with(' ') {
        out.pop(); // trailing space
    }
    out.push_str("</speak>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_some_expands_sentence_enders() {
        assert_eq!(
            expand_punctuation("Hello world. Really?", PunctMode::Some),
            "Hello world period Really question mark"
        );
    }

    #[test]
    fn punctuation_none_is_passthrough() {
        assert_eq!(expand_punctuation("a, b.", PunctMode::None), "a, b.");
    }

    #[test]
    fn punctuation_word_adjacent_untouched() {
        // "3.14" and "don't" keep their punctuation
        assert_eq!(
            expand_punctuation("It's 3.14, ok?", PunctMode::Most),
            "It's 3.14 comma ok question mark"
        );
    }

    #[test]
    fn punctuation_all_covers_more_marks() {
        assert_eq!(
            expand_punctuation("(yes) - no", PunctMode::All),
            "open paren yes close paren dash no"
        );
    }

    #[test]
    fn spelling_letters_with_pauses() {
        assert_eq!(expand_spelling("hi yo"), "h, i y, o");
    }

    #[test]
    fn capitals_prefixed() {
        assert_eq!(
            expand_capitals("the Big small OK"),
            "the capital Big small capital OK"
        );
    }

    #[test]
    fn capitals_single_letters_untouched() {
        // single uppercase letters are their own spelling; no prefix spam
        assert_eq!(expand_capitals("A B c"), "A B c");
    }

    #[test]
    fn apply_spelling_short_circuits() {
        let pp = Preprocess {
            punctuation: PunctMode::Some,
            spelling: true,
            capitals: CapMode::Spell,
        };
        // Spelling wins: no "capital" prefix, no "period" word.
        assert_eq!(apply("Go. now", pp), "G, o n, o, w");
    }

    #[test]
    fn apply_capitals_then_punctuation() {
        let pp = Preprocess {
            punctuation: PunctMode::Some,
            spelling: false,
            capitals: CapMode::Spell,
        };
        let out = apply("Go. now", pp);
        assert!(out.starts_with("capital Go"), "capital prefix: {out}");
        assert!(out.contains("period"), "period announced: {out}");
    }

    #[test]
    fn modes_parse() {
        assert_eq!(PunctMode::parse("some"), Some(PunctMode::Some));
        assert_eq!(PunctMode::parse("bogus"), None);
        assert_eq!(CapMode::parse("icon"), Some(CapMode::Icon));
    }

    #[test]
    fn spelling_ssml_wraps_words() {
        assert_eq!(
            spelling_ssml("hi yo", "en"),
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en'>\
             <say-as interpret-as=\"characters\">hi</say-as> \
             <say-as interpret-as=\"characters\">yo</say-as></speak>"
        );
    }

    #[test]
    fn spelling_ssml_escapes_markup() {
        // split_whitespace yields three words: "a<b", "&", "c>" — each
        // must be escaped inside its say-as element.
        let ssml = spelling_ssml("a<b & c>", "en");
        assert!(ssml.contains("a&lt;b"), "escaped <: {ssml}");
        assert!(ssml.contains("&amp;"), "escaped &: {ssml}");
        assert!(ssml.contains("c&gt;"), "escaped >: {ssml}");
        assert!(!ssml.contains("<b "), "no raw tag injection");
    }

    #[test]
    fn spelling_ssml_empty() {
        assert_eq!(
            spelling_ssml("", "en"),
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en'></speak>"
        );
    }
}
