# frozen_string_literal: true
#
# Makiri::XML reader benchmark. Measures the read-only XML operations and, when
# it is installed, compares against Nokogiri (libxml2) as a reference. Nokolexbor
# is HTML-only, so it has no column here. Makiri itself never links libxml2.
#
# The document is an Atom feed (a default namespace), so the namespaced-query
# path -- the common RSS/Atom shape -- is what gets measured: under strict
# matching a default namespace needs a registered prefix, in both engines.
#
# Run:
#   rake bench:xml
#   ruby -Ilib bench/bench_xml.rb            # direct (uses system gems)
#   BENCH_ITEMS=5000 ruby -Ilib bench/bench_xml.rb

require "benchmark/ips"
require "etc"
require "makiri"

begin
  require "nokogiri"
  NOKO = true
rescue LoadError
  NOKO = false
  warn "nokogiri not available - skipping its column"
end

ATOM = "http://www.w3.org/2005/Atom"

# --- representative document ----------------------------------------------

def build_atom(items)
  entries = (1..items).map do |i|
    <<~ENTRY
      <entry>
        <title>Item #{i} &amp; more</title>
        <link href="https://example.com/p/#{i}" rel="alternate"/>
        <id>urn:uuid:#{i}</id>
        <updated>2025-06-02T#{format("%02d", i % 24)}:00:00Z</updated>
        <summary>Summary text for entry number #{i}, tag #{i % 13}.</summary>
      </entry>
    ENTRY
  end.join

  <<~XML
    <?xml version="1.0" encoding="UTF-8"?>
    <feed xmlns="#{ATOM}" xml:lang="en">
      <title>Bench Feed</title>
      <updated>2025-06-02T10:00:00Z</updated>
      #{entries}
    </feed>
  XML
end

ITEMS = Integer(ENV.fetch("BENCH_ITEMS", "2000"))
XML   = build_atom(ITEMS)
NS    = { "a" => ATOM }

m_doc = Makiri::XML(XML)
n_doc = (Nokogiri::XML(XML) if NOKO)

entry_count = m_doc.xpath("//a:entry", NS).length
puts "document: #{XML.bytesize} bytes, #{ITEMS} entries (#{entry_count} matched)"
puts "ruby #{RUBY_VERSION}  makiri #{Makiri::VERSION}" \
     "#{"  nokogiri #{Nokogiri::VERSION}" if NOKO}"
puts

def bench(title)
  puts "== #{title} =="
  Benchmark.ips do |x|
    x.config(time: 2, warmup: 1)
    yield x
    x.compare! if NOKO
  end
  puts
end

bench("parse") do |x|
  x.report("makiri")   { Makiri::XML(XML) }
  x.report("nokogiri") { Nokogiri::XML(XML) } if NOKO
end

bench("xpath: //a:entry (namespaced descendant)") do |x|
  x.report("makiri")   { m_doc.xpath("//a:entry", NS).length }
  x.report("nokogiri") { n_doc.xpath("//a:entry", NS).length } if NOKO
end

bench("xpath: //a:entry/a:title (namespaced path)") do |x|
  x.report("makiri")   { m_doc.xpath("//a:entry/a:title", NS).length }
  x.report("nokogiri") { n_doc.xpath("//a:entry/a:title", NS).length } if NOKO
end

bench("xpath: //a:link/@href (attribute axis)") do |x|
  x.report("makiri")   { m_doc.xpath("//a:link/@href", NS).length }
  x.report("nokogiri") { n_doc.xpath("//a:link/@href", NS).length } if NOKO
end

bench("xpath: //a:link[@rel='alternate'] (attr-eq predicate)") do |x|
  x.report("makiri")   { m_doc.xpath("//a:link[@rel='alternate']", NS).length }
  x.report("nokogiri") { n_doc.xpath("//a:link[@rel='alternate']", NS).length } if NOKO
end

bench("at_xpath: //a:entry[500]/a:id (first-match)") do |x|
  x.report("makiri")   { m_doc.at_xpath("//a:entry[500]/a:id", NS)&.text }
  x.report("nokogiri") { n_doc.at_xpath("//a:entry[500]/a:id", NS)&.text } if NOKO
end

# --- CSS selectors --------------------------------------------------------
# Makiri lowers CSS to its native XPath engine; Nokogiri compiles CSS to an
# XPath run through libxml2. Both auto-bind a bare type selector to the
# document's default namespace, so these run namespace-free at the call site.

bench("css: entry (type, default-ns)") do |x|
  x.report("makiri")   { m_doc.css("entry").length }
  x.report("nokogiri") { n_doc.css("entry").length } if NOKO
end

bench("css: feed > entry (child combinator)") do |x|
  x.report("makiri")   { m_doc.css("feed > entry").length }
  x.report("nokogiri") { n_doc.css("feed > entry").length } if NOKO
end

bench("css: link[rel=\"alternate\"] (attribute)") do |x|
  x.report("makiri")   { m_doc.css('link[rel="alternate"]').length }
  x.report("nokogiri") { n_doc.css('link[rel="alternate"]').length } if NOKO
end

bench("at_css: entry (first-match)") do |x|
  x.report("makiri")   { m_doc.at_css("entry")&.text }
  x.report("nokogiri") { n_doc.at_css("entry")&.text } if NOKO
end

bench("traverse: count elements via children walk") do |x|
  walk = lambda do |node, &blk|
    node.children.each do |c|
      blk.call(c)
      walk.call(c, &blk) if c.element?
    end
  end
  count = lambda do |root|
    n = 0
    walk.call(root) { |c| n += 1 if c.element? }
    n
  end
  x.report("makiri")   { count.call(m_doc.root) }
  x.report("nokogiri") { count.call(n_doc.root) } if NOKO
end

bench("text extraction: full document text") do |x|
  x.report("makiri")   { m_doc.root.text }
  x.report("nokogiri") { n_doc.root.text } if NOKO
end

# --- threaded parse throughput --------------------------------------------
#
# XML parsing, like HTML, copies the source to a C buffer and runs the parser
# with the GVL released (the fresh document is not yet shared, so it cannot
# race). So parse throughput should scale with cores; query evaluation holds
# the GVL by design and is not measured here.

NCORES = Etc.nprocessors

def par_throughput(nthreads, duration: 2.0)
  counts = Array.new(nthreads, 0)
  threads = (0...nthreads).map do |i|
    Thread.new do
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + duration
      c = 0
      while Process.clock_gettime(Process::CLOCK_MONOTONIC) < deadline
        yield
        c += 1
      end
      counts[i] = c
    end
  end
  threads.each(&:join)
  counts.sum / duration
end

puts "== threaded parse throughput (#{NCORES} cores) =="
base = nil
[1, NCORES].each do |t|
  ips = par_throughput(t) { Makiri::XML(XML) }
  base ||= ips
  scale = t == 1 ? "" : format("  (%.2fx vs 1 thread)", ips / base)
  puts format("  makiri  %2d threads: %12.1f docs/s%s", t, ips, scale)
end
puts
