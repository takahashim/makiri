# frozen_string_literal: true

require "mkmf"
require "rbconfig"
require "fileutils"
require "shellwords"
require "etc"

# extconf for the Makiri C extension.
#
# Build strategy (see design_doc §12):
#   1. Build vendored Lexbor (unpatched) via cmake into vendor/lexbor/build,
#      install headers + a static archive into vendor/lexbor/dist.
#   2. Compile ext/makiri/**/*.c with rake-compiler, linking against the
#      static Lexbor archive only — no system libxml2/libxslt.
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

unless File.exist?(File.join(LEXBOR_DST, "lib", "liblexbor_static.a"))
  FileUtils.mkdir_p(LEXBOR_BLD)
  Dir.chdir(LEXBOR_BLD) do
    cmd = [
      cmake,
      "-DLEXBOR_BUILD_SHARED=OFF",
      "-DLEXBOR_BUILD_STATIC=ON",
      "-DLEXBOR_BUILD_TESTS=OFF",
      "-DLEXBOR_BUILD_EXAMPLES=OFF",
      "-DLEXBOR_BUILD_UTILS=OFF",
      "-DCMAKE_BUILD_TYPE=Release",
      "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
      "-DCMAKE_INSTALL_PREFIX=#{LEXBOR_DST}",
      LEXBOR_SRC,
    ].shelljoin
    system(cmd) or abort "cmake configure failed for Lexbor."
    system("#{cmake.shellescape} --build . --target install -- -j#{Etc.respond_to?(:nprocessors) ? Etc.nprocessors : 4}") or
      abort "cmake build/install failed for Lexbor."
  end
end

$INCFLAGS << " -I#{File.join(LEXBOR_DST, 'include').shellescape}"
$INCFLAGS << " -I#{File.join(EXT_DIR).shellescape}"

# Hard-link the static archive rather than pass -L/-llexbor_static, to
# avoid accidentally linking a system-installed Lexbor.
lexbor_archive = File.join(LEXBOR_DST, "lib", "liblexbor_static.a")
$LDFLAGS << " #{lexbor_archive.shellescape}"

# Sanitizer build (opt-in): MAKIRI_SANITIZE=address,undefined rake clean compile
# Then run the suite under the runtime via `rake sanitize` (which preloads the
# ASan runtime). Sanitizers replace the heap allocator, so even the vendored
# (uninstrumented) Lexbor's allocations get red-zoned — heap overflows on
# Lexbor-owned buffers are still caught. _FORTIFY_SOURCE is dropped here because
# it conflicts with the sanitizer interceptors.
sanitize = ENV["MAKIRI_SANITIZE"].to_s.strip
if sanitize.empty?
  # Security hardening flags. Keep -O2 active so _FORTIFY_SOURCE works.
  $CFLAGS << " -O2"
  $CFLAGS << " -D_FORTIFY_SOURCE=2"
else
  $CFLAGS << " -O1 -g -fno-omit-frame-pointer -fsanitize=#{sanitize}"
  $LDFLAGS  << " -fsanitize=#{sanitize}"
  $DLDFLAGS << " -fsanitize=#{sanitize}"
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
  $LIBRUBYARG = ""                          # drop the libruby link
  $DLDFLAGS << " -undefined dynamic_lookup" # symbols come from the loading ruby
elsif RbConfig::CONFIG["target_os"] =~ /linux/
  $LIBRUBYARG = ""                          # the ruby executable already provides them
end

# Recursively pick up C sources under ext/makiri/.
$srcs = Dir.glob(File.join(EXT_DIR, "**", "*.c")).map { |f| f.sub("#{EXT_DIR}/", "") }
$VPATH ||= []
$VPATH += Dir.glob(File.join(EXT_DIR, "**/")).map { |d| "$(srcdir)/#{d.sub("#{EXT_DIR}/", "")}".chomp("/") }

create_makefile("makiri/makiri")
