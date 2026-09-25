//! Generate the Lexbor layout from Lexbor's own headers.
//!
//! # Why generated rather than written
//!
//! Lexbor is a vendored dependency whose pin moves (CLAUDE.md). A hand-written
//! `#[repr(C)]` view of one of its structs does not fail to build when a field
//! is added or reordered - it reads the wrong offset, which is a silent wrong
//! answer. Nothing in the crate declares a Lexbor struct or a header-declared
//! Lexbor function by hand any more; the three exports no header declares are
//! the exception, and `check_undeclared_exports` pins their C definitions.
//!
//! Transcription by hand has already been wrong once, though, and not about an
//! offset: `LXB_NS_HTML` is 2, and guessing 1 made every HTML element foreign,
//! so every unprefixed name test matched nothing. bindgen removes that whole
//! class - the numbers come from the headers being compiled, not from a reading
//! of them.
//!
//! # What this does NOT remove
//!
//! bindgen reads the headers with libclang; the vendored Lexbor archive is
//! compiled by whatever `cc` cmake picks. They agree in every ordinary case,
//! but nothing here proves it - and with no C of our own left, there is no
//! translation unit that could report the real `sizeof`/`offsetof` back. What
//! survives is the narrower guarantee: the layout and the signatures come from
//! the headers Lexbor was built from, not from a reading of them.
//!
//! # Scope
//!
//! Allowlisted, not wholesale: Lexbor's headers are large and most of them are
//! nothing to do with us. One rule worth remembering, because it cost a round
//! to find - Lexbor's constants live in ANONYMOUS enums (`lxb_ns_id_enum_t`,
//! `lxb_dom_node_type_t`), so allowlisting by constant name matches nothing.
//! The enum TYPE has to be allowlisted instead.

// The crate's panic gate does not apply here: a build script reports failure BY
// panicking - cargo prints the message and stops the build - so aborting is the
// interface, not a missing error path. Nothing in this file ships.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MAKIRI_LEXBOR_INCLUDE");

    // Only a build that reads Lexbor's layout pays for this. Kani builds with
    // `--no-default-features` and has no Lexbor headers to point at, which is
    // the whole reason `rake kani` needs no `rake compile` first.
    if std::env::var_os("CARGO_FEATURE_LEXBOR").is_none() {
        return;
    }

    let include = lexbor_include();
    println!("cargo:rerun-if-changed={}", include.display());

    let header = "#include <lexbor/dom/dom.h>\n\
                  #include <lexbor/html/html.h>\n\
                  #include <lexbor/ns/ns.h>\n\
                  #include <lexbor/tag/tag.h>\n\
                  #include <lexbor/css/css.h>\n\
                  #include <lexbor/selectors/selectors.h>\n";

    check_undeclared_exports(&include);

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
        // Read only for their id fields, by the three header-less
        // Lexbor exports declared in lexbor/abi.rs.
        .allowlist_type("lxb_ns_data_t")
        .allowlist_type("lxb_dom_attr_data_t")
        .allowlist_type("lxb_dom_exception_code_t")
        .allowlist_type("lexbor_str_t")
        // <template>'s content fragment. Lexbor's accessor is a cast macro, so
        // the field read is the whole interface.
        .allowlist_type("lxb_html_template_element_t")
        .allowlist_type("lxb_dom_document_fragment_t")
        .allowlist_type("lxb_tag_id_enum_t")
        // The constants. By ENUM TYPE - see the note above.
        .allowlist_type("lxb_ns_id_enum_t")
        .allowlist_type("lxb_dom_node_type_t")
        // The CSS stylesheet surface (Makiri::Lexbor::CSS.parse_stylesheet).
        // Lexbor exposes the rule downcasts as macros - plain pointer casts
        // over a shared header - so there is nothing to link, only layout to
        // get right, which is exactly what generating it is for.
        .allowlist_type("lxb_css_stylesheet_t")
        .allowlist_type("lxb_css_rule_t")
        .allowlist_type("lxb_css_rule_list_t")
        .allowlist_type("lxb_css_rule_at_t")
        .allowlist_type("lxb_css_rule_style_t")
        .allowlist_type("lxb_css_rule_bad_style_t")
        .allowlist_type("lxb_css_rule_declaration_t")
        .allowlist_type("lxb_css_rule_declaration_list_t")
        .allowlist_type("lxb_css_selector_list_t")
        // The selector tree mkr_css.c lowers into XPath. A union-carrying
        // layout from a pinned dependency: exactly what generating is for.
        .allowlist_type("lxb_css_selector_t")
        .allowlist_type("lxb_css_selector_attribute_t")
        .allowlist_type("lxb_css_selector_anb_of_t")
        .allowlist_type("lxb_css_selector_contains_t")
        .allowlist_type("lxb_css_selector_type_t")
        .allowlist_type("lxb_css_selector_combinator_t")
        .allowlist_type("lxb_css_selector_match_t")
        .allowlist_type("lxb_css_selector_modifier_t")
        // The pseudo enums are `*_id_t`, not `*_t`: allowlisting the `_t`
        // spelling matched nothing and produced no constants at all - the same
        // trap as lxb_html_token_type, and the second time this exact shape has
        // cost a round.
        .allowlist_type("lxb_css_selector_pseudo_class_id_t")
        .allowlist_type("lxb_css_selector_pseudo_class_function_id_t")
        .allowlist_type("lxb_css_selector_pseudo_element_id_t")
        .allowlist_type("lxb_css_selectors_t")
        .allowlist_type("lxb_css_memory_t")
        .allowlist_type("lxb_css_at_rule__custom_t")
        .allowlist_type("lxb_css_at_rule__undef_t")
        .allowlist_type("lxb_css_at_rule_media_t")
        .allowlist_type("lxb_css_at_rule_font_face_t")
        .allowlist_type("lxb_css_rule_type_t")
        // The at-rule types live in a TRULY anonymous enum (no typedef name),
        // so there is no type to allowlist - only the items. That also rules
        // out ModuleConsts for them: bindgen would name the module
        // `_bindgen_ty_3`, and the number shifts when any other anonymous type
        // is added. Hence Consts below, which puts every constant at the top
        // level under the name it has in C.
        .allowlist_item("LXB_CSS_AT_RULE_.*")
        // The status codes: the enum is typedef'd `lexbor_status_t`, so the
        // TYPE is what allowlists it - allowlisting the constant names matches
        // nothing (the same trap as lxb_ns_id_enum_t).
        .allowlist_type("lexbor_status_t")
        .allowlist_type("lxb_html_serialize_opt")
        .allowlist_type("lxb_tag_id_enum_t")
        // The selector parser, its arena and its selector table. The parser is
        // OPAQUE below - nothing reads a field of it - so its large layout
        // stays out of the generated file.
        .allowlist_function("lxb_css_parser_create")
        .allowlist_function("lxb_css_parser_init")
        .allowlist_function("lxb_css_parser_clean")
        .allowlist_function("lxb_css_parser_destroy")
        .allowlist_function("lxb_css_memory_create")
        .allowlist_function("lxb_css_memory_init")
        .allowlist_function("lxb_css_memory_clean")
        .allowlist_function("lxb_css_memory_destroy")
        .allowlist_function("lxb_css_selectors_create")
        .allowlist_function("lxb_css_selectors_init")
        .allowlist_function("lxb_css_selectors_destroy")
        .allowlist_function("lxb_css_selectors_parse")
        .opaque_type("lxb_css_parser_t")
        .opaque_type("lxb_css_syntax_tokenizer_t")
        // The DOM readers glue/html_node uses. Generating them rather than
        // hand-declaring them is also the inline-only CHECK: bindgen does not
        // emit a `static inline`, so a name that is only inline in the headers
        // simply does not appear here and the use fails to compile, instead of
        // linking to nothing and becoming a NULL jump at runtime under macOS's
        // `-undefined dynamic_lookup`. Three such functions already cost this
        // project a segfault; the list below is what survived the check.
        .allowlist_function("lxb_dom_element_qualified_name")
        .allowlist_function("lxb_dom_element_local_name")
        .allowlist_function("lxb_dom_element_tag_name")
        .allowlist_function("lxb_dom_element_has_attribute")
        .allowlist_function("lxb_dom_element_get_attribute")
        .allowlist_function("lxb_dom_element_first_attribute")
        .allowlist_function("lxb_dom_element_next_attribute")
        .allowlist_function("lxb_dom_attr_qualified_name")
        .allowlist_function("lxb_dom_attr_local_name")
        .allowlist_function("lxb_dom_attr_value")
        .allowlist_function("lxb_dom_node_name")
        .allowlist_function("lxb_dom_node_text_content")
        .allowlist_function("lxb_dom_document_type_public_id")
        .allowlist_function("lxb_dom_document_type_system_id")
        .allowlist_function("lxb_dom_processing_instruction_target")
        .allowlist_function("lxb_dom_document_destroy_text")
        .allowlist_function("lxb_ns_by_id")
        .allowlist_function("lxb_dom_document_root")
        .allowlist_type("lxb_html_token_t")
        .allowlist_type("lxb_html_token_type_t")
        // The token-type FLAGS are in `enum lxb_html_token_type` - no trailing
        // `_t`, because `lxb_html_token_type_t` is a separate `typedef int`.
        // Allowlisting the `_t` spelling matched the typedef and produced no
        // constants at all; the enum's own name is what carries them.
        .allowlist_type("lxb_html_token_type")
        // The memory pools, walked to report a document's live bytes.
        .allowlist_type("lexbor_mem_t")
        .allowlist_type("lexbor_mem_chunk_t")
        .allowlist_type("lexbor_mraw_t")
        .allowlist_type("lxb_html_document_t")
        .allowlist_type("lxb_html_tokenizer_t")
        .allowlist_type("lxb_html_tokenizer_token_f")
        .allowlist_type("lxb_html_parser_t")
        .allowlist_function("lxb_html_parser_create")
        .allowlist_function("lxb_html_parser_init")
        .allowlist_function("lxb_html_parser_destroy")
        .allowlist_function("lxb_html_parse_chunk_begin")
        .allowlist_function("lxb_html_parse_chunk_process")
        .allowlist_function("lxb_html_parse_chunk_end")
        // The `_noi` twins. Lexbor publishes these accessors as `lxb_inline`,
        // which bindgen does not emit - allowlisting the plain name yields
        // nothing, and that is the inline-only CHECK described above. But each
        // has an exported `_noi` twin DECLARED in the same header, and those
        // generate like any other function: so their signatures come from the
        // headers too, and a Lexbor change to one fails the build.
        .allowlist_function("lxb_dom_node_type_noi")
        .allowlist_function("lxb_dom_attr_value_noi")
        .allowlist_function("lxb_dom_element_first_attribute_noi")
        .allowlist_function("lxb_dom_element_next_attribute_noi")
        .allowlist_function("lxb_dom_document_type_public_id_noi")
        .allowlist_function("lxb_dom_document_type_system_id_noi")
        .allowlist_function("lxb_dom_processing_instruction_target_noi")
        .allowlist_function("lxb_dom_document_destroy_text_noi")
        .allowlist_function("lxb_tag_id_by_name_noi")
        .allowlist_function("lxb_html_parser_tokenizer_noi")
        .allowlist_function("lxb_html_tokenizer_callback_token_done_set_noi")
        .allowlist_function("lxb_html_tokenizer_callback_token_done_ctx_noi")
        .allowlist_function("lxb_css_parser_status_noi")
        .allowlist_function("lxb_css_parser_memory_set_noi")
        .allowlist_function("lxb_css_parser_selectors_set_noi")
        // The fragment parse by tag id and the fragment constructor: ordinary
        // header-declared exports.
        .allowlist_function("lxb_html_parse_fragment_by_tag_id")
        .allowlist_function("lxb_dom_document_fragment_interface_create")
        // The mutators and factories glue/html_node/mutate uses. Three more
        // Lexbor exports it needs are in NO header at all (lxb_ns_append,
        // lxb_dom_attr_set_name_ns, lxb_dom_attr_qualified_name_append), so
        // bindgen cannot see them: they are hand-declared in lexbor/abi.rs, and
        // `check_undeclared_exports` below pins their C definitions instead.
        .allowlist_function("lxb_dom_node_remove")
        .allowlist_function("lxb_dom_node_insert_child")
        .allowlist_function("lxb_dom_node_insert_before")
        .allowlist_function("lxb_dom_node_insert_after")
        .allowlist_function("lxb_dom_node_text_content_set")
        .allowlist_function("lxb_dom_element_set_attribute")
        .allowlist_function("lxb_dom_element_attr_by_name")
        .allowlist_function("lxb_dom_element_attr_is_exist")
        .allowlist_function("lxb_dom_element_attr_append")
        .allowlist_function("lxb_dom_element_attr_remove")
        .allowlist_function("lxb_dom_attr_interface_create")
        .allowlist_function("lxb_dom_attr_set_value")
        .allowlist_function("lxb_dom_attr_set_name")
        .allowlist_function("lxb_dom_document_create_element")
        .allowlist_function("lxb_dom_element_create")
        .allowlist_function("lxb_dom_document_create_text_node")
        .allowlist_function("lxb_dom_document_create_comment")
        .allowlist_function("lxb_dom_document_create_processing_instruction")
        .allowlist_function("lxb_dom_document_create_document_fragment")
        .allowlist_function("lxb_dom_document_type_create")
        .allowlist_function("lxb_dom_document_type_valid_name")
        .allowlist_function("lxb_dom_document_import_node")
        .allowlist_function("lexbor_str_init")
        .allowlist_function("lxb_html_parse_fragment")
        .allowlist_function("lxb_html_document_destroy")
        // The document title reader, generated rather than hand-declared like
        // the rest of this list.
        .allowlist_function("lxb_html_document_title")
        .allowlist_function("lxb_css_stylesheet_create")
        .allowlist_function("lxb_css_stylesheet_parse")
        .allowlist_function("lxb_css_stylesheet_destroy")
        .allowlist_function("lxb_css_property_serialize")
        .allowlist_function("lxb_css_property_serialize_name")
        .allowlist_function("lxb_css_selector_serialize_chain")
        // The HTML serializers `lexbor::serialize` drives through its sink.
        .allowlist_function("lxb_html_serialize_tree_cb")
        .allowlist_function("lxb_html_serialize_deep_cb")
        .allowlist_function("lxb_html_serialize_pretty_tree_cb")
        .allowlist_function("lxb_html_serialize_pretty_deep_cb")
        // The selector matcher `lexbor::selectors` runs. Its option setter is
        // an inline whose `_noi` twin the header declares, so it generates too.
        .allowlist_function("lxb_selectors_create")
        .allowlist_function("lxb_selectors_init")
        .allowlist_function("lxb_selectors_destroy")
        .allowlist_function("lxb_selectors_find")
        .allowlist_function("lxb_selectors_match_node")
        .allowlist_function("lxb_selectors_opt_set_noi")
        .allowlist_type("lxb_selectors_opt_t")
        // Top-level consts, not modules: the names then match the headers
        // exactly and do not depend on bindgen's numbering of anonymous types.
        // Lexbor's constants are uniquely prefixed, so nothing collides.
        .default_enum_style(bindgen::EnumVariation::Consts)
        // Layout tests are `#[test]` functions, and this crate's are not run by
        // `cargo test` (they need a live Ruby), so they would be dead weight.
        // The compile-time asserts in lexbor/abi.rs are what actually run.
        .layout_tests(false)
        .generate_comments(false)
        .derive_default(false)
        .generate()
        .expect("bindgen failed over the vendored Lexbor headers");

    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    bindings
        .write_to_file(out.join("lexbor_sys.rs"))
        .expect("could not write the generated Lexbor bindings");

    // Link the vendored Lexbor static library. extconf.rb also passes this
    // archive when building the Ruby extension, so the extension Makefile's
    // link line and this build.rs line duplicate the same archive. That is
    // harmless for a static archive (the linker pulls only the object files it
    // needs), and having it here means cargo consumers that do not go through
    // extconf - chiefly cargo-fuzz - still link Lexbor.
    let lib_dir = lexbor_include().parent().unwrap().join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=lexbor_static");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("-linux-") || target.contains("-darwin-") {
        println!("cargo:rustc-link-lib=pthread");
    }

    // Makiri's OWN enums were generated here too, from ext/makiri/*.h, for the
    // same reason Lexbor's are - a transcribed `MKR_NODE_KIND_XML = 1` (it is 2)
    // had made `Document#import_node` treat every HTML node as an XML one. Those
    // headers are gone with the rest of the C, and the definitions are now
    // ordinary Rust consts in `lexbor::abi::mkr`, so there is no second reading
    // of them left to keep in agreement.
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

/// The Lexbor exports no header declares, with the C definition `lexbor/abi.rs`
/// was written against: (source file, return type, name, parameters), each as
/// the source spells it, whitespace collapsed.
///
/// bindgen cannot check these - there is no declaration for it to read - so a
/// Lexbor bump that changes one would otherwise link cleanly and be called with
/// the wrong arguments. This reads the DEFINITION instead and fails the build
/// on any difference, which forces the hand-written declaration to be looked at
/// again. A name that is not found at all fails too: that is the export having
/// gone, which `rake symbols` would otherwise report only after a link.
const UNDECLARED_EXPORTS: &[(&str, &str, &str, &str)] = &[
    (
        "lexbor/ns/ns.c",
        "LXB_API const lxb_ns_data_t *",
        "lxb_ns_append",
        "lexbor_hash_t *hash, const lxb_char_t *link, size_t length",
    ),
    (
        "lexbor/dom/interfaces/attr.c",
        "lxb_status_t",
        "lxb_dom_attr_set_name_ns",
        "lxb_dom_attr_t *attr, const lxb_char_t *link, size_t link_length, \
         const lxb_char_t *name, size_t name_length, bool to_lowercase",
    ),
    (
        "lexbor/dom/interfaces/attr.c",
        "lxb_dom_attr_data_t *",
        "lxb_dom_attr_qualified_name_append",
        "lexbor_hash_t *hash, const lxb_char_t *name, size_t length",
    ),
    (
        "lexbor/dom/interfaces/element.c",
        "LXB_API lxb_status_t",
        "lxb_dom_element_qualified_name_set",
        "lxb_dom_element_t *element, const lxb_char_t *prefix, size_t prefix_len, \
         const lxb_char_t *lname, size_t lname_len",
    ),
];

fn check_undeclared_exports(include: &std::path::Path) {
    let source = match std::env::var_os("MAKIRI_LEXBOR_SOURCE") {
        Some(p) => std::path::PathBuf::from(p),
        /* `<vendor/lexbor>/dist/include` -> `<vendor/lexbor>/source` */
        None => include.join("../../source"),
    };
    let collapse = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    for &(file, ret, name, params) in UNDECLARED_EXPORTS {
        let path = source.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {} to check {name}: {e}", path.display()));
        let found = definition(&text, name).unwrap_or_else(|| {
            panic!(
                "no definition of {name} in {} - the export lexbor/abi.rs \
                 declares by hand is gone",
                path.display()
            )
        });
        let want = (collapse(ret), collapse(params));
        let got = (collapse(&found.0), collapse(&found.1));
        if got != want {
            panic!(
                "{name} changed in {}:\n  was: {} {name}({})\n  now: {} {name}({})\n\
                 update its declaration in lexbor/abi.rs, then UNDECLARED_EXPORTS",
                path.display(),
                want.0,
                want.1,
                got.0,
                got.1
            );
        }
    }
}

/// `(return type, parameters)` of the function DEFINED as `name` in `text`:
/// Lexbor's style puts the return type on the line above a name that starts
/// its line, and a definition - unlike the forward declarations the same files
/// carry - is followed by `{`.
fn definition(text: &str, name: &str) -> Option<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let open = format!("{name}(");
    for (i, line) in lines.iter().enumerate() {
        if i == 0 || !line.starts_with(&open) {
            continue;
        }
        let rest = lines[i..].join("\n");
        let close = rest.find(')')?;
        let params = &rest[open.len()..close];
        if rest[close + 1..].trim_start().starts_with('{') {
            return Some((lines[i - 1].to_string(), params.to_string()));
        }
    }
    None
}
