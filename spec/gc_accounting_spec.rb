# frozen_string_literal: true

# A parsed document lives in an arena outside Ruby's allocator (Lexbor's pools
# for HTML, our own for XML), so the GC would see it as a few dozen bytes and
# never collect from memory pressure: a loop that parses and drops documents
# used to grow RSS by one document per parse and pay for freshly faulted pages
# on every one (measured at 2x the parse time). The bridge now reports each
# document's arena through rb_gc_adjust_memory_usage and takes the report back
# when the wrapper is freed. These examples pin both halves.
RSpec.describe "GC accounting of document arenas" do
  # ~280 KB of HTML, a few MB of arena: the shape the benchmark parses.
  let(:html) do
    rows = (1..2000).map do |i|
      %(<li class="item r#{i % 7}" data-id="#{i}"><a href="/p/#{i}">item #{i}</a><span>tag#{i % 13}</span></li>)
    end.join("\n")
    "<!doctype html><html><head><title>t</title></head><body><ul>#{rows}</ul></body></html>"
  end

  let(:xml) do
    rows = (1..2000).map { |i| %(<item id="#{i}" rank="#{i % 100}"><name>item #{i}</name></item>) }.join("\n")
    "<?xml version='1.0'?><root>#{rows}</root>"
  end

  before { require "objspace" }

  it "reports the HTML arena as the Document's memsize" do
    doc = Makiri::HTML(html)
    # The arena holds every node and every text run, so it outweighs the source.
    expect(ObjectSpace.memsize_of(doc)).to be > html.bytesize
  end

  it "reports the XML arena as the Document's memsize" do
    doc = Makiri::XML(xml)
    expect(ObjectSpace.memsize_of(doc)).to be > xml.bytesize
  end

  it "lets memory pressure from dropped HTML documents trigger a collection" do
    GC.start
    before = GC.count
    # Well past malloc_limit_max (32 MB by default) when the arenas are seen;
    # a handful of wrapper objects when they are not.
    64.times { Makiri::HTML(html) }
    expect(GC.count).to be > before
  end

  it "lets memory pressure from dropped XML documents trigger a collection" do
    GC.start
    before = GC.count
    64.times { Makiri::XML(xml) }
    expect(GC.count).to be > before
  end

  it "takes the report back when the document is freed, so RSS stays bounded" do
    # The report is balanced on free, so after a collection the next parse
    # reuses the freed arena instead of faulting a fresh one: the resident set
    # settles rather than growing by one document per parse.
    # KiB: /proc on Linux, ps(1) elsewhere (macOS); Windows has neither.
    rss = if File.readable?("/proc/self/status")
            -> { Integer(File.read("/proc/self/status")[/VmRSS:\s+(\d+)/, 1]) }
          elsif !Gem.win_platform?
            -> { Integer(`ps -o rss= -p #{Process.pid}`) }
          end
    skip "needs /proc or ps" unless rss
    32.times { Makiri::HTML(html) }
    GC.start
    settled = rss.call
    256.times { Makiri::HTML(html) }
    # 256 unfreed documents would be over a gigabyte; allow for the GC's
    # malloc_limit worth of documents in flight plus heap growth.
    expect(rss.call - settled).to be < 200 * 1024
  end

  # The node cache marks its wrappers MOVABLE rather than pinning them, so a
  # document walked end to end does not stop compaction doing its job. That makes
  # DocData::compact load-bearing: without it the cache hands back a VALUE for
  # where an object USED to be. Breaking the callback on purpose crashes the
  # process here, which is why this runs GC.compact rather than trusting a read.
  describe "the node cache survives compaction" do
    %w[HTML XML].each do |kind|
      it "keeps identity, attached state and reads across GC.compact (#{kind})" do
        src = "<r>#{(1..500).map { |i| %(<e#{i} id="i#{i}">t#{i}</e#{i}>) }.join}</r>"
        doc = kind == "HTML" ? Makiri::HTML(src) : Makiri::XML(src)
        root = kind == "HTML" ? doc.at_css("r") : doc.root
        held = root.children.to_a
        held.each_with_index { |n, i| n.instance_variable_set(:@tag, i) }
        ids = held.map(&:object_id)

        3.times { GC.compact }

        again = root.children.to_a
        expect(again).to eq(held)                       # same nodes
        expect(again.map(&:object_id)).to eq(ids)       # same OBJECTS
        expect(again.each_with_index.all? { |n, i| n.equal?(held[i]) }).to be true
        expect(again.each_with_index.all? { |n, i| n.instance_variable_get(:@tag) == i }).to be true
        expect(again.each_with_index.all? { |n, i| n["id"] == "i#{i + 1}" }).to be true
      end
    end

    it "still navigates after the wrappers are dropped and the heap compacted" do
      doc = Makiri::HTML("<r><a id='x'/></r>")
      root = doc.at_css("r")
      root.children.to_a
      3.times { GC.compact }
      expect(root.children.first["id"]).to eq("x")
    end
  end
end
