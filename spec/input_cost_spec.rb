# frozen_string_literal: true

# Inputs whose cost grew faster than their size, with no budget to stop them:
# each took seconds to minutes on input of a few kilobytes to a few megabytes.
# The bounds below are generous - ten to a hundred times the fixed cost - so a
# slow machine passes, and a return to the old growth fails by minutes.
RSpec.describe "Cost proportional to input", :timing do
  # An AddressSanitizer build runs several times slower, and a CI runner slower
  # again: 64k stylesheet rules took 1.59 s there against a 1.5 s bound. Every
  # regression these guard against was 10-100x over its bound, so a slack of 5
  # under the sanitizer still catches each one.
  SLACK = ENV.key?("ASAN_OPTIONS") ? 5.0 : 1.0

  # Seconds the block took, divided by SLACK, so each example can keep stating
  # the plain build's bound.
  def elapsed
    t = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t) / SLACK
  end

  # A single-context reverse-axis step was merge-sorted back into document
  # order, uncharged: count(preceding-sibling::i) per candidate over 4000
  # siblings took 3.9 s, following-sibling 0.2 s. It is reversed now.
  describe "a reverse-axis step" do
    it "costs what the forward one does" do
      doc = Makiri.HTML("<div>#{"<i>x</i>" * 4000}</div>")
      took = elapsed { doc.xpath("count(//i[count(preceding-sibling::i) mod 2 = 1])") }
      expect(took).to be < 1.5
      expect(doc.xpath("count(//i[count(preceding-sibling::i) mod 2 = 1])")).to eq(2000)
      expect(doc.at_xpath("//i[3]/preceding-sibling::i[1]")).to eq(doc.at_xpath("//i[2]"))
    end
  end

  describe "the preceding axis" do
    # Each climb re-walked the context's whole ancestor chain: O(depth^2) per
    # context node, uncharged. Depth 2000 took 8.8 s.
    it "is linear in the depth of the context" do
      doc = Makiri.HTML("<body>#{"<span>" * 5000}", max_tree_depth: -1)
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

    # 4000 rules hid it: the search that replaced the first quadratic one
    # re-validated the rest of the sheet on each rule - 64k rules took 4.5 s.
    # The prelude is now sliced by Lexbor's own offsets.
    it "stays linear when every rule holds one" do
      sheet = ":lexbor-contains(1 2 3 4){}\n" * 64_000
      expect(elapsed { Makiri::Lexbor::CSS.parse_stylesheet(sheet) }).to be < 1.5
    end

    # A search for the prelude's copy could land on an identical piece spelled
    # differently in the original (a declaration here), or on a comment the
    # caller typed the filler into.
    it "takes each prelude from the rule itself, not an identical piece elsewhere" do
      sheet = "q{--v: b:LEXBOR-CONTAINS(1 2)} b:lexbor-contains(1 2){}"
      expect(Makiri::Lexbor::CSS.parse_stylesheet(sheet).last[:selector_text]).to eq("b:lexbor-contains(1 2)")
      sheet = "/* a:zzzzzzzzzzzzzzz(1 2) */ a:lexbor-contains(1 2){}"
      expect(Makiri::Lexbor::CSS.parse_stylesheet(sheet).last[:selector_text]).to eq("a:lexbor-contains(1 2)")
    end

    it "shows a declaration value as written, not with the guard's rewrite" do
      sheet = "a{--x: :lexbor-contains(1 2); background: url(x:lexbor-contains(1 2))}"
      values = Makiri::Lexbor::CSS.parse_stylesheet(sheet).first[:declarations].map { |d| d[:value] }
      expect(values).to eq([":lexbor-contains(1 2)", "url(x:lexbor-contains(1 2))"])
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
    let(:nested) { Makiri.HTML("<body>#{"<span>" * 16_000}", max_tree_depth: -1) }

    {
      "a node's string-value, per candidate" => "count(//span[. = 'x'])",
      "lang()'s ancestor climb, per candidate" => "count(//span[lang('en')])"
    }.each do |what, expr|
      it "stops #{what}" do
        took = elapsed { expect { nested.xpath(expr) }.to raise_error(Makiri::XPath::LimitExceeded) }
        expect(took).to be < 5.0
      end
    end

    # Counting preceding siblings per candidate was n^2 over a flat list: 40k
    # ran for 15 s uncharged, and once charged a 10k-entry feed hit the
    # budget. The CSS lowering now reads a per-parent memo of positions.
    it "answers structural pseudo-classes over a long XML sibling list" do
      xml = Makiri::XML("<r>#{"<a/>" * 40_000}</r>")
      {
        "r > *:nth-of-type(2)" => 1, "a:nth-child(2n)" => 20_000, "a:first-of-type" => 1,
        "a:nth-last-child(3n+1)" => 13_334, "a:last-of-type" => 1, "a:only-child" => 0
      }.each do |selector, count|
        took = elapsed { expect(xml.css(selector).size).to eq(count), selector }
        expect(took).to be < 1.0
      end
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

  # Splitting "]]>" across CDATA sections searched the rest of the value with a
  # UTF-8 aware search that re-validated it on every match: 256k of them (3.8
  # MB) took 6.3 s to write.
  describe "writing a CDATA value full of ]]>" do
    it "is linear in the value" do
      doc = Makiri::XML("<r><![CDATA[#{"]]]]><![CDATA[>" * 256_000}]]></r>")
      expect(elapsed { doc.to_xml }).to be < 1.0
      expect(Makiri::XML(doc.to_xml).root.children.first.content).to eq(doc.root.children.first.content)
    end
  end
end
