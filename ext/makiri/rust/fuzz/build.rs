//! The C these targets still need.
//!
//! The port is partial by design, so a Ruby-free build of the engine leaves a
//! handful of symbols in C: the core allocator and buffer (`core/`), and the
//! XPath error setters (`xpath/mkr_xpath_err.c`). Compiling them here is not a
//! workaround - it is the same set the extension itself still links, so the
//! fuzz binary matches the shipped build rather than an idealised one.
//!
//! The HTML engine instance is the exception. The XPath dispatcher references
//! both monomorphizations, but these targets pin the context's engine kind to
//! XML, so the HTML entries are unreachable. They get abort() stubs - abort, not
//! a silent return, so a wrong engine-kind wiring fails loudly instead of
//! fuzzing nothing. That is `verify/stub.c`, compiled from here rather than
//! copied: the CBMC harnesses make the same call for the same reason, and two
//! copies of one decision drift.

use std::path::PathBuf;

fn main() {
    let ext: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..") // ext/makiri/rust/fuzz -> ext/makiri
        .canonicalize()
        .expect("ext/makiri must exist relative to the fuzz crate");

    let srcs = [
        // The HTML-side abort() stubs, shared with the CBMC harnesses rather
        // than copied: both link only the XML engine instance and both pin the
        // context's engine kind to XML, so it is one decision, not two.
        ext.join("../../verify/stub.c"),
        ext.join("core/mkr_alloc.c"),
        ext.join("core/mkr_buf.c"),
        ext.join("core/mkr_utf8.c"),
        ext.join("xpath/mkr_xpath_err.c"),
    ];

    // Headers only. mkr_xpath_err.c includes mkr_xpath.h, which includes
    // <lexbor/dom/dom.h> for the DOM node typedefs; nothing here calls into
    // Lexbor, so the archive is not linked - but the headers have to be there,
    // which means the vendored build must have run at least once.
    // The same place the main crate's build.rs looks, and the same override -
    // two build scripts deriving one path from two different base points is how
    // a move breaks only one of them.
    let lexbor_include = match std::env::var_os("MAKIRI_LEXBOR_INCLUDE") {
        Some(p) => PathBuf::from(p),
        None => ext.join("../../vendor/lexbor/dist/include"),
    };
    println!("cargo:rerun-if-env-changed=MAKIRI_LEXBOR_INCLUDE");
    if !lexbor_include.join("lexbor/dom/dom.h").exists() {
        panic!(
            "vendored Lexbor headers not found at {} - run `bundle exec rake compile` \
             once first (the fuzz targets need the headers, not the library)",
            lexbor_include.display()
        );
    }

    let mut build = cc::Build::new();
    build
        .include(&ext)
        .include(ext.join("core"))
        .include(ext.join("xml"))
        .include(ext.join("xpath"))
        .include(&lexbor_include)
        .flag_if_supported("-fno-omit-frame-pointer");

    // The C support files are NOT instrumented by default, and that is a
    // decision rather than an oversight.
    //
    // cargo-fuzz links Rust's own ASan runtime. Building these files with the
    // platform `cc` instruments them against a DIFFERENT compiler-rt, and the
    // two do not meet: on macOS the link fails outright on Apple clang's
    // `___asan_version_mismatch_check_apple_clang_2100`, which Rust's runtime
    // does not define. Getting both halves onto one runtime means either
    // building the C with the toolchain's own clang or routing Rust through
    // -Zexternal-clangrt and linking the platform runtime by hand - a build
    // that is fragile in exactly the way a nightly job should not be.
    //
    // What is given up is small and already covered elsewhere. These four files
    // are the best-verified C in the project: mkr_alloc.c, mkr_buf.c and
    // mkr_utf8.c carry CBMC proofs that close (cbmc-alloc, cbmc-buf, cbmc-utf8,
    // -chain, -words), and mkr_xpath_err.c is 69 lines of error setting. The
    // half these targets exist to fuzz - the Rust reader and engine - IS
    // instrumented, and ASan's malloc interception still red-zones the
    // allocations these files make. Set MAKIRI_FUZZ_SANITIZE=address on a
    // toolchain where the runtimes do agree to instrument them too.
    let san = std::env::var("MAKIRI_FUZZ_SANITIZE").unwrap_or_else(|_| "none".into());
    if san != "none" {
        build.flag(format!("-fsanitize={san}"));
    }

    for s in &srcs {
        println!("cargo:rerun-if-changed={}", s.display());
        build.file(s);
    }
    println!("cargo:rerun-if-env-changed=MAKIRI_FUZZ_SANITIZE");

    // Emit the link directive by hand, as a trailing link ARG rather than cc's
    // usual `-l`. GNU ld resolves archives strictly left to right and never
    // looks back: the references live in the makiri_rs rlib, so an archive
    // placed before it leaves every one of them undefined (mkr_err_set,
    // mkr_buf_append, mkr_strndup, ...). macOS re-scans and hides the problem,
    // which is exactly how the same mistake reached CI once before - see the
    // --start-group comment in ext/makiri/extconf.rb. A trailing arg is
    // order-correct on both.
    build.cargo_metadata(false);
    build.compile("makiri_fuzz_csupport");
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    println!("cargo:rustc-link-arg={out}/libmakiri_fuzz_csupport.a");
}
