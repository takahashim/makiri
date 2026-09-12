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
                  #include <lexbor/tag/tag.h>\n\
                  #include <lexbor/css/css.h>\n";

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
        // Lexbor exports declared in lexbor_abi.rs.
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
        // NOT lxb_css_parser_create/init/destroy: glue/css.rs already
        // declares those over an OPAQUE parser, which is the right shape (the
        // selector engine reads no field of it). Generating them here as well
        // gave the same C symbol two Rust types, and only the "everything"
        // feature combination caught it - the same way the mkr_wrap_xml_node
        // duplicate was caught. One declaration per symbol.
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
        // NOT lxb_html_parser_tokenizer / the two token-done accessors: all
        // three are lxb_inline, so bindgen emits nothing and they are declared
        // as `_noi` twins in lexbor_abi.rs. Left here as a record of the check.
        // The mutators and factories glue/html_node/mutate uses. Same rule, and
        // this time every one of them is a real exported function - the three
        // Lexbor exports this file needs that bindgen CANNOT see
        // (lxb_ns_append, lxb_dom_attr_set_name_ns,
        // lxb_dom_attr_qualified_name_append) are absent from the public
        // headers entirely, not inline, so they are hand-declared next to the
        // `_noi` twins in lexbor_abi.rs.
        .allowlist_function("lxb_dom_node_remove")
        .allowlist_function("lxb_dom_node_insert_child")
        .allowlist_function("lxb_dom_node_insert_before")
        .allowlist_function("lxb_dom_node_insert_after")
        .allowlist_function("lxb_dom_node_destroy")
        .allowlist_function("lxb_dom_node_text_content_set")
        .allowlist_function("lxb_dom_element_set_attribute")
        .allowlist_function("lxb_dom_element_remove_attribute")
        .allowlist_function("lxb_dom_element_attr_append")
        .allowlist_function("lxb_dom_element_attr_remove")
        .allowlist_function("lxb_dom_attr_interface_create")
        .allowlist_function("lxb_dom_attr_set_value")
        .allowlist_function("lxb_dom_attr_set_name")
        .allowlist_function("lxb_dom_document_create_element")
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
        .allowlist_function("lxb_css_stylesheet_create")
        .allowlist_function("lxb_css_stylesheet_parse")
        .allowlist_function("lxb_css_stylesheet_destroy")
        .allowlist_function("lxb_css_property_serialize")
        .allowlist_function("lxb_css_property_serialize_name")
        .allowlist_function("lxb_css_selector_serialize_chain")
                // Top-level consts, not modules: the names then match the headers
        // exactly and do not depend on bindgen's numbering of anonymous types.
        // Lexbor's constants are uniquely prefixed, so nothing collides.
        .default_enum_style(bindgen::EnumVariation::Consts)
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

    let ext_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("ext/makiri must exist");
    generate_makiri_enums(&ext_dir, &include, &out);
}

/// Makiri's OWN C types and enums, generated for the same reason Lexbor's are.
///
/// This was added after a transcribed `MKR_NODE_KIND_XML = 1` (it is 2) made
/// `Document#import_node` treat every HTML node as an XML one - the identical
/// failure to the `LXB_NS_HTML` incident, in our own constants, and made while
/// building the machinery that prevents it for Lexbor's. The lesson generalised:
/// a constant that is read rather than derived can be read wrongly, whoever owns
/// the header.
fn generate_makiri_enums(ext: &std::path::Path, lexbor_include: &std::path::Path, out: &std::path::Path) {
    let header = "#include \"glue/cross_import.h\"\n\
                  #include \"dom_adapter/compat.h\"\n";
    println!("cargo:rerun-if-changed={}", ext.join("glue/cross_import.h").display());
    println!("cargo:rerun-if-changed={}", ext.join("dom_adapter/compat.h").display());

    let rb = |k: &str| -> String {
        let out = std::process::Command::new("ruby")
            .args(["-e", &format!("require 'rbconfig'; print RbConfig::CONFIG['{k}']")])
            .output()
            .expect("ruby must be on PATH to locate its headers");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let bindings = bindgen::Builder::default()
        .header_contents("makiri_enums.h", header)
        .clang_arg(format!("-I{}", ext.display()))
        .clang_arg(format!("-I{}", lexbor_include.display()))
        .clang_arg(format!("-I{}", rb("rubyhdrdir")))
        .clang_arg(format!("-I{}", rb("rubyarchhdrdir")))
        .allowlist_type("mkr_node_kind_t")
        .allowlist_type("mkr_doc_kind_t")
        // The parse handle. dom_adapter reads two of its fields (`doc` and the
        // lazily-built index slot), so the layout is generated rather than
        // restated - a field added ahead of `doc` would otherwise be a silent
        // wrong read, which is the class this whole file exists to remove.
        .allowlist_type("mkr_parsed_t")
        .default_enum_style(bindgen::EnumVariation::Consts)
        .layout_tests(false)
        .generate_comments(false)
        .generate()
        .expect("bindgen failed over Makiri's own headers");
    bindings
        .write_to_file(out.join("makiri_enums.rs"))
        .expect("could not write the generated Makiri enums");
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
