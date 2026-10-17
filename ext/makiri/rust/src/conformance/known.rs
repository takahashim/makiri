//! The divergences already classified against the pinned HTML Standard.
//!
//! `spec/conformance/parser_known_divergences.txt` lists them, one per line as
//! `<file>#<index> <id>`, where the id is the entry in
//! `docs/parser_differences.ja.md` that states the spec step deciding it.
//! Both runners read this one file - the Ruby one over the public API, this
//! crate's over each parser - so they cannot disagree about what is known.
//! A listed test that starts passing is a failure in both, so the list cannot
//! outlive the divergence.

use std::collections::BTreeMap;

/// `test key -> table id`.
pub fn load() -> BTreeMap<String, String> {
    let path = super::spec_dir().join("parser_known_divergences.txt");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    parse(&text)
}

pub fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("# ") && *l != "#")
        .map(|l| {
            let mut it = l.split_whitespace();
            let key = it.next().unwrap_or_default().to_string();
            let id = it
                .next()
                .unwrap_or_else(|| panic!("no id on line {l:?}"))
                .to_string();
            (key, id)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_keys_and_ids_past_comments() {
        let m = super::parse("# a comment\n#\ntemplate.dat#44 P1\n\n  tests19.dat#51   P3 \n");
        assert_eq!(m.len(), 2);
        assert_eq!(m["template.dat#44"], "P1");
        assert_eq!(m["tests19.dat#51"], "P3");
    }

    #[test]
    fn the_shared_list_parses() {
        assert!(!super::load().is_empty());
    }
}
