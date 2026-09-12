# frozen_string_literal: true

require "mkmf"
require "rbconfig"
require "fileutils"
require "shellwords"
require "etc"
require "set"

# extconf for the Makiri C extension.
#
# Build strategy (see design_doc §12):
#   1. Build vendored Lexbor (unpatched) via cmake into vendor/lexbor/build,
#      install headers + a static archive into vendor/lexbor/dist.
#   2. Compile ext/makiri/**/*.c with rake-compiler, linking against the
#      static Lexbor archive only - no system libxml2/libxslt.
#
# Security note: the C extension is built with -D_FORTIFY_SOURCE=2,
# -fstack-protector-strong, and -Wformat -Wformat-security. -O2 is kept
# (not lowered) so fortify checks remain active.

EXT_DIR    = __dir__
GEM_ROOT   = File.expand_path("../..", EXT_DIR)
LEXBOR_SRC = File.join(GEM_ROOT, "vendor", "lexbor")
LEXBOR_BLD = File.join(LEXBOR_SRC, "build")
LEXBOR_DST = File.join(LEXBOR_SRC, "dist")

abort "Lexbor source not found at #{LEXBOR_SRC}. Did you `git submodule update --init`?" unless File.directory?(LEXBOR_SRC)

cmake = find_executable("cmake") or abort "cmake is required to build Lexbor."

# Windows (RubyInstaller / MinGW-UCRT gcc). Used below to force the GNU
# toolchain for the Lexbor build and to keep the Ruby import-library link.
windows = RbConfig::CONFIG["target_os"] =~ /mingw|mswin/

# Optionally build the vendored Lexbor itself under AddressSanitizer. This is the
# ONLY way to catch overflows *inside* Lexbor's bump (mraw) arena: a sub-allocation
# overrunning into the next one stays within one big malloc'd chunk, so the heap
# allocator's red-zones (and thus a plain ASan build of just our ext) never see it.
# Lexbor's own mraw is ASan-aware - with -DLEXBOR_BUILD_WITH_ASAN=ON its CMake
# defines LEXBOR_HAVE_ADDRESS_SANITIZER, and mraw then unpoisons exactly each
# allocation and re-poisons the gap, so an intra-arena overrun writes into
# poisoned memory and ASan reports it. Opt-in (slow full rebuild), only meaningful
# with MAKIRI_SANITIZE=...address...; drive it via `rake sanitize:lexbor`.
# vendor/lexbor stays vanilla - this is a build flag, not a source patch.
sanitize = ENV["MAKIRI_SANITIZE"].to_s.strip
lexbor_asan = !ENV["MAKIRI_SANITIZE_LEXBOR"].to_s.strip.empty? && sanitize.include?("address")
lexbor_mode = lexbor_asan ? "asan" : "plain"
lexbor_stamp = File.join(LEXBOR_DST, ".makiri_build_mode")

# Reuse the cached archive only when it was built in the mode we now want; a mode
# switch (plain <-> asan) forces a rebuild, so a sanitized Lexbor can never leak
# into a normal build or vice versa.
have_archive = File.exist?(File.join(LEXBOR_DST, "lib", "liblexbor_static.a"))
stamp_ok = have_archive && File.exist?(lexbor_stamp) && File.read(lexbor_stamp).strip == lexbor_mode
unless stamp_ok
  FileUtils.rm_rf(LEXBOR_BLD)
  FileUtils.rm_rf(LEXBOR_DST) if have_archive   # drop a wrong-mode install
  FileUtils.mkdir_p(LEXBOR_BLD)
  Dir.chdir(LEXBOR_BLD) do
    cmd = [
      cmake,
      # On Windows the runner ships Visual Studio, so a bare cmake defaults to the
      # MSVC generator -> lexbor_static.lib under a Release/ subdir, breaking the
      # hardcoded liblexbor_static.a path below. Force the GNU toolchain via Ninja
      # (bundled by lukka/get-cmake; single-config, so no Release/ subdir) and pin
      # the MinGW-UCRT gcc so cmake can't auto-probe MSVC. "MinGW Makefiles" is
      # avoided: cmake refuses it when sh.exe is on PATH (MSYS2/Git-Bash put it there).
      *(windows ? ["-G", "Ninja", "-DCMAKE_C_COMPILER=gcc"] : []),
      "-DLEXBOR_BUILD_SHARED=OFF",
      "-DLEXBOR_BUILD_STATIC=ON",
      "-DLEXBOR_BUILD_TESTS=OFF",
      "-DLEXBOR_BUILD_EXAMPLES=OFF",
      "-DLEXBOR_BUILD_UTILS=OFF",
      "-DCMAKE_BUILD_TYPE=Release",
      "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
      "-DCMAKE_INSTALL_PREFIX=#{LEXBOR_DST}",
      *(lexbor_asan ? ["-DLEXBOR_BUILD_WITH_ASAN=ON"] : []),
      LEXBOR_SRC,
    ]
    warn "makiri: building vendored Lexbor (mode=#{lexbor_mode})"
    # Multi-arg system() bypasses the shell, so args pass verbatim on every
    # platform. Do NOT shelljoin: Shellwords escapes `=` as `\=`, which a POSIX
    # shell unwraps but Windows cmd.exe does not, mangling every -DNAME=VALUE
    # (cmake then ignores CMAKE_INSTALL_PREFIX etc. and installs to the default).
    system(*cmd) or abort "cmake configure failed for Lexbor."
    nproc = Etc.respond_to?(:nprocessors) ? Etc.nprocessors : 4
    # `-- -jN` forwards to make and is wrong for Ninja; use cmake's portable
    # --parallel (cmake >= 3.12, satisfied by get-cmake) on Windows.
    build_parallel = windows ? ["--parallel", nproc.to_s] : ["--", "-j#{nproc}"]
    system(cmake, "--build", ".", "--target", "install", *build_parallel) or
      abort "cmake build/install failed for Lexbor."
  end
  File.write(lexbor_stamp, lexbor_mode)
end

$INCFLAGS << " -I#{File.join(LEXBOR_DST, 'include').shellescape}"
$INCFLAGS << " -I#{File.join(EXT_DIR).shellescape}"

# On Windows, Lexbor's public headers default LXB_API to __declspec(dllimport)
# (lexbor/core/def.h), emitting __imp_* references that don't resolve against the
# STATIC archive we link. Lexbor's own static CMake target defines LEXBOR_STATIC to
# neutralize this, but we link the .a directly (not via CMake's exported target),
# so we must define it ourselves. Harmless (and a no-op macro) on other platforms.
$CFLAGS << " -DLEXBOR_STATIC" if windows

# Hard-link the static archive rather than pass -L/-llexbor_static, to
# avoid accidentally linking a system-installed Lexbor.
lexbor_archive = File.join(LEXBOR_DST, "lib", "liblexbor_static.a")
$LDFLAGS << " #{lexbor_archive.shellescape}"

# Sanitizer build (opt-in): MAKIRI_SANITIZE=address,undefined rake clean compile
# Then run the suite under the runtime via `rake sanitize` (which preloads the
# ASan runtime). Sanitizers replace the heap allocator, so even the vendored
# (uninstrumented) Lexbor's allocations get red-zoned - a heap overflow off the
# END of a Lexbor malloc is caught. Overflows *inside* Lexbor's mraw arena are
# NOT caught this way (they stay within one malloc'd chunk); for those, also build
# Lexbor under ASan via MAKIRI_SANITIZE_LEXBOR=1 (see the Lexbor build above and
# `rake sanitize:lexbor`). _FORTIFY_SOURCE is dropped here because it conflicts
# with the sanitizer interceptors.
# Coverage build (opt-in): MAKIRI_COVERAGE=1 instruments OUR sources with clang
# source-based coverage (the vendored Lexbor is built separately and is NOT
# instrumented - we measure only the code we write). Run via `rake coverage`,
# which sets LLVM_PROFILE_FILE and renders an llvm-cov report. -O0 keeps the
# region map close to the source; _FORTIFY_SOURCE is dropped (it needs -O2).
coverage = !ENV["MAKIRI_COVERAGE"].to_s.strip.empty?

# OOM-injection build (opt-in): MAKIRI_ALLOC_INJECT=1 compiles the core
# allocation-failure hook (mkr_alloc.h) so `rake oom` can sweep "the nth core
# allocation fails" over representative workloads and assert every OOM branch
# fails closed. Debug/test builds only - a normal build carries no hook.
# Composes with the sanitize/coverage modes below.
alloc_inject = ENV["MAKIRI_ALLOC_INJECT"].to_s.strip == "1"
if alloc_inject
  $CFLAGS << " -DMKR_ALLOC_INJECT=1"
  warn "makiri: building with allocation-failure injection (MKR_ALLOC_INJECT)"
end

if coverage
  $CFLAGS   << " -O0 -g -fprofile-instr-generate -fcoverage-mapping"
  $LDFLAGS  << " -fprofile-instr-generate"
  $DLDFLAGS << " -fprofile-instr-generate"
  warn "makiri: building with clang source-based coverage"
elsif sanitize.empty?
  # Security hardening flags. Keep -O2 active so _FORTIFY_SOURCE works.
  $CFLAGS << " -O2"
  $CFLAGS << " -D_FORTIFY_SOURCE=2"
else
  $CFLAGS << " -O1 -g -fno-omit-frame-pointer -fsanitize=#{sanitize}"
  $LDFLAGS  << " -fsanitize=#{sanitize}"
  $DLDFLAGS << " -fsanitize=#{sanitize}"
  if sanitize.include?("address")
    # No ASan *stack* red zones in the ext. CRuby is built with
    # RUBY_SETJMP = __builtin_setjmp, so rb_raise unwinds via __builtin_longjmp,
    # which the ASan runtime does not intercept (no __asan_handle_no_return):
    # any raise crossing an instrumented frame - ours, or Ruby code raising
    # through rb_protect under the evaluator - leaves that frame's stack poison
    # behind, and an interceptor (memcpy & co.) in the uninstrumented interpreter
    # later trips over the stale shadow: a spurious report, which ASan itself
    # then aborts while rendering (asan_thread.cpp kCurrentStackFrameMagic
    # CHECK). Heap red zones, UBSan, and the manual arena poisoning in
    # mkr_xml_node.c are unaffected; only stack-buffer checks are lost.
    $CFLAGS << (RbConfig::CONFIG["CC"] =~ /clang/ || RbConfig::CONFIG["target_os"] =~ /darwin/ ?
                  " -mllvm -asan-stack=0" : " --param asan-stack=0")
  end
  warn "makiri: building with -fsanitize=#{sanitize}"
end

$CFLAGS << " -fstack-protector-strong"
$CFLAGS << " -Wformat -Wformat-security"
$CFLAGS << " -fvisibility=hidden"
$CFLAGS << " -fno-common"

# Silence a benign false positive from Ruby 3.4's <ruby/internal/core/rstring.h>:
# its static-inline rbimpl_rstring_getmem default-initialises a struct RString
# whose const member it never sets (callers only read the fields it does set),
# which newer clang flags as -Wdefault-const-init-field-unsafe. The warning is
# emitted in every TU that includes ruby.h, not by our code, so suppress just
# this one flag (guarded so compilers without it are unaffected).
%w[-Wno-default-const-init-field-unsafe].each do |flag|
  $CFLAGS << " #{flag}" if try_cflags(flag)
end
# Relocation hardening for shared object.
$DLDFLAGS << " -Wl,-z,relro" if RbConfig::CONFIG["target_os"] =~ /linux/
$DLDFLAGS << " -Wl,-z,now"   if RbConfig::CONFIG["target_os"] =~ /linux/

# Portability of the compiled extension (matters for precompiled / "fat" gems).
# A Ruby built --enable-shared (Homebrew, rbenv/ruby-build, mise, GitHub
# setup-ruby) otherwise makes mkmf hard-link the BUILD Ruby's libruby:
#   macOS  -> an absolute path to libruby.X.Y.dylib, so the .bundle refuses to
#            load on any other Ruby install ("linked to incompatible ...").
#   Linux  -> a libruby.so.X.Y SONAME dependency.
# Resolve the Ruby C API from the host process at load time instead, so one
# compiled binary works on any compatible Ruby of that ABI. Harmless for a
# from-source install (it links against the user's own Ruby either way).
if RbConfig::CONFIG["target_os"] =~ /darwin/
  # Do not link the extension to the build Ruby's libruby.
  # Ruby C API symbols are resolved from the loading Ruby process.
  $LIBRUBYARG = ""
  $LIBRUBYARG_SHARED = ""
  $LIBRUBYARG_STATIC = ""
  $DLDFLAGS << " -undefined dynamic_lookup" # symbols come from the loading ruby
elsif RbConfig::CONFIG["target_os"] =~ /linux/
  $LIBRUBYARG = ""                          # the ruby executable already provides them
  $LIBRUBYARG_SHARED = ""
  $LIBRUBYARG_STATIC = ""
# else (mingw/mswin): intentionally NOT blanked. PE/COFF has no dynamic_lookup
# equivalent, so a MinGW extension MUST link the Ruby import library - we fall
# through to mkmf's default libruby link. The precompiled x64-mingw-ucrt gem
# still loads on any compatible Ruby of that ABI (they share ruby.dll's name).
end

# Export ONLY Init_makiri from the compiled extension. `-fvisibility=hidden`
# above hides our own sources' symbols, but the vendored Lexbor static library
# is built (by Lexbor's own CMake) with default visibility, so without this the
# linker re-exports ~1700 `lxb_*` / `lexbor_*` symbols into the bundle's dynamic
# table. Another Lexbor-based extension loaded in the same process (e.g.
# nokolexbor) would then resolve its own `lxb_*` calls to OUR copy - a different
# Lexbor version with an incompatible ABI - and segfault. Restricting the export
# list to Init_makiri keeps Makiri's Lexbor entirely private (Ruby only needs
# Init_makiri, found via dlsym at require time).
if RbConfig::CONFIG["target_os"] =~ /darwin/
  $DLDFLAGS << " -Wl,-exported_symbol,_Init_makiri"
elsif RbConfig::CONFIG["target_os"] =~ /linux/
  # Hide every symbol pulled in from static archives (the Lexbor .a); our own
  # are already hidden by -fvisibility=hidden, leaving just RUBY_FUNC_EXPORTED
  # Init_makiri in the dynamic symbol table.
  $DLDFLAGS << " -Wl,--exclude-libs,ALL"
elsif windows
  # PE DLLs export nothing unless __declspec(dllexport); Init_makiri (marked
  # RUBY_FUNC_EXPORTED) is the only such export, which already turns off MinGW
  # ld's auto-export-all. Belt-and-suspenders parity with the arms above: keep
  # the export table to Init_makiri so a co-loaded Lexbor-based ext (e.g.
  # nokolexbor) can't bind our private lxb_*.
  $DLDFLAGS << " -Wl,--exclude-all-symbols"
end

# Spike (opt-in): each MAKIRI_RUST_* flag replaces a set of C sources with the
# Rust port in ext/makiri/rust, which exports the same C ABI (symbols + struct
# layouts), so nothing else changes (notes/rust_rewrite_plan.ja.md §6-§7).
#
#   MAKIRI_RUST_XML=1        ext/makiri/xml/*.c              -> `xml`
#   MAKIRI_RUST_XPATH=1      the XPath front end             -> `xpath`
#                            (xpath/mkr_xpath_{lex,number,parse}.c)
#   MAKIRI_RUST_XPATH_XML=1  the XML engine instance         -> `xpath-xml`
#                            (xpath/mkr_xpath_engine_xml.c)
#   MAKIRI_RUST_XPATH_HTML=1 the HTML engine instance        -> `xpath-html`
#                            (xpath/mkr_xpath_engine_html.c)
#   MAKIRI_RUST_XPATH_DRIVER=1 the driver                    -> `xpath-driver`
#                            (xpath/mkr_xpath.c: context, budgets, evaluate)
#   MAKIRI_RUST_XPATH_SHARED=1 the shared primitives        -> `xpath-shared`
#                            (xpath/mkr_xpath_shared.c)
#   MAKIRI_RUST_GLUE_SERIALIZE=1 HTML serialization           -> `glue-serialize`
#                            (glue/ruby_html_serialize.c)
#   MAKIRI_RUST_GLUE_NODE=1  the shared node core              -> `glue-node`
#                            (glue/ruby_node.c)
#   MAKIRI_RUST_BRIDGE_STRING=1 the text-input contract       -> `bridge-string`
#                            (bridge/ruby_string.c)
#   MAKIRI_RUST_GLUE_NODE_SET=1 Makiri::NodeSet               -> `glue-node-set`
#                            (glue/ruby_node_set.c)
#   MAKIRI_RUST_GLUE_CSS=1   CSS selector queries             -> `glue-css`
#
#   MAKIRI_RUST_GLUE_LEXBOR_CSS=1 the stylesheet binding       -> `glue-lexbor-css`
#
#   MAKIRI_RUST_GLUE_DOC=1   Document + the fragment pipeline -> `glue-doc`
#                            (glue/ruby_html_css.c)
#   MAKIRI_RUST_BRIDGE_XML_DECODE=1 the XML input decode      -> `bridge-xml-decode`
#                            (bridge/xml_decode.c; implies bridge-string)
#   MAKIRI_RUST_GLUE_XML=1   the XML reader's Ruby boundary    -> `glue-xml`
#                            (glue/ruby_xml.c)
#   MAKIRI_RUST_GLUE_XPATH=1 the XPath engine's Ruby boundary   -> `glue-xpath`
#                            (glue/ruby_xpath.c)
#   MAKIRI_RUST_GLUE_XML_NODE_READ=1 the XML node's readers    -> `glue-xml-node-read`
#                            (glue/ruby_xml_node_read.c)
#   MAKIRI_RUST_GLUE_XML_NODE_SERIALIZE=1 its serializers     -> `glue-xml-node-serialize`
#                            (glue/ruby_xml_node_serialize.c)
#   MAKIRI_RUST_GLUE_XML_NODE_MUTATE=1 its mutation + factories -> `glue-xml-node-mutate`
#                            (glue/ruby_xml_node.c; implies the readers, since it
#                             owns mkr_init_xml_node)
#
# The glue flags differ from the engine ones in what they preserve. An engine
# flag swaps one C ABI for an identical one; a glue flag swaps C that calls Ruby
# for Rust that calls Ruby through magnus. What stays fixed is the registration
# seam: Init_makiri still defines every class and still calls the same
# mkr_init_<feature>(), whichever language provides it. They are also the only
# flags that pull a dependency (magnus + rb-sys) into the crate.
#
# The two instances are independent: either C engine can be replaced on its own,
# which is what makes a regression bisectable. Both imply the front end, because
# both halves would otherwise define the same symbols as the C files still in
# the build.
#
# cargo builds one staticlib into this build dir with the selected features and
# it is linked like the Lexbor archive; the replaced C sources are dropped from
# the object list. A feature is what keeps the archive free of the symbols its
# C counterpart still defines, so the two can never both be linked in.
# The port table, shared with the Rakefile (which tells CI and the container
# scripts the full configuration). See ext/makiri/rust_ports.rb.
require_relative "rust_ports"

RustPorts.check_paths!(EXT_DIR)
enabled = RustPorts.enabled(ENV)

rust_xpath_html = enabled.include?("MAKIRI_RUST_XPATH_HTML")
rust_xpath      = enabled.include?("MAKIRI_RUST_XPATH")

if enabled.any?
  features = RustPorts.features(enabled)
  # The Rust half of `rake oom`, gated by the SAME env var that defines
  # -DMKR_ALLOC_INJECT for the C sources. Not a RustPorts row: it replaces no
  # C file, it arms a hook in one. (It was lost once, when the table replaced
  # the per-flag `features <<` lines - and the sweep then reported
  # `css_stylesheet allocations=0`, which is the guard that caught it.)
  features << "alloc-inject" if alloc_inject
  cargo = find_executable("cargo") or abort "MAKIRI_RUST_* needs cargo on PATH."
  rust_target = File.join(Dir.pwd, "rust-target")

  # The sanitizer has to be asked for on the Rust side too: MAKIRI_SANITIZE only
  # ever reached $CFLAGS/$LDFLAGS, so the crate was built plain --release and the
  # whole ported half of the extension sat outside the net. ASan instruments
  # loads and stores at compile time; an uninstrumented Rust frame reading past a
  # red-zoned allocation has no check to trip (only the memcpy-family
  # interceptors would catch it). Four constraints shape what follows:
  #
  #  - UBSan does NOT exist for Rust. rustc's -Zsanitizer takes one of
  #    address/cfi/dataflow/hwaddress/kcfi/kernel-address/leak/memory/memtag/
  #    safestack/shadow-call-stack/thread/realtime - no `undefined`. So
  #    MAKIRI_SANITIZE=undefined stays a C-only mode, and that is permanent, not
  #    a gap we intend to close.
  #  - -Zsanitizer is unstable, hence the nightly toolchain. Release builds stay
  #    on stable; only this sanitizer mode needs nightly.
  #  - asan-stack=0 must hold on BOTH sides. The C side turns stack red zones off
  #    because CRuby's rb_raise unwinds via __builtin_longjmp, which ASan cannot
  #    intercept, leaving stale stack poison behind (see the comment on the
  #    $CFLAGS above, and docs/ci-crash/INVESTIGATION.md). The Rust glue raises
  #    the same way, so instrumenting its stack would reintroduce exactly that
  #    spurious report.
  #  - Only one ASan runtime may be linked. The .bundle already pulls clang's in
  #    via -fsanitize=address on $DLDFLAGS; -Zexternal-clangrt tells rustc to
  #    reference it instead of linking Rust's own copy.
  #
  # --target is passed so build scripts and proc macros (rb-sys runs bindgen)
  # build for the host uninstrumented; it also moves the archive under the
  # triple, hence rust_out_dir below.
  rust_env = {}
  rust_flags = []
  rust_triple = nil
  if sanitize.include?("address")
    rust_triple = `rustc -vV`[/^host:\s*(\S+)/, 1] rescue nil
    rust_triple or abort "MAKIRI_SANITIZE with a Rust build needs `rustc -vV` to report a host triple."
    unless system("rustup", "run", "nightly", "rustc", "--version",
                  out: File::NULL, err: File::NULL)
      abort "MAKIRI_SANITIZE=#{sanitize} with MAKIRI_RUST_* needs the nightly " \
            "toolchain (-Zsanitizer is unstable): rustup toolchain install nightly. " \
            "Refusing to build the crate uninstrumented - a green sanitizer run " \
            "that silently skips the Rust half is worse than no run."
    end
    rust_env["RUSTUP_TOOLCHAIN"] = "nightly"
    rust_flags << "-Zsanitizer=address" << "-Zexternal-clangrt" <<
                  "-Cllvm-args=-asan-stack=0"
    warn "makiri: building the Rust crate with -Zsanitizer=address (nightly, #{rust_triple})"
  end
  if sanitize.include?("undefined") && !sanitize.include?("address")
    warn "makiri: NOTE -fsanitize=undefined covers the C sources only. Rust has " \
         "no UBSan (-Zsanitizer has no `undefined`); the crate is built plain."
  end
  rust_env["RUSTFLAGS"] = rust_flags.join(" ") unless rust_flags.empty?

  warn "makiri: building the Rust engine (spike) via cargo: #{features.join(", ")}"
  cargo_argv = [cargo, "build", "--release", "--quiet",
                "--manifest-path", File.join(EXT_DIR, "rust", "Cargo.toml"),
                "--features", features.join(",")]
  cargo_argv += ["--target", rust_triple] if rust_triple
  cargo_argv += ["--target-dir", rust_target]
  system(rust_env, *cargo_argv) or abort "cargo build failed for the Rust engine."
  rust_out_dir = rust_triple ? File.join(rust_target, rust_triple, "release")
                             : File.join(rust_target, "release")
  rust_archive = File.join(rust_out_dir, "libmakiri_rs.a")
  if RbConfig::CONFIG["target_os"] =~ /linux/
    # GNU ld resolves static archives left-to-right and never looks back, and
    # the Lexbor archive is already on the line (above) - ahead of this one. The
    # engine features only ever referenced our own objects, so that was fine;
    # the moment Rust calls a Lexbor function (the serialization glue calls
    # lxb_html_serialize_*), the reference comes too late and the link leaves it
    # undefined. A shared object tolerates that, so it is not a link error: the
    # .so loads and dies with "undefined symbol: lxb_html_serialize_tree_cb".
    # A group makes ld re-scan until nothing is pending, in either direction.
    # macOS's linker re-scans archives on its own, hence no group there.
    $LDFLAGS << " -Wl,--start-group #{rust_archive.shellescape} " \
                "#{lexbor_archive.shellescape} -Wl,--end-group"
  else
    $LDFLAGS << " #{rust_archive.shellescape}"
  end
  # Lets the layout cross-check in xpath/mkr_xpath_rs_check.c compile itself in.
  $defs << "-DMAKIRI_RUST_XPATH=1" if rust_xpath
  # Compiles in the Lexbor shims + the layout cross-check the HTML backend needs.
  $defs << "-DMAKIRI_RUST_XPATH_HTML=1" if rust_xpath_html
end

# Recursively pick up C sources under ext/makiri/, excluding standalone
# libFuzzer harnesses. Those define LLVMFuzzerTestOneInput and are linked by
# ext/makiri/fuzz/Makefile, never into the Ruby extension.
# The C files this configuration replaces, as absolute paths.
#
# No `rescue` here, deliberately. This was written as `... .to_set rescue nil`
# with an `||= []` behind it, which means any exception - a missing require, a
# typo in the block - degrades to "drop nothing": the C stays in and the Rust is
# dead, silently. The safe direction for this expression is to STOP.
rust_dropped = RustPorts.replaced_sources(enabled).flat_map { |rel|
  rel == "xml/" ? Dir.glob(File.join(EXT_DIR, "xml", "*.c")) : [File.join(EXT_DIR, rel)]
}.to_set

if enabled.any? && rust_dropped.empty?
  abort "extconf: #{enabled.size} port flag(s) are set but no C source was " \
        "selected for replacement - the build would compile both halves."
end

$srcs = Dir.glob(File.join(EXT_DIR, "**", "*.c"))
           .reject { |f| f.start_with?(File.join(EXT_DIR, "fuzz") + File::SEPARATOR) }
           # Nothing under the Rust crate is a source of this extension. It holds
           # its own cargo-fuzz crate, whose C support files (an abort() stub for
           # the HTML engine entries) would otherwise be globbed in and collide
           # with the real definitions at link time.
           .reject { |f| f.start_with?(File.join(EXT_DIR, "rust") + File::SEPARATOR) }
           # One rule, from the same table the features come from.
           .reject { |f| rust_dropped.include?(f) }
           .map { |f| f.sub("#{EXT_DIR}/", "") }
$VPATH ||= []
# fuzz/ must be excluded here too: after a `rake fuzz:libfuzzer_build`,
# fuzz/build/{core,xml,xpath}/ hold sanitizer-instrumented .o files, and a
# VPATH that includes them lets make resolve the extension's object
# prerequisites there instead of compiling them - breaking the link (or worse,
# silently mixing differently-flagged objects).
$VPATH += Dir.glob(File.join(EXT_DIR, "**/"))
             .reject { |d| d.start_with?(File.join(EXT_DIR, "fuzz") + File::SEPARATOR) }
             .reject { |d| d.start_with?(File.join(EXT_DIR, "rust") + File::SEPARATOR) }
             .map { |d| "$(srcdir)/#{d.sub("#{EXT_DIR}/", "")}".chomp("/") }

create_makefile("makiri/makiri")

# mkmf's generated Makefile carries NO header dependencies, so editing a header
# (e.g. a struct layout in an internal .h) recompiles only the .c files whose
# own timestamps changed - the rest keep their stale layout and the objects
# silently disagree (ABI mismatch, runtime breakage). Append the coarsest sound
# rule instead: every object depends on every project header. A header edit
# then recompiles everything - a few seconds for this ext, and it can never
# rot (the list regenerates each configure; a header NEW since the last
# configure is reachable only from .c files edited to include it, which rebuild
# on their own timestamp). Ruby/Lexbor headers are deliberately excluded: a
# Ruby upgrade gets a fresh build dir from rake-compiler, and a Lexbor pin
# change already requires `rake clean:lexbor` (see CLAUDE.md).
project_headers = Dir.glob(File.join(EXT_DIR, "**", "*.h"))
                     .reject { |f| f.start_with?(File.join(EXT_DIR, "fuzz") + File::SEPARATOR) }
                     .reject { |f| f.start_with?(File.join(EXT_DIR, "rust") + File::SEPARATOR) }
                     .map { |f| "$(srcdir)/#{f.sub("#{EXT_DIR}/", "")}" }
                     .sort
File.open("Makefile", "a") do |mk|
  mk.puts
  mk.puts "# Project-header dependencies appended by extconf.rb (mkmf emits none)."
  mk.puts "$(OBJS): #{project_headers.join(" ")}"
end
