# frozen_string_literal: true

# Inputs whose cost grew faster than their size, with no budget to stop them:
# each took seconds to minutes on input of a few kilobytes to a few megabytes.
# The bounds below are generous - ten to a hundred times the fixed cost - so a
# slow machine passes, and a return to the old growth fails by minutes.
RSpec.describe "Cost proportional to input" do
  def elapsed
    t = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - t
  end

  describe "the preceding axis" do
    # Each climb re-walked the context's whole ancestor chain: O(depth^2) per
    # context node, uncharged. Depth 2000 took 8.8 s.
    it "is linear in the depth of the context" do
      doc = Makiri.HTML("<body>#{"<span>" * 5000}")
      expect(elapsed { doc.xpath("count(//span/preceding::a)") }).to be < 2.0
    end

    # The walk now recognises an ancestor by the one still to be skipped. This
    # holds it to the definition: everything before the context in document
    # order, less its ancestors - from element, text and attribute contexts.
    it "still yields exactly the preceding nodes" do
      [Makiri.HTML(%(<p k="1">a<b k="2">c<i>d</i></b>e</p><p><b k="3">f</b></p>)),
       Makiri::XML(%(<r><a k="1">x<b><c k="2">y</c></b></a><a><b k="3"/>z</a></r>))].each do |doc|
        all = doc.xpath("//node()").to_a
        (all + doc.xpath("//@*").to_a).each do |ctx|
          base = ctx.attribute? ? ctx.parent : ctx
          ancestors = []
          a = ctx.parent
          while a && !a.document?
            ancestors << a
            a = a.parent
          end
          expected = all.take(all.index(base)).reject { |n| ancestors.include?(n) }
          expect(ctx.xpath("preceding::node()").to_a).to eq(expected), ctx.path
        end
      end
    end
  end

  describe "Makiri::Lexbor::CSS.parse_stylesheet" do
    let(:bad_rules) { (1..4000).map { |i| "p::x#{i}abcdefghijklmnopqrstuvwxyz0123456789 {}" }.join("\n") }

    # Once one :lexbor-contains was rewritten, every rejected rule searched the
    # whole sheet for its prelude: 186 KB took 4.6 s.
    it "stays linear after a rewritten :lexbor-contains" do
      sheet = "a:lexbor-contains(1) {x:y}\n#{bad_rules}"
      expect(elapsed { Makiri::Lexbor::CSS.parse_stylesheet(sheet) }).to be < 1.0
    end

    it "stays linear when every rule holds one" do
      sheet = (1..4000).map { |i| "a#{i}:lexbor-contains(1) {x:y}" }.join("\n")
      expect(elapsed { Makiri::Lexbor::CSS.parse_stylesheet(sheet) }).to be < 1.0
    end

    # The same prelude twice was "ambiguous" and showed the rewritten name.
    it "shows each rejected prelude as written" do
      sheet = "a:lexbor-contains(1), b {x:y} a:lexbor-contains(1), b {x:y} c:lexbor\\-contains(2) {x:y}"
      texts = Makiri::Lexbor::CSS.parse_stylesheet(sheet).map { |r| r[:selector_text] }
      expect(texts).to eq(["a:lexbor-contains(1), b ", "a:lexbor-contains(1), b ", "c:lexbor\\-contains(2) "])
    end
  end

  describe "XPath string searches" do
    # contains / substring-before / substring-after searched with a window
    # scan, O(haystack x needle): 400 KB took 2.4 s.
    it "is linear in the strings" do
      doc = Makiri.HTML("<p>#{"a" * 400_000}</p><p>#{"a" * 200_000}b</p>")
      expect(elapsed { doc.xpath("contains(//p[1], //p[2])") }).to be < 1.0
      expect(doc.xpath("substring-before('abcabc', 'ca')")).to eq("ab")
      expect(doc.xpath("substring-after('abcabc', 'ca')")).to eq("bc")
      expect(doc.xpath("contains('日本語', '本')")).to be(true)
    end
  end

  describe "the XML parser's duplicate-attribute check" do
    # Pairwise over up to 4096 attributes an element: 100 such elements (3 MB)
    # took 16.6 s.
    it "is not quadratic in an element's attribute count" do
      attrs = (0...4096).map { |i| %(a#{i}="") }.join(" ")
      xml = "<r>#{"<e #{attrs}/>" * 100}</r>"
      expect(elapsed { Makiri::XML(xml) }).to be < 3.0
    end

    it "still finds a duplicate past the pairwise limit, by namespace and local name" do
      many = (0...40).map { |i| %(a#{i}="") }.join(" ")
      expect { Makiri::XML(%(<r><e #{many} a7="x"/></r>)) }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML(%(<r xmlns:p="u" xmlns:q="u"><e #{many} p:k="1" q:k="2"/></r>)) }
        .to raise_error(Makiri::XML::SyntaxError)
      expect(Makiri::XML(%(<r xmlns:p="u" xmlns:q="v"><e #{many} p:k="1" q:k="2"/></r>)).root).not_to be_nil
    end
  end

  # Walks with no budget charge: each ran its quadratic course and answered.
  # Charged a tick per step, they stop at the op budget instead.
  describe "XPath walks charged to the op budget" do
    let(:nested) { Makiri.HTML("<body>#{"<span>" * 16_000}") }

    {
      "a node's string-value, per candidate" => "count(//span[. = 'x'])",
      "lang()'s ancestor climb, per candidate" => "count(//span[lang('en')])"
    }.each do |what, expr|
      it "stops #{what}" do
        took = elapsed { expect { nested.xpath(expr) }.to raise_error(Makiri::XPath::LimitExceeded) }
        expect(took).to be < 5.0
      end
    end

    it "stops :nth-of-type's sibling count over XML" do
      xml = Makiri::XML("<r>#{"<a/>" * 40_000}</r>")
      took = elapsed { expect { xml.css("r > *:nth-of-type(2)") }.to raise_error(Makiri::XPath::LimitExceeded) }
      expect(took).to be < 5.0
    end

    it "leaves ordinary sizes inside the budget" do
      doc = Makiri.HTML("<div lang='en'>#{"<p><span>x</span></p>" * 2000}</div>")
      expect(doc.xpath("count(//span[. = 'x'])")).to eq(2000)
      expect(doc.xpath("count(//span[lang('en')])")).to eq(2000)
      expect(Makiri::XML("<r>#{"<a/><b/>" * 500}</r>").css("r > a:nth-of-type(2)").size).to eq(1)
    end

    # translate() looked each character up by scanning `from`; now a table.
    it "keeps translate()'s first-occurrence rule" do
      x = Makiri::XML("<r/>")
      expect(x.xpath(%q{translate("bar","abc","ABC")})).to eq("BAr")
      expect(x.xpath(%q{translate("--aaa--","abc-","ABC")})).to eq("AAA")
      expect(x.xpath(%q{translate("aba","aab","xyz")})).to eq("xzx")
      expect(x.xpath(%q{translate("日本","本日","XY")})).to eq("YX")
    end
  end
end
