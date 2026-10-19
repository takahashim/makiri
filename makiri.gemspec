# frozen_string_literal: true

require_relative "lib/makiri/version"

Gem::Specification.new do |spec|
  spec.name        = "makiri"
  spec.version     = Makiri::VERSION
  spec.authors     = ["takahashim"]
  spec.email       = ["takahashimm@gmail.com"]
  spec.license     = "Apache-2.0"

  spec.summary     = "HTML5 parser + native XPath 1.0 for Ruby, with no libxml2 dependency."
  spec.description = <<~DESC
    Makiri parses HTML5 documents via the Lexbor library
    and queries them with a native XPath 1.0 engine written for this project.
    It does not depend on libxml2 at any layer. The API is
    Nokogiri-compatible for the subset of methods used in HTML scraping.
  DESC

  spec.homepage             = "https://github.com/takahashim/makiri"
  spec.required_ruby_version = ">= 3.2.0"

  spec.metadata["homepage_uri"]      = spec.homepage
  # spec.metadata["source_code_uri"]   = spec.homepage
  spec.metadata["bug_tracker_uri"]   = "#{spec.homepage}/issues"
  spec.metadata["changelog_uri"]     = "#{spec.homepage}/blob/main/CHANGELOG.md"
  spec.metadata["rubygems_mfa_required"] = "true"

  # The extension builds from the crate alone: the HTML and CSS parsers are
  # Rust ports of Lexbor's, and the vendored Lexbor (a git submodule) is only
  # the reference the tests compare them with (`rake lexbor`). So of `vendor/lexbor` the gem carries only what the ports owe it:
  # Lexbor's LICENSE and NOTICE (Apache-2.0, see NOTICE). `--recurse-submodules`
  # is what lists those two files; `git ls-files` alone reports the submodule
  # as one gitlink entry.
  lexbor_kept = %w[vendor/lexbor/LICENSE vendor/lexbor/NOTICE]
  gemspec = File.basename(__FILE__)
  spec.files = IO.popen(%w[git ls-files -z --recurse-submodules], chdir: __dir__, err: IO::NULL) do |ls|
    ls.readlines("\x0", chomp: true).reject do |f|
      (f == gemspec) ||
        f.start_with?(*%w[bin/ Gemfile .gitignore .rspec spec/ test/ bench/ docs/ CLAUDE.md AGENTS.md]) ||
        (f.start_with?("vendor/lexbor/") && !lexbor_kept.include?(f))
    end
  end
  spec.bindir            = "exe"
  spec.executables       = spec.files.grep(%r{\Aexe/}) { |f| File.basename(f) }
  spec.require_paths     = ["lib"]
  # The extension is the Rust crate; rb_sys runs this extconf from the crate's
  # own manifest directory.
  spec.extensions        = ["ext/makiri/rust/extconf.rb"]

  # A RUNTIME dependency, and that is not a slip. extconf.rb does
  # `require "rb_sys/mkmf"` for `create_rust_makefile`, and extconf runs at gem
  # INSTALL time - so a development dependency would not be installed for the
  # person building from source, and `gem install makiri` would fail with
  # "cannot load such file -- rb_sys/mkmf". (The C extconf needed only mkmf,
  # which ships with Ruby, so this hazard did not exist before.) rb_sys is small
  # and pure Ruby; RubyGems installs runtime dependencies before building
  # extensions, which is what makes this work.
  spec.add_dependency "rb_sys", "~> 0.9"

  # The reference Lexbor (`rake lexbor`) builds via cmake, outside the gem.
  spec.add_development_dependency "rake",            "~> 13.0"
  spec.add_development_dependency "rake-compiler",   "~> 1.2"
  spec.add_development_dependency "rspec",           "~> 3.13"
end
