//! Generate the Lexbor layout from Lexbor's own headers.
//!
//! # Why generated rather than written
//!
//! Lexbor is a vendored dependency whose pin moves (CLAUDE.md). A hand-written
//! `#[repr(C)]` view of one of its structs does not fail to build when a field
//! is added or reordered - it reads the wrong offset, which is a silent wrong
//! answer. `xpath/html_abi.rs` handles that today by declaring the three node
//! structs by hand and having `mkr_xpath_html_shim.c` compare every offset
//! against the real `offsetof` at load time. That works, and it caught nothing
//! only because nobody has moved the pin since.
//!
//! Transcription by hand has already been wrong once, though, and not about an
//! offset: `LXB_NS_HTML` is 2, and guessing 1 made every HTML element foreign,
//! so every unprefixed name test matched nothing. bindgen removes that whole
//! class - the numbers come from the headers being compiled, not from a reading
//! of them.
//!
//! # What this does NOT remove
//!
//! bindgen reads the headers with libclang; the extension's C is compiled with
//! whatever `cc` the Ruby build uses. They agree in every ordinary case, but
//! nothing here proves it. That residual is what keeps a small C translation
//! unit reporting real `sizeof`/`offsetof` worth having - see
//! notes/rust_port_remaining.ja.md §2. The difference is that the C side now
//! checks a generated view instead of being the only source of truth for a
//! hand-written one.
//!
//! # Scope
//!
//! Allowlisted, not wholesale: Lexbor's headers are large and most of them are
//! nothing to do with us. One rule worth remembering, because it cost a round
//! to find - Lexbor's constants live in ANONYMOUS enums (`lxb_ns_id_enum_t`,
//! `lxb_dom_node_type_t`), so allowlisting by constant name matches nothing.
//! The enum TYPE has to be allowlisted instead.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MAKIRI_LEXBOR_INCLUDE");

    // Only the features that actually read Lexbor's layout pay for this. Kani
    // builds `xml,xpath` and has no Lexbor headers to point at, which is the
    // whole reason `rake kani` needs no `rake compile` first.
    if std::env::var_os("CARGO_FEATURE_LEXBOR_ABI").is_none() {
        return;
    }

    let include = lexbor_include();
    println!("cargo:rerun-if-changed={}", include.display());

    let header = "#include <lexbor/dom/dom.h>\n\
                  #include <lexbor/html/html.h>\n\
                  #include <lexbor/ns/ns.h>\n\
                  #include <lexbor/tag/tag.h>\n";

    let bindings = bindgen::Builder::default()
        .header_contents("makiri_lexbor.h", header)
        .clang_arg(format!("-I{}", include.display()))
        // The structs we navigate by. `lxb_dom_document_t` is here for its
        // pointer type, not to be read field-by-field: it is large, carries
        // function pointers, and the two fields anyone wants (`ns`, `tags`)
        // are reached through shims that see the real header.
        .allowlist_type("lxb_dom_node_t")
        .allowlist_type("lxb_dom_element_t")
        .allowlist_type("lxb_dom_attr_t")
        .allowlist_type("lxb_dom_character_data_t")
        .allowlist_type("lxb_dom_document_type_t")
        .allowlist_type("lxb_dom_processing_instruction_t")
        .allowlist_type("lexbor_str_t")
        // The constants. By ENUM TYPE - see the note above.
        .allowlist_type("lxb_ns_id_enum_t")
        .allowlist_type("lxb_dom_node_type_t")
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        // Layout tests are `#[test]` functions; this crate is a staticlib that
        // is never `cargo test`ed, so they would be dead weight. The compile
        // time asserts in lexbor_abi.rs are what actually run.
        .layout_tests(false)
        .generate_comments(false)
        .derive_default(false)
        .generate()
        .expect("bindgen failed over the vendored Lexbor headers");

    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    bindings
        .write_to_file(out.join("lexbor_sys.rs"))
        .expect("could not write the generated Lexbor bindings");
}

/// Where the vendored Lexbor headers are. extconf builds them into
/// `vendor/lexbor/dist` before it invokes cargo, so the default is that path
/// relative to this crate; `MAKIRI_LEXBOR_INCLUDE` overrides it for anything
/// that builds the crate from somewhere else.
fn lexbor_include() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("MAKIRI_LEXBOR_INCLUDE") {
        return std::path::PathBuf::from(p);
    }
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let p = manifest.join("../../../vendor/lexbor/dist/include");
    if !p.join("lexbor/dom/dom.h").exists() {
        panic!(
            "vendored Lexbor headers not found at {} - run `bundle exec rake compile` \
             once, or set MAKIRI_LEXBOR_INCLUDE",
            p.display()
        );
    }
    p
}
