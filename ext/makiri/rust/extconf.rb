# frozen_string_literal: true

require "mkmf"
require "rbconfig"
require "fileutils"
require "shellwords"
require "etc"
require "tmpdir"
require "rb_sys/mkmf"

# extconf for the Makiri extension.
#
# Makiri's extension is a Rust crate (`makiri_rs`), and this file hands it to
# cargo; it builds the engine (`makiri_engine`), the fallible allocation it
# allocates through (`falloc`) and the binding layer (`nupur`) with it, from the
# one workspace. The vendored Lexbor is not part of it: it is the reference the
# ports are tested against (`reference/`, a workspace of its own), built by
# `rake lexbor` (script/build_lexbor.rb) for the tests alone. There is no C left to compile, so there is no mkmf object list, no
# $srcs, and no $CFLAGS: cargo performs the final link, and everything that used
# to be a compiler or linker flag is now either a rustc argument or gone.
#
# What "or gone" covers, stated plainly rather than quietly dropped:
#
#   -D_FORTIFY_SOURCE=2, -fstack-protector-strong, -Wformat-security, -fno-common
#     were hardening for C sources. They have no subject any more - not because
#     the protection was dropped, but because the code they protected does not
#     exist. Rust has no equivalent flag and needs none for the classes they
#     addressed (unchecked formatting, stack smashing through unbounded copies).
#
#   -fvisibility=hidden
#     its JOB survives - see the export restriction at the bottom of this file -
#     but the mechanism changes, because rustc decides a cdylib's export list.
#
# Requires cargo. That is deliberate and was decided when the C was retired:
# binary gems are built for the platforms that matter, and a source install
# needs a Rust toolchain.

windows = RbConfig::CONFIG["target_os"] =~ /mingw|mswin/
darwin  = RbConfig::CONFIG["target_os"] =~ /darwin/
linux   = RbConfig::CONFIG["target_os"] =~ /linux/

sanitize = ENV["MAKIRI_SANITIZE"].to_s.strip

# ---------------------------------------------------------------------------
# What the crate is built with
# ---------------------------------------------------------------------------

# The extension has one feature, the injection hook below; the engine's
# default (the arena HTML DOM) comes with it.
features = []

# OOM-injection build (opt-in): MAKIRI_ALLOC_INJECT=1 compiles the allocation
# failure hook so `rake oom` can sweep "the nth core allocation fails" over
# representative workloads and assert every OOM branch fails closed.
# Debug/test builds only - a normal build carries no hook.
if ENV["MAKIRI_ALLOC_INJECT"].to_s.strip == "1"
  features << "alloc-inject"
  warn "makiri: building with allocation-failure injection (MKR_ALLOC_INJECT)"
end

# Arguments for the FINAL crate only, passed after `cargo rustc --`. They must
# not go through RUSTFLAGS: that would also apply them to build scripts and proc
# macros (rb-sys runs bindgen), which build for the host.
rustc_args = []

# Nothing is added here for macOS's `-undefined dynamic_lookup`: rb_sys already
# passes it, so Ruby C API symbols are resolved from the loading process and one
# compiled binary works on any compatible Ruby of that ABI. (The crate takes
# rb-sys deliberately WITHOUT `link-ruby` for the same reason.) Adding a second
# copy was harmless but implied we owned a decision that belongs to rb_sys.
if linux
  # Relocation hardening for the shared object. These are the two C-era
  # $DLDFLAGS that still have a subject: they are properties of the link, not of
  # the language that produced the objects.
  rustc_args += ["-C", "link-arg=-Wl,-z,relro", "-C", "link-arg=-Wl,-z,now"]
end

# Flags that must reach the crate AND its dependencies (rb-sys), which
# rustc args cannot do - these are the ones that genuinely belong in RUSTFLAGS.
rustflags = []
cargo_env = {}
cargo_target = nil

# Sanitizer build (opt-in): MAKIRI_SANITIZE=address rake clean compile, then run
# the suite under the runtime via `rake sanitize` (which preloads it).
#
# Four constraints shape what follows, and all four predate this file:
#
#  - UBSan does NOT exist for Rust. rustc's -Zsanitizer takes one of
#    address/cfi/dataflow/hwaddress/kcfi/kernel-address/leak/memory/memtag/
#    safestack/shadow-call-stack/thread/realtime - no `undefined`. With the C
#    gone, MAKIRI_SANITIZE=undefined no longer has anything to instrument.
#  - -Zsanitizer is unstable, hence the nightly toolchain. Release builds stay
#    on stable; only this mode needs nightly.
#  - asan-stack=0. CRuby is built with RUBY_SETJMP = __builtin_setjmp, so a
#    raise unwinds via __builtin_longjmp, which ASan cannot intercept: a raise
#    crossing an instrumented frame leaves that frame's stack poison behind, and
#    a later interceptor in the uninstrumented interpreter trips over the stale
#    shadow - a spurious report that ASan then aborts on while rendering. Heap
#    red zones are unaffected; only stack-buffer checks are lost.
#  - --target is passed so build scripts and proc macros build for the host
#    uninstrumented.
unless sanitize.empty?
  if sanitize.include?("address")
    cargo_target = `rustc -vV`[/^host:\s*(\S+)/, 1] rescue nil
    cargo_target or abort "MAKIRI_SANITIZE needs `rustc -vV` to report a host triple."
    unless system("rustup", "run", "nightly", "rustc", "--version",
                  out: File::NULL, err: File::NULL)
      abort "MAKIRI_SANITIZE=#{sanitize} needs the nightly toolchain " \
            "(-Zsanitizer is unstable): rustup toolchain install nightly."
    end
    cargo_env["RUSTUP_TOOLCHAIN"] = "nightly"
    rustflags += ["-Zsanitizer=address", "-Cllvm-args=-asan-stack=0"]
    # Only one ASan runtime may be linked, and it is rustc's own.
    # NOT -Zexternal-clangrt. That flag says "do not link a runtime, one is
    # already here", and while the C was compiled that was true: `$DLDFLAGS`
    # carried `-fsanitize=address`, so clang linked its runtime into the bundle
    # and rustc had to be told not to add a second. There is no C link step any
    # more, so the flag now means "link no runtime at all" - the bundle's
    # `__asan_*` are all undefined and it depends entirely on the preload, which
    # is a fragile shape for an image dlopen'd late into an uninstrumented host.
    # Letting rustc link Rust's own runtime is what a normal sanitized Rust
    # build does.
    if !ENV["MAKIRI_EXTERNAL_CLANGRT"].to_s.empty?
      rustflags << "-Zexternal-clangrt"
      warn "makiri: -Zexternal-clangrt (no runtime linked; needs the preload)"
    end
    warn "makiri: building with -Zsanitizer=address (nightly, #{cargo_target})"
  else
    abort "MAKIRI_SANITIZE=#{sanitize}: with the C retired, only `address` has " \
          "an implementation here (Rust has no UBSan). Refusing to build " \
          "uninstrumented under a sanitizer request - a green run that covers " \
          "nothing is worse than no run."
  end
end

# Coverage build (opt-in): MAKIRI_COVERAGE=1 instruments the crate with LLVM
# source-based coverage. Run via `rake coverage`.
unless ENV["MAKIRI_COVERAGE"].to_s.strip.empty?
  rustflags << "-Cinstrument-coverage"
  warn "makiri: building with LLVM source-based coverage"
end

# Windows: match Ruby's ABI, or rb-sys refuses to generate bindings.
#
# RubyInstaller's Ruby is built for target_os="mingw32"/"ucrt", while a default
# `rustup` on Windows installs the MSVC host toolchain - and rb-sys will not
# generate bindings from Ruby's headers for a different operating-system ABI
# ("Ruby was built for target_os=\"mingw32\", but Cargo is compiling for
# x86_64-pc-windows-msvc"). So name the GNU target explicitly.
#
# The triple is asked of rb_sys rather than written here: it already maps every
# gem platform to its Rust target (x64-mingw-ucrt -> x86_64-pc-windows-gnu,
# aarch64-mingw-ucrt -> aarch64-pc-windows-gnullvm, x86-mingw32 -> i686-...),
# and hand-rolling that table is how the aarch64 and 32-bit cases get missed.
#
# The toolchain must actually HAVE that target (`rustup target add`); CI installs
# it, and a source install on Windows needs it too.
if windows && cargo_target.nil?
  begin
    # `rb_sys/mkmf` alone is NOT enough: ToolchainInfo#initialize reads
    # RbSys::VERSION, which only `rb_sys` pulls in. Getting this wrong is silent
    # - the rescue fires, no target is set, and the build dies later with the
    # ABI message this block exists to prevent.
    require "rb_sys"
    require "rb_sys/toolchain_info"
    cargo_target = RbSys::ToolchainInfo.local.rust_target
    warn "makiri: Ruby is a MinGW build; targeting #{cargo_target}"
  rescue StandardError, LoadError => e
    # Fail closed: proceeding would hit rb-sys's ABI error instead, which says
    # nothing about how this build chose its target.
    abort "makiri: cannot resolve the Rust target for #{RUBY_PLATFORM} " \
          "(#{e.class}: #{e.message}). Ruby here is a MinGW build, so cargo must " \
          "be told a *-pc-windows-gnu target; set RUST_TARGET and retry."
  end
end

create_rust_makefile("makiri/makiri") do |r|
  r.features = features
  r.extra_rustc_args = rustc_args
  r.extra_rustflags = rustflags
  r.env = cargo_env
  r.target = cargo_target if cargo_target
  # The extension alone, as a `cdylib`: with an `rlib` in the build rustc runs
  # no LTO ("lto can only be run for executables, cdylibs and static library
  # outputs"), and every call into the engine and the binding layer, crates
  # of their own, would be an opaque cross-crate call - the light readers
  # measured 9-25% slower when `nupur` was split off. With `cdylib` alone, the
  # release profile's `lto = true` applies. (The manifest lists `cdylib`
  # only; this says it again, so a type added there cannot undo it.)
  r.extra_cargo_args = ["--crate-type", "cdylib"]
  # The C was built -O2. Match it: a debug-profile extension is several times
  # slower, and every benchmark in this project is stated against a release one.
  r.profile = (ENV["MAKIRI_CARGO_PROFILE"] || "release").to_sym
end

# ---------------------------------------------------------------------------
# Export ONLY Init_makiri
# ---------------------------------------------------------------------------
#
# The hazard was not hypothetical while the extension linked the vendored
# Lexbor: its archive is built with default visibility, so a bundle that
# re-exported its ~1700 `lxb_*` symbols let another Lexbor-based extension in
# the same process (e.g. nokolexbor) resolve its own `lxb_*` calls to OUR copy -
# a different Lexbor version with an incompatible ABI - and segfault. No
# Lexbor is linked any more, and an exported name of our own is the same
# hazard for whatever binds it, so the restriction stays.
#
# What changed is where it is enforced. Under mkmf this was a link flag
# (-Wl,-exported_symbol / --exclude-libs,ALL). rustc performs the cdylib link
# itself and ALWAYS passes its own -exported_symbols_list built from the crate's
# `#[no_mangle]` items, so an added flag is either merged (no restriction) or
# refused outright - `ld: -unexported_symbol cannot be used with
# -exported_symbol*`. Both were measured before settling on a post-link trim.
#
# `_ruby_abi_version` stays: Ruby looks it up at require time to check the
# extension was built for this ABI. Dropping it does not fail loudly - it
# removes a check - so it is listed explicitly rather than left to luck.
#
# What actually enforces the restriction is the SOURCE: only `Init_makiri` is
# `#[no_mangle]`, so no other name is emitted to export. This file's trim used to
# be the mechanism, and that was wrong on Linux - `objcopy --keep-global-symbol`
# cannot remove an entry from a linked shared object's `.dynsym`, so 222 `mkr_*`
# names shipped exported there while macOS looked clean. The trim stays as a
# second line on macOS, where `strip -u -r -s` does work (it prints "removing
# global symbols from a final linked no longer supported" and works anyway).
# `rake symbols` asserts the outcome, so
# that day arrives as a failing gate rather than as a silent re-export.
keep = File.join(Dir.pwd, "makiri-exported.sym")

# MAKIRI_NO_EXPORT_TRIM=1 skips the trim entirely, to tell whether the trim is
# what changes a build's behaviour at load. Since nothing but `Init_makiri` is
# `#[no_mangle]`, a build made with it should export nothing more; `rake
# symbols` is what says so. Diagnostic only; not a supported configuration.
restrict =
  if !ENV["MAKIRI_NO_EXPORT_TRIM"].to_s.empty?
    warn "makiri: SKIPPING the export trim (MAKIRI_NO_EXPORT_TRIM) - diagnostic build"
    nil
  elsif darwin
    File.write(keep, "_Init_makiri\n_ruby_abi_version\n")
    "strip -u -r -s #{keep.shellescape} $(DLLIB)"
  elsif linux
    # objcopy, for the same reason: rustc owns the version script at link time.
    "objcopy --keep-global-symbol=Init_makiri " \
      "--keep-global-symbol=ruby_abi_version $(DLLIB)"
  end

if restrict
  File.open("Makefile", "a") do |mk|
    mk.puts
    mk.puts "# Restrict the export table to Init_makiri (see extconf.rb)."
    mk.puts "all: makiri-restrict-exports"
    mk.puts ".PHONY: makiri-restrict-exports"
    mk.puts "makiri-restrict-exports: $(DLLIB)"
    mk.puts "\t$(Q) #{restrict}"
  end
end
