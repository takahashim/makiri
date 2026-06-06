# frozen_string_literal: true

source "https://rubygems.org"

# Specify dependencies in makiri.gemspec
gemspec

gem "irb"
gem "rake", "~> 13.0"
gem "rake-compiler", "~> 1.2"
gem "rspec", "~> 3.13"

# Benchmark-only (never a runtime/gemspec dependency — Makiri itself stays
# libxml2-free). Nokogiri (libxml2) and Nokolexbor (a Nokogiri-compatible API
# also built on Lexbor) are performance references in bench/.
group :bench, optional: true do
  gem "benchmark-ips", "~> 2.0"
  gem "nokogiri", "~> 1.19"
  gem "nokolexbor", "~> 0.7"
end
