//! The divergences already classified against the pinned HTML Standard.
//!
//! `spec/conformance/parser_known_divergences.txt` lists them, one per line as
//! `<file>#<index> <id> [<parsers>]`, where the id is the entry in
//! `docs/parser_differences.ja.md` that states the spec step deciding it, and
//! `<parsers>` - `lexbor`, `port`, or both comma-separated; both when absent -
//! names the parsers that still diverge (a fix in the port leaves the entry to
//! Lexbor alone). Both runners read this one file - the Ruby one over the
//! public API, this crate's over each parser - so they cannot disagree about
//! what is known. A listed test that starts passing is a failure in both, so
//! the list cannot outlive the divergence.

use std::collections::BTreeMap;

/// The parsers an entry can name.
pub const PARSERS: [&str; 2] = ["lexbor", "port"];

/// `test key -> table id`, the entries that hold for `parser`.
pub fn load(parser: &str) -> BTreeMap<String, String> {
    let path = super::spec_dir().join("parser_known_divergences.txt");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    parse(&text, parser)
}

pub fn parse(text: &str, parser: &str) -> BTreeMap<String, String> {
    assert!(PARSERS.contains(&parser), "unknown parser {parser:?}");
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("# ") && *l != "#")
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let key = it.next().unwrap_or_default().to_string();
            let id = it
                .next()
                .unwrap_or_else(|| panic!("no id on line {l:?}"))
                .to_string();
            let parsers: Vec<&str> = it
                .next()
                .map_or(PARSERS.to_vec(), |p| p.split(',').collect());
            for p in &parsers {
                assert!(PARSERS.contains(p), "unknown parser {p:?} on line {l:?}");
            }
            parsers.contains(&parser).then_some((key, id))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_keys_and_ids_past_comments() {
        let text = "# a comment\n#\ntemplate.dat#44 P1\n\n  tests19.dat#51   P3 lexbor \n";
        let m = super::parse(text, "lexbor");
        assert_eq!(m.len(), 2);
        assert_eq!(m["template.dat#44"], "P1");
        assert_eq!(m["tests19.dat#51"], "P3");
        let m = super::parse(text, "port");
        assert_eq!(m.len(), 1);
        assert_eq!(m["template.dat#44"], "P1");
    }

    #[test]
    fn the_shared_list_parses() {
        assert!(!super::load("lexbor").is_empty());
    }
}
