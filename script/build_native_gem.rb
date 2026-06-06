#!/usr/bin/env ruby
# frozen_string_literal: true

# Assemble a precompiled ("native" / fat) gem for one platform from the
# per-Ruby-ABI extension binaries already staged under
#   lib/makiri/<ruby_minor>/makiri.{so,bundle}
#
# Usage: ruby script/build_native_gem.rb <gem-platform>
#   e.g. ruby script/build_native_gem.rb arm64-darwin
#
# Run by .github/workflows/release.yml after downloading the compile artifacts.
# The resulting gem ships the compiled binaries (one per Ruby minor version) and
# declares no C extension, so `gem install` does NOT recompile or need cmake /
# the Lexbor submodule. lib/makiri.rb already selects lib/makiri/<minor>/makiri
# at require time.

require "rubygems"
require "rubygems/package"

platform = ARGV[0]
abort "usage: build_native_gem.rb <gem-platform>" if platform.nil? || platform.empty?

root = File.expand_path("..", __dir__)
spec = Gem::Specification.load(File.join(root, "makiri.gemspec"))

libs = Dir[File.join(root, "lib", "makiri", "*", "makiri.{so,bundle}")].sort
abort "no precompiled libraries found under lib/makiri/*/ - stage them first" if libs.empty?

# Native gem: ship binaries, not the C sources or the vendored Lexbor tree, and
# declare no extension so install never tries to compile.
spec.platform   = platform
spec.extensions = []
spec.files = spec.files.reject { |f| f.start_with?("ext/", "vendor/") }
spec.files += libs.map { |p| p.sub("#{root}/", "") }
spec.files.uniq!

# Bound the Ruby versions this binary gem serves - one subdir per ABI minor.
abis = libs.map { |p| File.basename(File.dirname(p)) }.sort_by { |v| v.split(".").map(&:to_i) }
lo_major, lo_minor = abis.first.split(".").map(&:to_i)
hi_major, hi_minor = abis.last.split(".").map(&:to_i)
spec.required_ruby_version = [">= #{lo_major}.#{lo_minor}.0", "< #{hi_major}.#{hi_minor + 1}.dev"]

puts "Building native gem for #{platform}"
puts "  ABIs:    #{abis.join(', ')}"
puts "  ruby:    #{spec.required_ruby_version}"
puts "  binaries:"
libs.each { |p| puts "    #{p.sub("#{root}/", '')}" }

gem = Gem::Package.build(spec)
puts "Built #{gem}"
