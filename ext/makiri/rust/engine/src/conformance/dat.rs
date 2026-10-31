//! The html5lib `.dat` tree-construction format.
//!
//! A file is a run of tests, each a sequence of `#section` headers with the
//! section's lines under them. The reader keeps what a parser test needs: the
//! input (`#data`), the expected dump (`#document`), the fragment context
//! (`#document-fragment`) and the scripting flag (`#script-on` /
//! `#script-off`). `#errors`, `#new-errors` and `#description` are read past.
//! It reads the format as `spec/conformance/html5lib_runner.rb` does, so the
//! two runners agree on what each test is.

use std::path::Path;

/// Whether a test runs with the scripting flag set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scripting {
    /// Neither section: the test holds either way.
    Either,
    On,
    Off,
}

#[derive(Clone, Debug)]
pub struct DatTest {
    /// The file's name (`tests1.dat`).
    pub file: String,
    /// 1-based position in its file, as the Ruby runner and the known
    /// divergence list number it.
    pub index: usize,
    pub data: Vec<u8>,
    /// The expected html5lib dump, no trailing newline.
    pub document: String,
    /// `#document-fragment`'s context: `td`, or `svg desc` for a foreign one.
    pub fragment: Option<String>,
    pub scripting: Scripting,
}

impl DatTest {
    /// `tests1.dat#40`, the key the known divergence list uses.
    pub fn key(&self) -> String {
        format!("{}#{}", self.file, self.index)
    }
}

const SECTIONS: &[&str] = &[
    "data",
    "errors",
    "new-errors",
    "document-fragment",
    "script-on",
    "script-off",
    "document",
    "description",
];

/// Every test in `text`, which came from the file named `file`.
pub fn parse(file: &str, text: &str) -> Vec<DatTest> {
    let mut tests = Vec::new();
    /* A test starts at a "#data" line, as in the Ruby runner's split; the
     * text before the first one is not a test. */
    let mut chunks: Vec<Vec<&str>> = Vec::new();
    for l in text.split_inclusive('\n') {
        let l = l.strip_suffix('\n').unwrap_or(l);
        if l == "#data" {
            chunks.push(Vec::new());
        } else if let Some(c) = chunks.last_mut() {
            c.push(l);
        }
    }
    for chunk in chunks {
        let mut section = "data";
        let mut data: Vec<&str> = Vec::new();
        let mut document: Vec<&str> = Vec::new();
        let mut fragment = None;
        let mut scripting = Scripting::Either;
        for l in chunk {
            if let Some(name) = l.strip_prefix('#').filter(|n| SECTIONS.contains(n)) {
                section = name;
                match name {
                    "script-on" => scripting = Scripting::On,
                    "script-off" => scripting = Scripting::Off,
                    _ => {}
                }
                continue;
            }
            match section {
                "data" => data.push(l),
                "document" => document.push(l),
                "document-fragment" if fragment.is_none() => fragment = Some(l.to_string()),
                _ => {}
            }
        }
        /* The blank line that separates tests lands in the last section. */
        if document.last() == Some(&"") {
            document.pop();
        }
        tests.push(DatTest {
            file: file.to_string(),
            index: tests.len() + 1,
            data: data.join("\n").into_bytes(),
            document: document.join("\n"),
            fragment,
            scripting,
        });
    }
    tests
}

/// Every test of every `.dat` file directly in `dir`, files in name order.
/// `None` when the directory holds none (the data is fetched, not vendored).
pub fn read_dir(dir: &Path) -> Option<Vec<DatTest>> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "dat"))
        .collect();
    files.sort();
    if files.is_empty() {
        return None;
    }
    let mut all = Vec::new();
    for p in files {
        let text = std::fs::read_to_string(&p).ok()?;
        let name = p.file_name()?.to_string_lossy().into_owned();
        all.extend(parse(&name, &text));
    }
    Some(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sections_and_skips_the_rest() {
        let text = "#data\n<p>a\nb\n#errors\n(1,1): x\n#new-errors\n#description\nwhy\n\
                    #document\n| <html>\n|   \"a\nb\"\n\n#data\nx\n#errors\n#document-fragment\n\
                    svg desc\n#script-on\n#document\n| \"x\"\n";
        let t = parse("t.dat", text);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].data, b"<p>a\nb");
        assert_eq!(t[0].document, "| <html>\n|   \"a\nb\"");
        assert_eq!(t[0].fragment, None);
        assert_eq!(t[0].scripting, Scripting::Either);
        assert_eq!(t[1].key(), "t.dat#2");
        assert_eq!(t[1].fragment.as_deref(), Some("svg desc"));
        assert_eq!(t[1].scripting, Scripting::On);
        assert_eq!(t[1].document, "| \"x\"");
    }
}
