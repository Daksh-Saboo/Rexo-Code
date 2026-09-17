//! A tiny, dependency-free `---\nkey: value\n---\nbody` front-matter
//! parser. Deliberately not full YAML — no nesting, no block scalars —
//! just flat `key: value` lines, which is all [`super::skills`] and
//! [`super::custom_commands`] need. Pulling in a real YAML crate for this
//! would mean fighting the edition2024/MSRV dependency wall in this
//! project's build environment for a feature that only ever needs a
//! handful of one-line string/list fields.

use std::collections::BTreeMap;

pub struct Frontmatter {
    /// Lower-cased keys, trimmed values.
    pub fields: BTreeMap<String, String>,
    pub body: String,
}

impl Frontmatter {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    /// A comma-separated field split into trimmed, lower-cased, non-empty
    /// parts — used for `triggers:`.
    pub fn list(&self, key: &str) -> Vec<String> {
        self.get(key)
            .map(|v| v.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default()
    }
}

/// Parse `raw` as `---\n...\n---\n<body>`. If it doesn't start with a
/// `---` frontmatter block at all, every field is empty and `body` is the
/// whole input unchanged — a plain markdown file with no frontmatter is a
/// valid skill/command, not an error.
pub fn parse(raw: &str) -> Frontmatter {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw); // defensive BOM strip
    let normalized = raw.replace("\r\n", "\n");
    if let Some(rest) = normalized.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let header = &rest[..end];
            let after = &rest[end + 4..];
            let body = after.strip_prefix('\n').unwrap_or(after);
            let mut fields = BTreeMap::new();
            for line in header.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    fields.insert(k.trim().to_lowercase(), v.trim().to_string());
                }
            }
            return Frontmatter { fields, body: body.to_string() };
        }
    }
    Frontmatter { fields: BTreeMap::new(), body: normalized }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fields_and_body() {
        let fm = parse("---\nname: foo\ndescription: does a thing\ntriggers: a, B ,c\n---\nBody text here.\n");
        assert_eq!(fm.get("name"), Some("foo"));
        assert_eq!(fm.get("description"), Some("does a thing"));
        assert_eq!(fm.list("triggers"), vec!["a", "b", "c"]);
        assert_eq!(fm.body.trim(), "Body text here.");
    }

    #[test]
    fn no_frontmatter_is_the_whole_body() {
        let fm = parse("Just a plain file.\n");
        assert!(fm.fields.is_empty());
        assert_eq!(fm.body.trim(), "Just a plain file.");
    }

    #[test]
    fn missing_closing_delimiter_falls_back_to_whole_body() {
        let fm = parse("---\nname: foo\nno closing delimiter here\n");
        assert!(fm.fields.is_empty());
    }
}
