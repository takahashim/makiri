# frozen_string_literal: true
#
# Makiri performance benchmark. Measures the main operations and, when they are
# installed, compares against Nokogiri (libxml2) and Nokolexbor (a
# Nokogiri-compatible API also built on Lexbor) as references. Both are
# bench-only dependencies - Makiri itself never links libxml2.
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
  warn "nokogiri not available - skipping its column"
end

begin
  require "nokolexbor"
  NOKOLEX = true
rescue LoadError
  NOKOLEX = false
  warn "nokolexbor not available - skipping its column"
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

m_doc  = Makiri::HTML(HTML)
n_doc  = (Nokogiri::HTML(HTML) if NOKO)
nl_doc = (Nokolexbor::HTML(HTML) if NOKOLEX)

el_count = m_doc.css("*").length
puts "document: #{HTML.bytesize} bytes, #{ITEMS} items, #{el_count} elements"
puts "ruby #{RUBY_VERSION}  makiri #{Makiri::VERSION}" \
     "#{"  nokogiri #{Nokogiri::VERSION}" if NOKO}" \
     "#{"  nokolexbor #{Nokolexbor::VERSION}" if NOKOLEX}"
puts

def bench(title)
  puts "== #{title} =="
  Benchmark.ips do |x|
    x.config(time: 2, warmup: 1)
    yield x
    x.compare! if NOKO || NOKOLEX
  end
  puts
end

bench("parse") do |x|
  x.report("makiri")     { Makiri::HTML(HTML) }
  x.report("nokogiri")   { Nokogiri::HTML(HTML) } if NOKO
  x.report("nokolexbor") { Nokolexbor::HTML(HTML) } if NOKOLEX
end

bench("css: ul li.item") do |x|
  x.report("makiri")     { m_doc.css("ul li.item").length }
  x.report("nokogiri")   { n_doc.css("ul li.item").length } if NOKO
  x.report("nokolexbor") { nl_doc.css("ul li.item").length } if NOKOLEX
end

bench("at_css: #main") do |x|
  x.report("makiri")     { m_doc.at_css("#main") }
  x.report("nokogiri")   { n_doc.at_css("#main") } if NOKO
  x.report("nokolexbor") { nl_doc.at_css("#main") } if NOKOLEX
end

bench("xpath: //li[@data-rank='1']") do |x|
  x.report("makiri")     { m_doc.xpath("//li[@data-rank='1']").length }
  x.report("nokogiri")   { n_doc.xpath("//li[@data-rank='1']").length } if NOKO
  x.report("nokolexbor") { nl_doc.xpath("//li[@data-rank='1']").length } if NOKOLEX
end

bench("xpath: //a/@href (attribute axis)") do |x|
  x.report("makiri")     { m_doc.xpath("//a/@href").length }
  x.report("nokogiri")   { n_doc.xpath("//a/@href").length } if NOKO
  x.report("nokolexbor") { nl_doc.xpath("//a/@href").length } if NOKOLEX
end

bench("xpath: //li (descendant tag)") do |x|
  x.report("makiri")     { m_doc.xpath("//li").length }
  x.report("nokogiri")   { n_doc.xpath("//li").length } if NOKO
  x.report("nokolexbor") { nl_doc.xpath("//li").length } if NOKOLEX
end

bench("xpath: //*[@id='main'] (id)") do |x|
  x.report("makiri")     { m_doc.xpath("//*[@id='main']").length }
  x.report("nokogiri")   { n_doc.xpath("//*[@id='main']").length } if NOKO
  x.report("nokolexbor") { nl_doc.xpath("//*[@id='main']").length } if NOKOLEX
end

bench("xpath: //li[@data-id='1000'] (attr eq)") do |x|
  x.report("makiri")     { m_doc.xpath("//li[@data-id='1000']").length }
  x.report("nokogiri")   { n_doc.xpath("//li[@data-id='1000']").length } if NOKO
  x.report("nokolexbor") { nl_doc.xpath("//li[@data-id='1000']").length } if NOKOLEX
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
  x.report("makiri")     { count.call(m_doc.root) }
  x.report("nokogiri")   { count.call(n_doc.root) } if NOKO
  x.report("nokolexbor") { count.call(nl_doc.root) } if NOKOLEX
end

bench("serialize: to_html") do |x|
  x.report("makiri")     { m_doc.to_html }
  x.report("nokogiri")   { n_doc.to_html } if NOKO
  x.report("nokolexbor") { nl_doc.to_html } if NOKOLEX
end

bench("text extraction: full document text") do |x|
  # Makiri/Nokogiri's Document#text returns the whole document's text; Nokolexbor
  # gives a Document an empty textContent (per the DOM), so extract from its root.
  x.report("makiri")     { m_doc.text }
  x.report("nokogiri")   { n_doc.text } if NOKO
  x.report("nokolexbor") { nl_doc.root.text } if NOKOLEX
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
