# frozen_string_literal: true

# Builds the vendored Lexbor (`rake lexbor`): the reference the ports are tested
# against - the Rust tests with the `lexbor` feature, the differential fuzz
# targets and `rake conformance:parser`. Nothing that ships links it, so this is
# a plain static build through Lexbor's own cmake, unpatched, installed into
# vendor/lexbor/dist, where build.rs finds its headers and archive.
#
# The install is reused while its stamp says it was built by this script; a
# different Lexbor commit still needs `rake clean:lexbor` (the stamp tracks the
# mode, not the revision).

require "etc"
require "fileutils"
require "rbconfig"

GEM_ROOT   = File.expand_path("..", __dir__)
LEXBOR_SRC = File.join(GEM_ROOT, "vendor", "lexbor")
LEXBOR_BLD = File.join(LEXBOR_SRC, "build")
LEXBOR_DST = File.join(LEXBOR_SRC, "dist")
MODE       = "reference"

unless File.directory?(File.join(LEXBOR_SRC, "source"))
  abort "Lexbor source not found at #{LEXBOR_SRC}. Did you `git submodule update --init`?"
end

stamp = File.join(LEXBOR_DST, ".makiri_build_mode")
archive = File.join(LEXBOR_DST, "lib", "liblexbor_static.a")
exit 0 if File.exist?(archive) && File.exist?(stamp) && File.read(stamp).strip == MODE

cmake = ENV.fetch("CMAKE", "cmake")
system(cmake, "--version", out: File::NULL) or abort "cmake is required to build Lexbor."
windows = RbConfig::CONFIG["target_os"] =~ /mingw|mswin/

FileUtils.rm_rf(LEXBOR_BLD)
FileUtils.rm_rf(LEXBOR_DST)
FileUtils.mkdir_p(LEXBOR_BLD)
Dir.chdir(LEXBOR_BLD) do
  cmd = [
    cmake,
    # A bare cmake on Windows defaults to the MSVC generator, which puts the
    # archive under Release/ as lexbor_static.lib; Ninja and MinGW gcc keep
    # build.rs's path.
    *(windows ? ["-G", "Ninja", "-DCMAKE_C_COMPILER=gcc"] : []),
    "-DLEXBOR_BUILD_SHARED=OFF",
    "-DLEXBOR_BUILD_STATIC=ON",
    "-DLEXBOR_BUILD_TESTS=OFF",
    "-DLEXBOR_BUILD_EXAMPLES=OFF",
    "-DLEXBOR_BUILD_UTILS=OFF",
    "-DCMAKE_BUILD_TYPE=Release",
    "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
    "-DCMAKE_INSTALL_PREFIX=#{LEXBOR_DST}",
    LEXBOR_SRC,
  ]
  warn "makiri: building vendored Lexbor (the reference)"
  # Multi-arg system() passes every -DNAME=VALUE verbatim, on Windows too.
  system(*cmd) or abort "cmake configure failed for Lexbor."
  nproc = Etc.respond_to?(:nprocessors) ? Etc.nprocessors : 4
  system(cmake, "--build", ".", "--target", "install", "--parallel", nproc.to_s) or
    abort "cmake build/install failed for Lexbor."
end
File.write(stamp, MODE)
