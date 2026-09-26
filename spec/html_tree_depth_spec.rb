# frozen_string_literal: true

require "spec_helper"

# The HTML tree-depth limit (lexbor/adapter/tree_guard.rs). HTML tree
# construction is quadratic in nesting depth - most start tags walk the stack of
# open elements - and the parse runs with the GVL released and cannot be
# interrupted, so an unbounded depth is a denial of service. Every HTML parse
# entry refuses a tree deeper than `max_tree_depth` (default 400, Nokogiri's
# name and default; negative disables it) with a Makiri::Error.
#
# Depth counts elements from the root, itself included: in a document <html> is
# 1 and <body> 2, so 398 nested <div>s in the body reach 400; in a fragment the
# top-level elements are 1, so a fragment holds 400. Both boundaries are
# Nokogiri::HTML5's, checked against it.
RSpec.describe "HTML tree depth limit" do
  let(:message) { /\Adocument tree depth limit exceeded \((\d+)\)\z/ }

  def nest(n, tag = "div")
    "<#{tag}>" * n
  end

  def elapsed
    t = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - t
  end

  describe "a document" do
    it "refuses the default depth with a Makiri::Error naming the limit" do
      expect { Makiri::HTML(nest(1000)) }
        .to raise_error(Makiri::Error, "document tree depth limit exceeded (400)")
    end

    it "pins the boundary: 400 deep is accepted, 401 refused" do
      # html (1) + body (2) + 398 divs = 400.
      doc = Makiri::HTML(nest(398))
      expect(doc.xpath("count(//div)")).to eq(398)
      expect { Makiri::HTML(nest(399)) }.to raise_error(Makiri::Error, message)
      # The same with the implied elements written out.
      expect { Makiri::HTML("<html><body>#{nest(398)}") }.not_to raise_error
      expect { Makiri::HTML("<html><body>#{nest(399)}") }.to raise_error(Makiri::Error, message)
    end

    it "pins the boundary for an explicit limit" do
      expect { Makiri::HTML(nest(3), max_tree_depth: 5) }.not_to raise_error
      expect { Makiri::HTML(nest(4), max_tree_depth: 5) }
        .to raise_error(Makiri::Error, "document tree depth limit exceeded (5)")
    end

    it "accepts a deeper tree under a larger limit, and refuses past it" do
      doc = Makiri::HTML(nest(1000), max_tree_depth: 2000)
      expect(doc.xpath("count(//div)")).to eq(1000)
      expect { Makiri::HTML(nest(3000), max_tree_depth: 2000) }
        .to raise_error(Makiri::Error, "document tree depth limit exceeded (2000)")
    end

    it "is disabled by a negative limit, and a Bignum is no limit in practice" do
      expect(Makiri::HTML(nest(3000), max_tree_depth: -1).xpath("count(//div)")).to eq(3000)
      expect(Makiri::HTML(nest(3000), max_tree_depth: -(2**100)).xpath("count(//div)")).to eq(3000)
      expect(Makiri::HTML(nest(3000), max_tree_depth: 2**100).xpath("count(//div)")).to eq(3000)
    end

    it "treats nil as the default" do
      expect { Makiri::HTML(nest(399), max_tree_depth: nil) }.to raise_error(Makiri::Error, message)
    end

    it "takes the keyword on every document entry" do
      deep = nest(500)
      expect(Makiri::HTML(deep, max_tree_depth: -1)).to be_a(Makiri::HTML::Document)
      expect(Makiri.parse(deep, max_tree_depth: -1)).to be_a(Makiri::HTML::Document)
      expect(Makiri::HTML::Document.parse(deep, max_tree_depth: -1)).to be_a(Makiri::HTML::Document)
      expect { Makiri.parse(deep) }.to raise_error(Makiri::Error, message)
      expect { Makiri::HTML::Document.parse(StringIO.new(deep)) }.to raise_error(Makiri::Error, message)
    end

    it "rejects a non-Integer limit and an unknown keyword" do
      expect { Makiri::HTML("<p>", max_tree_depth: 1.5) }.to raise_error(TypeError, /Integer/)
      expect { Makiri::HTML("<p>", max_tree_depth: "400") }.to raise_error(TypeError, /Integer/)
      expect { Makiri::HTML("<p>", max_tree_depth_typo: 1) }.to raise_error(ArgumentError)
    end

    it "counts a <template>'s contents on the same stack" do
      # html, head, template, then the contents: 397 divs reach 400.
      expect { Makiri::HTML("<template>#{nest(397)}") }.not_to raise_error
      expect { Makiri::HTML("<template>#{nest(398)}") }.to raise_error(Makiri::Error, message)
    end

    it "counts foreign content too" do
      expect { Makiri::HTML("<svg>#{nest(500, "g")}") }.to raise_error(Makiri::Error, message)
    end

    it "leaves normal documents, and their source lines, unchanged" do
      html = "<!doctype html>\n<html><head><title>t</title></head>\n<body>\n" \
             "#{(1..200).map { |i| "<ul><li><a href='/#{i}'>#{i}</a></li></ul>" }.join("\n")}\n</body></html>"
      doc = Makiri::HTML(html)
      expect(doc.css("li").size).to eq(200)
      expect(doc.at_css("a[href='/5']").line).to eq(8)
      expect(doc.to_html).to eq(Makiri::HTML(html, max_tree_depth: -1).to_html)
    end

    it "keeps Document#dup working for a document parsed without a limit" do
      doc = Makiri::HTML(nest(1000), max_tree_depth: -1)
      expect(doc.dup.xpath("count(//div)")).to eq(1000)
    end

    it "bounds the quadratic case: 80,000 nested elements fail fast" do
      skip "timing is meaningless under GC.stress" if GC_COMPACT_STRESS
      deep = nest(80_000) # 400 KB; took ~5 s to parse before the limit
      took = elapsed { expect { Makiri::HTML(deep) }.to raise_error(Makiri::Error, message) }
      expect(took).to be < 0.1
    end

    it "leaves the document usable after a refusal" do
      3.times { expect { Makiri::HTML(nest(1000)) }.to raise_error(Makiri::Error) }
      expect(Makiri::HTML("<p>ok").at_css("p").text).to eq("ok")
    end
  end

  describe "a fragment" do
    it "pins the boundary: top-level elements are depth 1, 400 accepted, 401 refused" do
      frag = Makiri::HTML::DocumentFragment.parse(nest(400))
      expect(frag.xpath("count(.//div)")).to eq(400)
      expect { Makiri::HTML::DocumentFragment.parse(nest(401)) }
        .to raise_error(Makiri::Error, "document tree depth limit exceeded (400)")
    end

    it "takes max_tree_depth on DocumentFragment.parse and Document#fragment" do
      expect { Makiri::HTML::DocumentFragment.parse(nest(5), max_tree_depth: 5) }.not_to raise_error
      expect { Makiri::HTML::DocumentFragment.parse(nest(6), max_tree_depth: 5) }
        .to raise_error(Makiri::Error, "document tree depth limit exceeded (5)")
      expect(Makiri::HTML::DocumentFragment.parse(nest(3000), max_tree_depth: -1).xpath("count(.//div)"))
        .to eq(3000)

      doc = Makiri::HTML("<p>")
      expect { doc.fragment(nest(401)) }.to raise_error(Makiri::Error, message)
      expect(doc.fragment(nest(1000), max_tree_depth: 2000).xpath("count(.//div)")).to eq(1000)
      expect { doc.fragment(nest(3000), max_tree_depth: 2000) }.to raise_error(Makiri::Error, message)
    end

    it "is guarded in any context, the context element not counted" do
      %w[body div template tr svg].each do |ctx|
        tag = ctx == "svg" ? "g" : "div"
        inner = ctx == "tr" ? "<td>#{nest(399, tag)}" : nest(400, tag)
        expect { Makiri::HTML::DocumentFragment.parse(inner, context: ctx) }.not_to raise_error, ctx
        expect { Makiri::HTML::DocumentFragment.parse("#{inner}<#{tag}>", context: ctx) }
          .to raise_error(Makiri::Error, message), ctx
      end
    end

    it "guards Node#parse with the default limit" do
      div = Makiri::HTML("<div></div>").at_css("div")
      expect(div.parse(nest(400)).size).to eq(1)
      expect { div.parse(nest(401)) }.to raise_error(Makiri::Error, message)
    end

    it "bounds the quadratic case for a fragment too" do
      skip "timing is meaningless under GC.stress" if GC_COMPACT_STRESS
      took = elapsed do
        expect { Makiri::HTML::DocumentFragment.parse(nest(80_000)) }.to raise_error(Makiri::Error, message)
      end
      expect(took).to be < 0.1
    end
  end

  describe "the fragment setters" do
    let(:doc) { Makiri::HTML("<div id='a'><b>old</b></div><div id='b'><i>x</i></div><template id='t'><s>t</s></template>") }

    it "guards inner_html= and leaves the element unchanged on refusal" do
      a = doc.at_css("#a")
      a.inner_html = nest(400)
      expect(a.xpath("count(.//div)")).to eq(400)
      a.inner_html = "<b>old</b>"
      expect { a.inner_html = nest(401) }.to raise_error(Makiri::Error, message)
      expect(a.inner_html).to eq("<b>old</b>")
    end

    it "guards outer_html= and leaves the tree unchanged on refusal" do
      b = doc.at_css("#b")
      expect { b.outer_html = nest(401) }.to raise_error(Makiri::Error, message)
      expect(doc.at_css("#b").inner_html).to eq("<i>x</i>")
    end

    it "guards a <template>'s inner_html=" do
      t = doc.at_css("#t")
      expect { t.inner_html = nest(401) }.to raise_error(Makiri::Error, message)
      expect(t.inner_html).to eq("<s>t</s>")
      t.inner_html = nest(400)
      expect(t.content_fragment.xpath("count(.//div)")).to eq(400)
    end

    it "survives many refusals (the transient document is freed each time)" do
      a = doc.at_css("#a")
      200.times { expect { a.inner_html = nest(500) }.to raise_error(Makiri::Error) }
      GC.start
      expect(a.inner_html).to eq("<b>old</b>")
    end
  end

  # The one quadratic shape depth does not cover: every <option> a select
  # receives re-runs Lexbor's selectedness algorithm over all its options, so
  # 40,000 options (a flat 400 KB) took four seconds. Each select may receive
  # at most 10,000 during a parse.
  describe "the <option> count per <select>" do
    let(:message) { /too many option elements in one select element \(limit 10000\)/ }

    def options(n, open = "<select>", close = "</select>") = "#{open}#{"<option>x" * n}#{close}"

    it "accepts 10,000 options in one select and refuses one more" do
      expect(Makiri::HTML(options(10_000)).css("option").size).to eq(10_000)
      expect { Makiri::HTML(options(10_001)) }.to raise_error(Makiri::Error, message)
    end

    it "bounds the quadratic case: 40,000 options are refused, not parsed" do
      started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      expect { Makiri::HTML(options(40_000)) }.to raise_error(Makiri::Error, message)
      expect(Process.clock_gettime(Process::CLOCK_MONOTONIC) - started).to be < 2.5
    end

    it "counts per select, and counts options inside an optgroup" do
      expect(Makiri::HTML(options(6_000) * 2).css("select").size).to eq(2)
      grouped = "<select>#{"<optgroup>#{"<option>x" * 100}</optgroup>" * 101}</select>"
      expect { Makiri::HTML(grouped) }.to raise_error(Makiri::Error, message)
    end

    it "does not count options that update no select" do
      expect(Makiri::HTML(options(12_000, "<datalist>", "</datalist>")).css("option").size).to eq(12_000)
      expect(Makiri::HTML("<option>x" * 12_000).css("option").size).to eq(12_000)
    end

    it "guards fragments and inner_html=, leaving the element unchanged on refusal" do
      expect { Makiri::HTML::DocumentFragment.parse(options(10_001)) }.to raise_error(Makiri::Error, message)
      doc = Makiri::HTML("<div><b>old</b></div>")
      expect { doc.at_css("div").inner_html = options(10_001) }.to raise_error(Makiri::Error, message)
      expect(doc.at_css("div").inner_html).to eq("<b>old</b>")
    end
  end

  describe "the XML side" do
    it "is unchanged: its own fixed limit, and no max_tree_depth keyword" do
      expect(Makiri::XML("#{"<a>" * 1000}#{"</a>" * 1000}").root.name).to eq("a")
      expect { Makiri::XML("<a/>", max_tree_depth: 10) }.to raise_error(ArgumentError, /unknown keyword/)
    end
  end
end
