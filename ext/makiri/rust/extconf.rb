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
# Makiri is one Rust crate plus one vendored C dependency. This file builds
# Lexbor (unpatched, through its own cmake) and then hands the crate to cargo.
# There is no C of ours left to compile, so there is no mkmf object list, no
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

CRATE_DIR = __dir__
GEM_ROOT  = File.expand_path("../../..", CRATE_DIR)
LEXBOR_SRC = File.join(GEM_ROOT, "vendor", "lexbor")
LEXBOR_BLD = File.join(LEXBOR_SRC, "build")
LEXBOR_DST = File.join(LEXBOR_SRC, "dist")

unless File.directory?(LEXBOR_SRC)
  abort "Lexbor source not found at #{LEXBOR_SRC}. Did you `git submodule update --init`?"
end

cmake = find_executable("cmake") or abort "cmake is required to build Lexbor."

# Windows (RubyInstaller / MinGW-UCRT gcc): forces the GNU toolchain for the
# Lexbor build below.
windows = RbConfig::CONFIG["target_os"] =~ /mingw|mswin/
darwin  = RbConfig::CONFIG["target_os"] =~ /darwin/
linux   = RbConfig::CONFIG["target_os"] =~ /linux/

# Optionally build the vendored Lexbor itself under AddressSanitizer. This is
# the ONLY way to catch overflows *inside* Lexbor's bump (mraw) arena: a
# sub-allocation overrunning into the next one stays within one big malloc'd
# chunk, so the heap allocator's red-zones (and thus a plain ASan build of just
# our crate) never see it. Lexbor's own mraw is ASan-aware - with
# -DLEXBOR_BUILD_WITH_ASAN=ON it poisons the arena and unpoisons each
# allocation, so an intra-arena overrun writes into poisoned memory and ASan
# reports it. Opt-in (slow full rebuild), only meaningful with
# MAKIRI_SANITIZE=...address...; drive it via `rake sanitize:lexbor`.
# vendor/lexbor stays vanilla - this is a build flag, not a source patch.
sanitize    = ENV["MAKIRI_SANITIZE"].to_s.strip
lexbor_asan = !ENV["MAKIRI_SANITIZE_LEXBOR"].to_s.strip.empty? && sanitize.include?("address")

# Link-time optimization across Lexbor's own translation units, DARWIN ONLY.
#
# It is the one compile-option win there was: measured -17% on parse and -10%
# on to_html (CPU time, 6000-element document), for about two seconds of link.
# Lexbor is already -O3 - `CMAKE_BUILD_TYPE=Release` appends `-O3 -DNDEBUG`
# AFTER its own `-O2`, and the last one wins - so there was nothing left in the
# -O level.
#
# Darwin only because the archive stops being ordinary objects, and what can
# read it afterwards is a property of each platform's linker. CI found all
# three answers, so do not "simplify" this to every platform:
#
#   darwin  ld64 reads the bitcode natively. Extension AND `cargo test` link.
#   linux   the extension links (gcc drives it, with its LTO plugin), but
#           `cargo test` uses rust-lld, which cannot read GCC's GIMPLE at all.
#   mingw   BFD ld fails outright: cmake indexes the archive with plain `ar`,
#           which records no LTO symbols, so every `lxb_*` is undefined.
#
# Both could in principle be chased - `gcc-ar` for the index, clang for the
# Linux Lexbor build so lld sees bitcode - but each turns on a version match
# between toolchains we do not control. The measured win stays where it is
# measured. `MAKIRI_LEXBOR_NO_LTO=1` opts out of it.
#
# NOT under the sanitizer: that build exists to find bugs, where inlining across
# the whole archive only makes a report harder to read.
#
# Linux needs the WHOLE chain to be LLVM, not just the compile: `cargo test`
# links with rust-lld, which reads LLVM bitcode and not GCC's GIMPLE, so clang
# has to produce the archive, llvm-ar has to index it (plain `ar` records no
# LTO symbols - that is what broke mingw), and clang has to drive the final
# link. Missing any of those, LTO is simply off: this DETECTS the toolchain
# rather than requiring it, so a machine with only gcc builds exactly as before
# instead of failing.
def llvm_chain_ok?
  return false unless find_executable("clang")
  return false unless find_executable("llvm-ar") && find_executable("llvm-ranlib")

  # clang knowing the NAME lld is not the same as lld being installed.
  probe = File.join(Dir.tmpdir, "makiri_lld_probe_#{Process.pid}")
  File.write("#{probe}.c", "int main(void){return 0;}\n")
  ok = system("clang", "-fuse-ld=lld", "#{probe}.c", "-o", probe,
              out: File::NULL, err: File::NULL)
  ok
ensure
  FileUtils.rm_f(["#{probe}.c", probe]) if probe
end

lexbor_lto =
  if lexbor_asan || !ENV["MAKIRI_LEXBOR_NO_LTO"].to_s.strip.empty?
    false
  elsif darwin
    true
  elsif linux
    llvm_chain_ok?
  else
    false # mingw: BFD ld cannot read the archive at all - see above
  end
lexbor_lto_llvm = lexbor_lto && !darwin # darwin's own toolchain needs no help
lexbor_mode = if lexbor_asan
                "asan"
              elsif lexbor_lto_llvm
                "plain-lto-llvm"
              elsif lexbor_lto
                "plain-lto"
              else
                "plain"
              end
lexbor_stamp = File.join(LEXBOR_DST, ".makiri_build_mode")

# Reuse the cached archive only when it was built in the mode we now want; a
# mode switch (plain <-> asan <-> plain-lto) forces a rebuild, so a sanitized or
# non-LTO Lexbor can never leak into a build that wanted the other.
have_archive = File.exist?(File.join(LEXBOR_DST, "lib", "liblexbor_static.a"))
stamp_ok = have_archive && File.exist?(lexbor_stamp) && File.read(lexbor_stamp).strip == lexbor_mode
unless stamp_ok
  FileUtils.rm_rf(LEXBOR_BLD)
  FileUtils.rm_rf(LEXBOR_DST) if have_archive # drop a wrong-mode install
  FileUtils.mkdir_p(LEXBOR_BLD)
  Dir.chdir(LEXBOR_BLD) do
    cmd = [
      cmake,
      # On Windows the runner ships Visual Studio, so a bare cmake defaults to
      # the MSVC generator -> lexbor_static.lib under a Release/ subdir,
      # breaking the hardcoded liblexbor_static.a path below. Force the GNU
      # toolchain via Ninja (bundled by lukka/get-cmake; single-config, so no
      # Release/ subdir) and pin the MinGW-UCRT gcc so cmake can't auto-probe
      # MSVC. "MinGW Makefiles" is avoided: cmake refuses it when sh.exe is on
      # PATH (MSYS2/Git-Bash put it there).
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
      *(lexbor_lto ? ["-DLEXBOR_C_FLAGS=#{lexbor_lto_llvm ? "-flto=thin" : "-flto"}"] : []),
      *(lexbor_lto_llvm ? ["-DCMAKE_C_COMPILER=clang", "-DCMAKE_AR=#{`which llvm-ar`.strip}",
                           "-DCMAKE_RANLIB=#{`which llvm-ranlib`.strip}"] : []),
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

lexbor_include = File.join(LEXBOR_DST, "include")
lexbor_archive = File.join(LEXBOR_DST, "lib", "liblexbor_static.a")

# ---------------------------------------------------------------------------
# What the crate is built with
# ---------------------------------------------------------------------------

# The default feature set is the extension: `ruby` (which implies `lexbor`).
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
# macros (rb-sys runs bindgen), which build for the host and have no business
# linking Lexbor.
rustc_args = []

# Hard-link the static archive rather than pass -L/-llexbor_static, to avoid
# accidentally linking a system-installed Lexbor.
rustc_args += ["-C", "link-arg=#{lexbor_archive}"]

# Windows: the vendored Lexbor calls CRT functions (strncmp in the HTML initial
# insertion mode, &c.), but rustc's windows-gnu cdylib link runs gcc with
# `-nodefaultlibs` and a hardcoded CRT list (-lmsvcrt -lmingwex -lgcc ...).
# RubyInstaller's Ruby is a UCRT build and its toolchain's CRT is libucrt.a,
# which is NOT in that list, and our archive sits at the END of the link line -
# so anything Lexbor references has to be resolved by a library that appears
# AFTER it. References the Rust std already pulled from an earlier archive are
# incidentally satisfied; the rest fail with "undefined reference" (observed:
# strncmp, the only CRT symbol nothing before Lexbor needed). Re-pass the CRT
# import after the archive so ld's single pass sees it. (The gnullvm target for
# aarch64 Ruby already links libucrt through clang's own specs; the -lucrt there
# is a harmless duplicate.)
if windows
  rustc_args += ["-C", "link-arg=#{RUBY_PLATFORM =~ /mingw32/ ? "-lmsvcrt" : "-lucrt"}"]
end

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

  # With an LLVM-bitcode Lexbor the link has to be LLVM too: gcc's plugin reads
  # GIMPLE, not bitcode. `cargo test` already uses rust-lld and needs nothing
  # here; this is for the extension, which rb_sys otherwise links with Ruby's
  # own CC.
  if lexbor_lto_llvm
    rustc_args += ["-C", "linker=clang", "-C", "link-arg=-fuse-ld=lld"]
  end
end

# Flags that must reach the crate AND its dependencies (magnus, rb-sys), which
# rustc args cannot do - these are the ones that genuinely belong in RUSTFLAGS.
rustflags = []
cargo_env = { "MAKIRI_LEXBOR_INCLUDE" => lexbor_include }
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
#    shadow - a spurious report that ASan then aborts on while rendering (see
#    docs/ci-crash/INVESTIGATION.md). Heap red zones and the arena poisoning in
#    xml::node are unaffected; only stack-buffer checks are lost.
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
    # Only one ASan runtime may be linked; reference clang's rather than
    # linking Rust's own copy.
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
# source-based coverage (the vendored Lexbor is built separately and is NOT
# instrumented - we measure only the code we write). Run via `rake coverage`.
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
  # The C was built -O2. Match it: a debug-profile extension is several times
  # slower, and every benchmark in this project is stated against a release one.
  r.profile = (ENV["MAKIRI_CARGO_PROFILE"] || "release").to_sym
end

# ---------------------------------------------------------------------------
# Export ONLY Init_makiri
# ---------------------------------------------------------------------------
#
# The hazard is unchanged and is not hypothetical: the vendored Lexbor archive
# is built with default visibility, so a bundle that re-exports its ~1700
# `lxb_*` symbols lets another Lexbor-based extension in the same process (e.g.
# nokolexbor) resolve its own `lxb_*` calls to OUR copy - a different Lexbor
# version with an incompatible ABI - and segfault.
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

# MAKIRI_NO_EXPORT_TRIM=1 skips the trim entirely. It exists to answer one
# question - whether the trim is what makes the ASan build segfault at load -
# and a build made with it exports every `mkr_*` name, so `rake symbols` will
# (correctly) fail on it. Diagnostic only; not a supported configuration.
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
