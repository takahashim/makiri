# frozen_string_literal: true
#
# Makiri performance benchmark. Measures the main operations and, when Nokogiri
# is installed, compares against it as a reference (Nokogiri is a bench-only
# dependency — Makiri itself never links libxml2).
#
# Run:
#   rake bench
#   ruby -Ilib bench/bench.rb            # direct (uses system gems)
#   BENCH_ITEMS=5000 ruby -Ilib bench/bench.rb
#
# Use it to find hot paths and to A/B a change: capture the numbers before,
# apply the optimization, re-run, and confirm the gain is real (and that the
# spec suite + fuzzer still pass).

require "benchmark/ips"
require "etc"
require "makiri"

begin
  require "nokogiri"
  NOKO = true
rescue LoadError
  NOKO = false
  warn "nokogiri not available — running Makiri-only (no comparison)"
end

# --- representative document ----------------------------------------------

def build_html(items)
  rows = (1..items).map do |i|
    %(<li class="item r#{i % 7}" data-id="#{i}" data-rank="#{i % 100}">) +
      %(<a href="/p/#{i}" class="link">item #{i}</a>) +
      %(<span class="meta">tag#{i % 13} &amp; more</span></li>)
  end.join("\n")

  <<~HTML
    <!doctype html><html><head><title>bench</title>
    <meta charset="utf-8"></head>
    <body>
      <header id="top"><nav><a href="/">home</a><a href="/about">about</a></nav></header>
      <main id="main" class="container">
        <section class="list"><ul>#{rows}</ul></section>
      </main>
      <footer id="bot"><p>&copy; bench</p></footer>
    </body></html>
  HTML
end

ITEMS = Integer(ENV.fetch("BENCH_ITEMS", "2000"))
HTML  = build_html(ITEMS)

m_doc = Makiri::HTML(HTML)
n_doc = (Nokogiri::HTML(HTML) if NOKO)

el_count = m_doc.css("*").length
puts "document: #{HTML.bytesize} bytes, #{ITEMS} items, #{el_count} elements"
puts "ruby #{RUBY_VERSION}  makiri #{Makiri::VERSION}#{"  nokogiri #{Nokogiri::VERSION}" if NOKO}"
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
  x.report("makiri")   { Makiri::HTML(HTML) }
  x.report("nokogiri") { Nokogiri::HTML(HTML) } if NOKO
end

bench("css: ul li.item") do |x|
  x.report("makiri")   { m_doc.css("ul li.item").length }
  x.report("nokogiri") { n_doc.css("ul li.item").length } if NOKO
end

bench("at_css: #main") do |x|
  x.report("makiri")   { m_doc.at_css("#main") }
  x.report("nokogiri") { n_doc.at_css("#main") } if NOKO
end

bench("xpath: //li[@data-rank='1']") do |x|
  x.report("makiri")   { m_doc.xpath("//li[@data-rank='1']").length }
  x.report("nokogiri") { n_doc.xpath("//li[@data-rank='1']").length } if NOKO
end

bench("xpath: //a/@href (attribute axis)") do |x|
  x.report("makiri")   { m_doc.xpath("//a/@href").length }
  x.report("nokogiri") { n_doc.xpath("//a/@href").length } if NOKO
end

bench("xpath: //li (descendant tag)") do |x|
  x.report("makiri")   { m_doc.xpath("//li").length }
  x.report("nokogiri") { n_doc.xpath("//li").length } if NOKO
end

bench("xpath: //*[@id='main'] (id)") do |x|
  x.report("makiri")   { m_doc.xpath("//*[@id='main']").length }
  x.report("nokogiri") { n_doc.xpath("//*[@id='main']").length } if NOKO
end

bench("xpath: //li[@data-id='1000'] (attr eq)") do |x|
  x.report("makiri")   { m_doc.xpath("//li[@data-id='1000']").length }
  x.report("nokogiri") { n_doc.xpath("//li[@data-id='1000']").length } if NOKO
end

bench("traverse: count elements via children walk") do |x|
  walk = lambda do |node, &blk|
    node.children.each do |c|
      blk.call(c)
      walk.call(c, &blk) if c.element?
    end
  end
  x.report("makiri") do
    n = 0
    walk.call(m_doc.root) { |c| n += 1 if c.element? }
    n
  end
  if NOKO
    x.report("nokogiri") do
      n = 0
      walk.call(n_doc.root) { |c| n += 1 if c.element? }
      n
    end
  end
end

bench("serialize: to_html") do |x|
  x.report("makiri")   { m_doc.to_html }
  x.report("nokogiri") { n_doc.to_html } if NOKO
end

bench("text extraction: full document text") do |x|
  x.report("makiri")   { m_doc.text }
  x.report("nokogiri") { n_doc.text } if NOKO
end

# --- threaded throughput --------------------------------------------------
#
# Measures aggregate ops/sec with T worker threads, each doing CPU-bound work
# in a tight loop for a fixed wall-time. The gate metric is the N-thread vs
# 1-thread speedup: with the GVL held it stays ~flat (no scaling), with the
# GVL released around the pure-C region it should approach core count. Each
# thread uses its own data (no shared Document) to mirror real concurrent use.

NCORES = Etc.nprocessors

# Run +block+ in a loop on +nthreads+ threads for +duration+ seconds; returns
# total ops/sec. +setup+ (if given) runs once per thread and its result is
# passed to the block, so per-thread state (e.g. a Document) isn't shared.
def par_throughput(nthreads, duration: 2.0, setup: nil)
  counts = Array.new(nthreads, 0)
  threads = (0...nthreads).map do |i|
    Thread.new do
      ctx = setup&.call
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + duration
      c = 0
      while Process.clock_gettime(Process::CLOCK_MONOTONIC) < deadline
        yield ctx
        c += 1
      end
      counts[i] = c
    end
  end
  threads.each(&:join)
  counts.sum / duration
end

def threaded(title, unit, setup: nil)
  puts "== #{title} (#{NCORES} cores) =="
  base = nil
  [1, NCORES].each do |t|
    ips = par_throughput(t, setup: setup) { |ctx| yield ctx }
    base ||= ips
    scale = t == 1 ? "" : format("  (%.2fx vs 1 thread)", ips / base)
    puts format("  makiri  %2d threads: %12.1f %s/s%s", t, ips, unit, scale)
  end
  puts
end

threaded("threaded parse throughput", "docs") { |_| Makiri::HTML(HTML) }
threaded("threaded xpath throughput", "queries",
         setup: -> { Makiri::HTML(HTML) }) { |doc| doc.xpath("//li").length }
