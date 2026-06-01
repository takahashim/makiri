# frozen_string_literal: true

# Web Platform Tests — XPath over HTML (the `domxpath/` suite), ported.
#
# WPT's domxpath tests drive browsers' document.evaluate over the HTML DOM and
# are the de-facto reference for "XPath over HTML5". They are browser JS
# (document.evaluate + testharness.js), not a portable data corpus, so the
# semantic cases are hand-ported here as plain Makiri assertions (no Nokogiri;
# this runs under `rake spec`). Source:
#   https://github.com/web-platform-tests/wpt/tree/master/domxpath
#
# Scope: the evaluation-semantics subset. The DOM Level 3 XPath *API* tests
# (XPathEvaluator / resolver callbacks / XPathResult / iterateNext), Shadow DOM,
# cross-realm and crash tests are out of scope (they target a browser API
# Makiri does not mirror).
#
# Known intentional deviations from browser behaviour are marked `pending` with
# a reason, so the suite records them honestly AND flags us (a pending example
# that starts passing fails the run) if Makiri ever aligns:
#   * HTML name/attribute tests are case-SENSITIVE in Makiri (and Nokogiri::HTML5);
#     browsers fold ASCII case for HTML. e.g. //DiV, //*[@Id].
#   * Makiri's XPath lexer accepts ASCII NCNames only; a non-ASCII element/
#     attribute name in a name test raises SyntaxError (browsers accept it).
#   * xmlns="..." is a normal attribute to Makiri; browsers hide it from XPath.

RSpec.describe "WPT domxpath (XPath over HTML)" do
  XHTML_NS = "http://www.w3.org/1999/xhtml"
  SVG_NS   = "http://www.w3.org/2000/svg"

  # ------------------------------------------------------------------
  # text-html-elements.html
  # ------------------------------------------------------------------
  describe "element name tests (text-html-elements)" do
    let(:doc) do
      Makiri::HTML(<<~HTML)
        <html><body>
          <div id="log"><span>x</span></div>
          <div><span>y</span></div>
          <svg><path id="a"/><path id="b"/></svg>
        </body></html>
      HTML
    end
    let(:ns) do
      ctx = Makiri::XPathContext.new(doc)
      ctx.register_namespace("html", XHTML_NS)
      ctx.register_namespace("svg", SVG_NS)
      ctx
    end

    it "//div matches HTML elements (unprefixed resolves in the HTML namespace)" do
      expect(doc.xpath("//div").length).to eq(2)
    end

    it "//html:div matches the same elements via the HTML-namespace prefix" do
      expect(ns.evaluate("//html:div").length).to eq(2)
      expect(ns.evaluate("//html:div/span").length).to eq(2)
    end

    it "//path does NOT match SVG elements unprefixed (strict default)" do
      expect(doc.xpath("//path")).to be_empty
      expect(doc.xpath("//svg")).to be_empty
    end

    it "//svg:path matches SVG elements via the SVG-namespace prefix" do
      expect(ns.evaluate("//svg:path").length).to eq(2)
    end

    it "prefixed name tests are case-sensitive (//svg:PatH matches nothing)" do
      expect(ns.evaluate("//svg:PatH")).to be_empty
    end

    it "unprefixed lax mode finds SVG elements by local name" do
      expect(doc.xpath("//path", namespace_matching: :lax).length).to eq(2)
    end

    it "an unknown namespace prefix is rejected" do
      expect { doc.xpath("//invalid:path") }.to raise_error(Makiri::Error)
    end

    it "accepts a non-ASCII element name test (XML NCName, not ASCII-only)" do
      d = Makiri::HTML("<body><dØdd>x</dØdd></body>")
      expect(d.xpath("//dØdd").map(&:text)).to eq(["x"])
      expect(d.xpath("//dødd")).to be_empty # a different non-ASCII letter
    end

    it "HTML name tests fold ASCII case (//DiV == //div; //DØDD == //dØdd)" do
      pending "Makiri (like Nokogiri::HTML5) is case-sensitive; browsers fold HTML case"
      expect(doc.xpath("//DiV").length).to eq(2)
      d = Makiri::HTML("<body><dØdd></dØdd></body>")
      expect(d.xpath("//DØDD").length).to eq(1)
    end
  end

  # ------------------------------------------------------------------
  # text-html-attributes.html
  # ------------------------------------------------------------------
  describe "attribute tests (text-html-attributes)" do
    let(:doc) do
      Makiri::HTML(<<~HTML)
        <html><body>
          <div id="log" foo="bar"></div>
          <svg><path id="a" refx="1"/><path id="b" xlink:href="u"/></svg>
        </body></html>
      HTML
    end
    let(:ns) do
      ctx = Makiri::XPathContext.new(doc)
      ctx.register_namespace("svg", SVG_NS)
      ctx.register_namespace("xlink", "http://www.w3.org/1999/xlink")
      ctx
    end

    it "matches an attribute predicate exactly (//div[@id='log'])" do
      expect(doc.xpath("//div[@id='log']").length).to eq(1)
    end

    it "//*[@id] matches every element with an id, across namespaces" do
      # div#log plus both SVG paths (the predicate is namespace-agnostic)
      expect(doc.xpath("//*[@id]").length).to eq(3)
    end

    it "the attribute axis matches a no-namespace attribute on any element" do
      # `id` is a null-namespace attribute even on an <svg> child
      expect(doc.xpath("//@id").length).to eq(3)
    end

    it "a prefixed foreign attribute is reachable via its prefix" do
      expect(ns.evaluate("//svg:path[@xlink:href]").length).to eq(1)
    end

    it "attribute matching is uniformly case-sensitive (predicate and axis)" do
      # Exact case matches; wrong case does not — consistently across the
      # [@attr] predicate and the attribute axis (= XPath 1.0 = Nokogiri::HTML5).
      expect(doc.xpath("//div[@id='log']").length).to eq(1)
      expect(doc.xpath("//div[@Id='log']")).to be_empty
      expect(doc.xpath("//@id").length).to eq(3) # div + both svg path ids
      expect(doc.xpath("//@Id")).to be_empty
    end

    it "HTML attribute name tests fold ASCII case (//div[@Id] == //div[@id])" do
      pending "Makiri (like Nokogiri::HTML5) is case-sensitive everywhere; browsers fold HTML case"
      expect(doc.xpath("//div[@Id='log']").length).to eq(1)
    end

    it "hides xmlns from attribute tests" do
      pending "Makiri exposes xmlns as a normal attribute; browsers hide it"
      d = Makiri::HTML('<html><body><svg xmlns="x"></svg></body></html>')
      expect(d.xpath("//*[@xmlns]")).to be_empty
    end
  end

  # ------------------------------------------------------------------
  # elements-are-parents-of-their-attributes.html
  # ------------------------------------------------------------------
  describe "attributes' parent (elements-are-parents-of-their-attributes)" do
    let(:ctx) { Makiri::HTML('<html><body><div id="context" foo="bar"></div></body></html>').at_css("#context") }

    it "an element is the parent of its attribute (@foo/parent::*)" do
      expect(ctx.xpath("@foo/parent::*").map(&:name)).to eq(["div"])
    end

    it "attributes are not children (node() does not select them)" do
      expect(ctx.xpath("node()")).to be_empty
    end
  end

  # ------------------------------------------------------------------
  # numbers.html / booleans.html  (context: 3 <span> + 2 <br> children)
  # ------------------------------------------------------------------
  describe "numeric and boolean operators (numbers / booleans)" do
    let(:ctx) do
      Makiri::HTML("<body><div><span>1</span><span>2</span><span>3</span><br><br></div></body>")
        .at_css("div")
    end

    it "arithmetic operators (+ - * div mod)" do
      expect(ctx.xpath("count((./span)[1]) + count(./br)")).to eq(3)
      expect(ctx.xpath("count((./span)[1]) - count(./br)")).to eq(-1)
      expect(ctx.xpath("count((./span)[1]) * count(./br)")).to eq(2)
      expect(ctx.xpath("count((./span)[1]) div count(./br)")).to eq(0.5)
      expect(ctx.xpath("count((./span)[1]) mod count(./br)")).to eq(1)
    end

    it "boolean and relational operators" do
      expect(ctx.xpath("(./span)[4] or ./br[2]")).to be(true)
      expect(ctx.xpath("count((./span)[3]) = count(./br[2])")).to be(true)
      expect(ctx.xpath("count((./span)[3]) != count(./br[2])")).to be(false)
      expect(ctx.xpath("count((./span)[3]) < count(./br)")).to be(true)
      expect(ctx.xpath("count((./span)[3]) > count(./br[2])")).to be(false)
      expect(ctx.xpath("count((./span)[3]) >= count(./br)")).to be(false)
      expect(ctx.xpath("count((./span)[3]) <= count(./br[2])")).to be(true)
    end
  end

  # ------------------------------------------------------------------
  # lexical-structure.html
  # ------------------------------------------------------------------
  describe "lexical structure (lexical-structure)" do
    let(:doc) { Makiri::HTML("<p>x</p>") }

    it "accepts ASCII straight quotes as literal delimiters, with the other embedded" do
      expect(doc.xpath(%q{'a"bc'})).to eq('a"bc')
      expect(doc.xpath(%Q{"a'bc"})).to eq("a'bc")
    end

    it "rejects non-ASCII (smart) quotes as literal delimiters" do
      expect { doc.xpath("’xyz’") }.to raise_error(Makiri::XPath::SyntaxError)
    end

    it "treats only #x20/#x9/#xD/#xA as expression whitespace" do
      expect(doc.xpath("\t\r\n.\r\n\t")).not_to be_empty            # ASCII whitespace: ok
      expect { doc.xpath("\x0B\x0C .") }.to raise_error(Makiri::XPath::SyntaxError) # \v \f
      expect { doc.xpath("　 .") }.to raise_error(Makiri::XPath::SyntaxError)   # ideographic space
      expect { doc.xpath("  .") }.to raise_error(Makiri::XPath::SyntaxError)   # paragraph sep
    end
  end

  # ------------------------------------------------------------------
  # node-sets.html / predicates.html / node-set-tree-order.html
  # ------------------------------------------------------------------
  describe "node-set operators and predicates" do
    it "union evaluates both sides against the same context node (node-sets)" do
      doc = Makiri::HTML("<html><body><div></div></body></html>")
      result = doc.root.xpath("(.//div)[1]|.")
      expect(result.map(&:name)).to eq(%w[html div]) # context node + first descendant div
    end

    it "a predicate sub-expression does not change the outer context (predicates)" do
      doc = Makiri::HTML("<body><table></table>" \
                         "<table><tr><th></th><th></th><th></th><th></th></tr></table>" \
                         "<table></table></body>")
      tables = doc.xpath("//table").to_a
      # predicate = [count((//table)[2]//th) - 1] = [4 - 1] = [3] -> the third table
      result = doc.xpath("(//table)[count((//table)[2]/descendant::th)-1]")
      expect(result.length).to eq(1)
      expect(result.first).to eq(tables.last)
    end

    it "temporary node-sets from a union are in document order (node-set-tree-order)" do
      ctx = Makiri::HTML('<div id="container"><span></span><p id="p"></p></div>').at_css("#container")
      # (./p | ./span) is ordered [span, p] in tree order, so last() is the <p>
      result = ctx.xpath("(./p | ./span)[last()]")
      expect(result.map { |n| n["id"] }).to eq(["p"])
    end
  end

  # ------------------------------------------------------------------
  # fn-*.html  (string functions, mostly with context-dependent arguments)
  # ------------------------------------------------------------------
  describe "string functions (fn-*)" do
    it "substring / substring-before / substring-after / translate" do
      c = Makiri::HTML("<body><div><span>^^^bar$$$</span><b>bar</b><b>foo</b>" \
                       "<br><br><br><br></div></body>").at_css("div")
      expect(c.xpath("substring((./span)[1], count(./br))")).to eq("bar$$$")
      expect(c.xpath("substring-before((./span)[1], ./b)")).to eq("^^^")
      expect(c.xpath("substring-after((./span)[1], ./b)")).to eq("$$$")
      expect(c.xpath("translate((./span)[1], (./b)[1], ./b[2])")).to eq("^^^foo$$$")
    end

    it "concat" do
      c = Makiri::HTML("<body><div><span>foo</span><span>bar</span><b>ber</b></div></body>").at_css("div")
      expect(c.xpath("concat((./span)[2], ./b)")).to eq("barber")
    end

    it "contains" do
      c = Makiri::HTML("<body><div><span>bar bar</span><span>bar<b>ber</b></span><b>bar</b></div></body>").at_css("div")
      expect(c.xpath("contains((./span)[1], ./b)")).to be(true)
    end

    it "starts-with" do
      c = Makiri::HTML("<body><div><span>foo</span><span>bar<b>ber</b></span><b>bar</b></div></body>").at_css("div")
      expect(c.xpath("starts-with((./span)[2], ./b)")).to be(true)
    end

    it "normalize-space normalizes only #x20/#x9/#xD/#xA" do
      doc = Makiri::HTML("<body><p> a <br> b</p></body>")
      expect(doc.at_css("p").xpath("normalize-space()")).to eq("a b")
      expect(doc.xpath("normalize-space(' a \t  b\r\nc ')")).to eq("a b c")
      # \v \f and other control/space chars are NOT XPath whitespace: left intact
      expect(doc.xpath("normalize-space('y\x0B\x0C\x0E\x0Fz')")).to eq("y\x0B\x0C\x0E\x0Fz")
      expect(doc.xpath("normalize-space('  　')")).to eq("  　")
    end
  end

  # ------------------------------------------------------------------
  # fn-id.html / fn-lang.html  (adapted to the HTML id / lang attributes)
  # ------------------------------------------------------------------
  describe "id() and lang()" do
    let(:doc) do
      Makiri::HTML('<body><div id="test1">a</div><div id="test2">b</div>' \
                   '<div id="test-1">c</div></body>')
    end

    it "id() resolves IDs, splits on whitespace, and is case-sensitive" do
      expect(doc.xpath('id("test1")').map { |n| n["id"] }).to eq(["test1"])
      expect(doc.xpath('id("test1 test2")').map { |n| n["id"] }).to eq(%w[test1 test2])
      expect(doc.xpath('id("test-1")').map { |n| n["id"] }).to eq(["test-1"])
      expect(doc.xpath('id(" test1 ")').map { |n| n["id"] }).to eq(["test1"]) # trimmed/split
      expect(doc.xpath('id("Test1")')).to be_empty   # case-sensitive
      expect(doc.xpath('id("nonexistent")')).to be_empty
    end

    it "lang() matches the language attribute, inheriting and case-insensitive" do
      el = Makiri::HTML('<body><div lang="en-US"><p>x</p></div></body>').at_css("p")
      expect(el.xpath('lang("en")')).to be(true)   # prefix match + inherited
      expect(el.xpath('lang("EN")')).to be(true)   # case-insensitive
      expect(el.xpath('lang("fr")')).to be(false)
      none = Makiri::HTML("<body><p>x</p></body>").at_css("p")
      expect(none.xpath('lang("en")')).to be(false)
    end
  end
end
