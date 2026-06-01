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
end
