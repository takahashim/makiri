# frozen_string_literal: true

desc "Run the performance benchmark (Makiri vs Nokogiri reference)"
task bench: :compile do
  # Run outside the bundle so the bench-only gems (nokogiri, benchmark-ips)
  # resolve from system RubyGems without polluting the runtime dependency set.
  Bundler.with_unbundled_env do
    sh "#{FileUtils::RUBY} -Ilib bench/bench.rb"
  end
end

desc "Run the XML reader benchmark (Makiri::XML vs Nokogiri::XML reference)"
task "bench:xml" => :compile do
  Bundler.with_unbundled_env do
    sh "#{FileUtils::RUBY} -Ilib bench/bench_xml.rb"
  end
end

desc "CSS engines on one document as HTML and XML (bench/bench_css.rb; CSS_ARGS=--json PATH / --compare PATH)"
task "bench:css" => :compile do
  sh "#{FileUtils::RUBY} -Ilib bench/bench_css.rb #{ENV['CSS_ARGS']}"
end
