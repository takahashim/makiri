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
  spec.metadata["source_code_uri"]   = spec.homepage
  spec.metadata["bug_tracker_uri"]   = "#{spec.homepage}/issues"
  spec.metadata["changelog_uri"]     = "#{spec.homepage}/blob/main/CHANGELOG.md"
  spec.metadata["rubygems_mfa_required"] = "true"

  gemspec = File.basename(__FILE__)
  spec.files = IO.popen(%w[git ls-files -z], chdir: __dir__, err: IO::NULL) do |ls|
    ls.readlines("\x0", chomp: true).reject do |f|
      (f == gemspec) ||
        f.start_with?(*%w[bin/ Gemfile .gitignore .rspec spec/ test/ bench/])
    end
  end
  spec.bindir            = "exe"
  spec.executables       = spec.files.grep(%r{\Aexe/}) { |f| File.basename(f) }
  spec.require_paths     = ["lib"]
  spec.extensions        = ["ext/makiri/extconf.rb"]

  # Vendored Lexbor builds via cmake; declared as a dev requirement.
  spec.add_development_dependency "rake",            "~> 13.0"
  spec.add_development_dependency "rake-compiler",   "~> 1.2"
  spec.add_development_dependency "rspec",           "~> 3.13"
end
