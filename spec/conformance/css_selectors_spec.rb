# frozen_string_literal: true

# CSS Selectors conformance - a resident, browser-authoritative regression for
# Makiri's CSS query surface (Node#css, backed by Lexbor's selector engine).
# Pure Ruby, no Nokogiri, so it runs under `rake spec`. The matching engine is
# Lexbor's (mature); these specs pin the supported selector surface and Makiri's
# glue (descendant-only scope, document order, comma de-duplication), and record
# the deliberate non-support of jQuery extensions plus one known Lexbor
# divergence (class/id case-sensitivity).
#
# Companion differential: spec/conformance/css_diff.rb (vs Nokogiri::HTML5).

RSpec.describe "CSS Selectors conformance" do
  let(:doc) do
    Makiri::HTML(<<~HTML)
      <!DOCTYPE html><html><body>
        <main id="main" class="container">
          <section class="list">
            <ul>
              <li class="item first" data-id="1"><a href="/p/1">one</a></li>
              <li class="item" data-id="2"><a href="/p/2" rel="next">two</a></li>
              <li class="item last" data-id="3"><span>three</span></li>
            </ul>
          </section>
          <p class="lead">intro</p>
          <p>body</p>
          <div class="empty"></div>
        </main>
      </body></html>
    HTML
  end

  def ids(set) = set.map { |n| n["data-id"] || n["id"] || n["class"] || n.name }

  describe "type / universal / class / id" do
    it "matches by type, class, id, and compound" do
      expect(doc.css("li").length).to eq(3)
      expect(doc.css(".item").length).to eq(3)
      expect(doc.css(".item.first").map { |n| n["data-id"] }).to eq(["1"])
      expect(doc.css("#main").map { |n| n["id"] }).to eq(["main"])
      expect(doc.css("main.container").length).to eq(1)
    end

    it "matches type selectors case-insensitively (HTML)" do
      expect(doc.css("LI").length).to eq(3) # CSS: type selectors are ASCII case-insensitive in HTML
    end
  end

  describe "combinators" do
    it "descendant / child / adjacent-sibling / general-sibling" do
      expect(doc.css("ul li").length).to eq(3)
      expect(doc.css("ul > li").length).to eq(3)
      expect(doc.css("li > a").length).to eq(2)
      expect(doc.css("p + p").length).to eq(1)
      expect(doc.css("li ~ li").length).to eq(2)
    end
  end

  describe "attribute selectors" do
    it "supports presence and all value operators" do
      expect(doc.css("[data-id]").length).to eq(3)
      expect(doc.css("a[rel]").length).to eq(1)
      expect(doc.css("[data-id='2']").map { |n| n["data-id"] }).to eq(["2"])
      expect(doc.css("[href^='/p']").length).to eq(2)
      expect(doc.css("[href$='/2']").length).to eq(1)
      expect(doc.css("[href*='p/']").length).to eq(2)
      expect(doc.css("[class~='first']").length).to eq(1)
      # |= matches the whole value being 'item' or starting 'item-'; the middle
      # <li class="item"> qualifies, the space-separated ones do not.
      expect(doc.css("[class|='item']").map { |n| n["data-id"] }).to eq(["2"])
    end
  end

  describe "structural pseudo-classes" do
    it "child/of-type position and :empty/:root" do
      expect(doc.css("li:first-child").map { |n| n["data-id"] }).to eq(["1"])
      expect(doc.css("li:last-child").map { |n| n["data-id"] }).to eq(["3"])
      expect(doc.css("li:nth-child(2)").map { |n| n["data-id"] }).to eq(["2"])
      expect(doc.css("li:nth-child(odd)").map { |n| n["data-id"] }).to eq(%w[1 3])
      expect(doc.css("p:first-of-type").map(&:text)).to eq(["intro"])
      expect(doc.css("p:nth-of-type(2)").map(&:text)).to eq(["body"])
      expect(doc.css("div:empty").length).to eq(1)
      expect(doc.css(":root").map(&:name)).to eq(["html"])
    end
  end

  describe "negation, matches-any, relational, grouping" do
    it ":not / :is / :where / :has / comma grouping" do
      expect(doc.css("li:not(.first)").length).to eq(2)
      expect(doc.css(":is(p, span)").length).to eq(3)     # 2 p + 1 span (Level 4)
      expect(doc.css(":where(p, span)").length).to eq(3)
      expect(doc.css("main:has(> section)").length).to eq(1)
      expect(doc.css("p, span").length).to eq(3)
      # comma lists are de-duplicated (a node matched by both arms appears once)
      expect(doc.css("li, .item").length).to eq(3)
    end
  end

  describe "Makiri glue semantics" do
    it "is descendant-only (the context node is excluded)" do
      main = doc.at_css("#main")
      expect(main.css("#main")).to be_empty       # self not matched
      expect(main.css(".container")).to be_empty
      expect(main.css("li").length).to eq(3)      # descendants matched
    end

    it "returns matches in document order, not grouped by selector arm" do
      # `a` and `li` interleave in the tree; the result is document-ordered
      # regardless of the order of the comma arms.
      expect(doc.css("a, li").map(&:name)).to eq(%w[li a li a li])
    end

    it "at_css returns the first match in document order" do
      expect(doc.at_css("li")["data-id"]).to eq("1")
    end

    it "raises CSS::SyntaxError on a malformed selector" do
      expect { doc.css("li[") }.to raise_error(Makiri::CSS::SyntaxError)
    end
  end

  describe "deliberately unsupported (standards-only engine)" do
    # jQuery / Nokogiri CSS extensions are not standard CSS; Lexbor (and thus
    # Makiri) rejects them. Use XPath or Enumerable instead, e.g.
    # doc.xpath("//p[contains(., 'intro')]") / doc.css('li')[1].
    it "rejects jQuery extension pseudo-classes" do
      %w[p:contains('intro') li:gt(0) li:eq(1) li:first li:last].each do |sel|
        expect { doc.css(sel) }.to raise_error(Makiri::CSS::SyntaxError), sel
      end
    end

    it "rejects pseudo-elements (they select no element nodes)" do
      expect { doc.css("p::before") }.to raise_error(Makiri::CSS::SyntaxError)
    end
  end

  describe "known divergence from browsers (Lexbor behaviour)" do
    it "class/id selectors are case-sensitive in no-quirks mode" do
      pending "Lexbor matches class/id case-insensitively regardless of quirks mode; " \
              "browsers (no-quirks) and Nokogiri::HTML5 are case-sensitive"
      expect(doc.css(".ITEM")).to be_empty # browser/no-quirks: 'ITEM' != 'item'
    end
  end
end
