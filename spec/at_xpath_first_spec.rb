# frozen_string_literal: true

# Node#at_xpath has an engine-level first-match short-circuit: for the common
# "first descendant by name (+ a simple [@attr] / [@attr='lit'] predicate)"
# shapes it walks the subtree in document order and stops at the first match
# instead of building the whole node-set and taking [0]. The result MUST stay
# byte-identical to xpath(e).first; everything outside the recognised subset
# (positional predicates, functions, reverse axes, unions, prefixes, longer
# paths) falls back to the full evaluator. These examples pin that invariant.
RSpec.describe "Makiri Node#at_xpath first-match short-circuit" do
  let(:html) do
    <<~HTML
      <!doctype html><html><body>
      <main id="main" class="c"><section class="list">
        <div class="a" data-x="1" title="">one</div>
        <p class="a">p1</p>
        <div class="a" data-x="2">two</div>
        <span class="a" data-x="2">sp</span>
        <svg><path d="z"/><rect/></svg>
        <!-- cm -->
        <div class="b">b</div>
      </section></main>
      <footer id="foot"><div class="a" data-x="9">f</div></footer>
      </body></html>
    HTML
  end
  let(:doc) { Makiri::HTML(html) }
  let(:main) { doc.at_css("#main") }

  # Recognised (short-circuited) shapes.
  recognised = [
    "//div", "//div[@class]", "//div[@class='a']", "//*[@data-x='2']",
    "//*[@id='foot']", ".//div", ".//div[@data-x='2']", "descendant::div",
    "descendant::div[@class='a']", "//div[@data-x='9']", "//*[@data-x]",
    "//p[@class='a']", "//*[@title]", "//div[@class='b']", "//comment()",
    "//text()", "//*", "//node()", "//div[@data-x='1'][@title]",
    "//nope", "//div[@nope='x']"
  ]
  # Outside the subset -> must fall back, still identical.
  fallback = [
    "//div[1]", "//div[last()]", "//div[position()=2]", "//div/p",
    "//main//div", "//div | //p", "//*[contains(@class,'a')]",
    "//*[@id='main']/div", "//svg/*", "//*[@data-x='2'][1]"
  ]

  # Compare wrappers by underlying node (Makiri does not cache per-node wrappers,
  # so xpath.first and at_xpath return distinct objects for the same DOM node).
  def same_node?(a, b)
    return true if a.nil? && b.nil?
    return false if a.nil? || b.nil?
    a == b
  end

  [%w[doc], %w[main]].each do |ctx_name|
    (recognised + fallback).each do |expr|
      it "at_xpath(#{expr.inspect}) == xpath(#{expr.inspect}).first on #{ctx_name}" do
        ctx = send(ctx_name.first)
        expect(same_node?(ctx.at_xpath(expr), ctx.xpath(expr).first)).to be(true)
      end
    end
  end

  it "returns nil for an absent recognised shape" do
    expect(doc.at_xpath("//aside[@id='ghost']")).to be_nil
  end

  it "honours namespace_matching: :lax on the short-circuit path" do
    %w[//path //svg //rect].each do |e|
      expect(same_node?(doc.at_xpath(e, namespace_matching: :lax),
                        doc.xpath(e, namespace_matching: :lax).first)).to be(true)
    end
  end

  it "leaves :strict (default) unprefixed foreign tests empty" do
    expect(doc.at_xpath("//path")).to be_nil       # SVG path not matched without prefix
    expect(doc.at_xpath("//div")).not_to be_nil    # HTML div is
  end

  it "still returns scalars unchanged (non-node-set expressions)" do
    expect(doc.at_xpath("count(//div)")).to eq(doc.xpath("count(//div)"))
    expect(doc.at_xpath("string(//div)")).to eq(doc.xpath("string(//div)"))
  end

  it "returns the first attribute node for an attribute-leaf path" do
    expect(same_node?(doc.at_xpath("//div/@data-x"),
                      doc.xpath("//div/@data-x").first)).to be(true)
  end

  it "respects the context node for a relative short-circuit" do
    # .//div from <main> must not reach the <footer> div, and must be the first
    # div inside main.
    expect(main.at_xpath(".//div")).to eq(main.xpath(".//div").first)
    expect(main.at_xpath(".//div")["data-x"]).to eq("1")
  end
end
