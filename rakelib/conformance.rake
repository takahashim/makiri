# frozen_string_literal: true

namespace :conformance do
  desc "WHATWG HTML parsing conformance: the WPT tree-construction data through Makiri"
  task html5: :compile do
    sh "#{FileUtils::RUBY} -Ilib spec/conformance/html5lib_runner.rb #{ENV['H5_ARGS']}"
  end

  desc "HTML parser conformance in the crate: WPT tree construction through each parser"
  task parser: :lexbor do
    sh "#{FileUtils::RUBY} spec/conformance/parser_data.rb"
    Dir.chdir("ext/makiri/rust") do
      sh "cargo test --no-default-features --features lexbor -- --include-ignored wpt html5lib entities_are_the_whatwg res_gen"
    end
  end

  # The ported tree builder against Lexbor over 100,000 real homepages
  # (web100k, ~3.1 GB downloaded batch by batch). A manual check, not in CI;
  # BATCHES=1,2 narrows it, KEEP=1 keeps the decompressed pages.
  desc "HTML parser port vs Lexbor over the web100k corpus (BATCHES=1,2; needs zstd)"
  task :web100k do
    sh "#{FileUtils::RUBY} spec/conformance/web100k.rb"
  end

  # Evidence, not a gate: docs/rewrite_parser.ja.md makes the pinned HTML
  # Standard the authority, and browsers can all predate a spec change.
  desc "HTML parser vs Chromium/Firefox/WebKit (Playwright; needs Node.js)"
  task browsers: :compile do
    dir = "spec/conformance/browser"
    unless File.directory?("#{dir}/node_modules/playwright-core")
      sh "npm", "install", "--no-audit", "--no-fund", "--prefix", dir
    end
    sh "#{FileUtils::RUBY} -Ilib spec/conformance/browser_diff.rb #{ENV['BROWSER_ARGS']}"
  end

  desc "CSS selectors vs Chromium/Firefox/WebKit, HTML and XML (Playwright; needs Node.js)"
  task css_browsers: :compile do
    dir = "spec/conformance/browser"
    unless File.directory?("#{dir}/node_modules/playwright-core")
      sh "npm", "install", "--no-audit", "--no-fund", "--prefix", dir
    end
    sh "#{FileUtils::RUBY} -Ilib spec/conformance/css_browser_diff.rb #{ENV['BROWSER_ARGS']}"
  end

  desc "XPath 1.0 differential conformance vs Nokogiri (libxml2 reference)"
  task xpath: :compile do
    # Like `bench`, run outside the bundle so the bench-only Nokogiri resolves
    # from system RubyGems without entering the runtime dependency set.
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/xpath_diff.rb #{ENV['XPATH_ARGS']}"
    end
  end

  desc "XML XPath 1.0 differential conformance: Makiri::XML vs Nokogiri::XML"
  task xpath_xml: :compile do
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/xml_xpath_diff.rb #{ENV['XPATH_ARGS']}"
    end
  end

  desc "W3C XML Conformance Test Suite: well-formedness through Makiri::XML"
  task xmlconf: :compile do
    # Nokogiri (bench-only) parses the manifests, so run outside the bundle.
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/xmlconf_runner.rb #{ENV['XMLCONF_ARGS']}"
    end
  end

  desc "Property-based XML differential: generated documents, Makiri vs Nokogiri tree"
  task xml_pbt: :compile do
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/xml_pbt_diff.rb #{ENV['PBT_ARGS']}"
    end
  end

  desc "CSS Selectors differential conformance vs Nokogiri::HTML5"
  task css: :compile do
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/css_diff.rb #{ENV['CSS_ARGS']}"
    end
  end

  desc "XML CSS-selector differential conformance: Makiri::XML vs Nokogiri::XML"
  task css_xml: :compile do
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/xml_css_diff.rb #{ENV['CSS_XML_ARGS']}"
    end
  end

  desc "XML Builder differential conformance: Makiri::XML::Builder vs Nokogiri::XML::Builder"
  task builder: :compile do
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/builder_diff.rb #{ENV['BUILDER_ARGS']}"
    end
  end
end

desc "Run all conformance suites"
task conformance: %w[conformance:html5 conformance:xpath conformance:css
                     conformance:xmlconf conformance:xpath_xml conformance:css_xml
                     conformance:builder]
