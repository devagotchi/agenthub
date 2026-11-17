//! A comment that opens with a marker word and a colon: `// TODO: text`, `# FIXME(ann): text`.

use super::clip;

/// The text after `MARKER:` or `MARKER(who):` when the marker opens a comment on this line.
pub(super) fn read(line: &str, markers: &[String]) -> Option<String> {
    // What a comment starts with, in the languages the library reads.
    const OPENERS: &[&str] = &["//", "//!", "#", "/*", "*", "--", "<!--", ";", "%"];
    for word in markers {
        let mut from = 0;
        while let Some(at) = line[from..].find(word.as_str()) {
            let at = from + at;
            from = at + word.len();
            let before = line[..at].trim_end();
            if !OPENERS.iter().any(|opener| before.ends_with(opener)) {
                continue;
            }
            let mut rest = &line[from..];
            if let Some(inner) = rest.strip_prefix('(') {
                rest = &inner[inner.find(')')? + 1..];
            }
            let Some(text) = rest.strip_prefix(':') else {
                continue;
            };
            let text = text
                .trim()
                .trim_end_matches("*/")
                .trim_end_matches("-->")
                .trim();
            if !text.is_empty() {
                return Some(clip(text));
            }
        }
    }
    None
}
