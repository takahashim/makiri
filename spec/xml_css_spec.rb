# frozen_string_literal: true

require "spec_helper"

# CSS selector queries over Makiri::XML, lowered to the native XPath engine
# (ext/makiri/xpath/mkr_css.c). Unlike an HTML matcher, matching is
# case-sensitive and namespace-aware; bare type selectors bind to the document's
# default namespace (Nokogiri-compatible).
RSpec.describe "Makiri::XML CSS selectors" do
  describe "type selectors (case-sensitive)" do
    let(:doc) { Makiri::XML("<Root><Item/><Item/><item/></Root>") }

    it "matches element names exactly, preserving case" do
      expect(doc.css("Item").length).to eq(2)
      expect(doc.css("item").length).to eq(1) # distinct from Item
      expect(doc.css("*").length).to eq(4)    # Root + 3 children (descendants of doc)
    end

    it "#at_css returns the first match or nil" do
      expect(doc.at_css("Item")).to eq(doc.root.children.first)
      expect(doc.at_css("nope")).to be_nil
    end
  end

  describe "namespaces (Nokogiri-compatible)" do
    let(:atom) do
      Makiri::XML(<<~XML)
        <feed xmlns="urn:atom" xmlns:dc="urn:dc">
          <entry dc:role="lead"><title>A</title></entry>
          <entry><title>B</title></entry>
          <dc:entry>meta</dc:entry>
        </feed>
      XML
    end

    it "binds a bare type selector to the default namespace" do
      expect(atom.css("entry").length).to eq(2) # the urn:atom entries, not dc:entry
      expect(atom.css("feed > entry").length).to eq(2)
      expect(atom.css("title").map(&:text)).to eq(%w[A B])
    end

    it "selects a prefixed element with a supplied namespace" do
      expect(atom.css("dc|entry", "dc" => "urn:dc").map(&:text)).to eq(["meta"])
    end

    it "selects a prefixed attribute" do
      expect(atom.css("entry[dc|role]", "dc" => "urn:dc").length).to eq(1)
      expect(atom.css("[dc|role=\"lead\"]", "dc" => "urn:dc").length).to eq(1)
    end

    it "matches no-namespace elements when there is no default namespace" do
      plain = Makiri::XML("<feed><entry/><entry/></feed>")
      expect(plain.css("entry").length).to eq(2)
    end
  end

  describe "class / id / attribute operators" do
    let(:doc) do
      Makiri::XML(%(<r><a id="x1" class="lead big" href="/p/1"/><a class="big" href="/q"/><a/></r>))
    end

    it "supports #id and .class (the literal id / class attributes)" do
      expect(doc.css("#x1").length).to eq(1)
      expect(doc.css(".lead").length).to eq(1)
      expect(doc.css(".big").length).to eq(2)
    end

    it "supports the attribute operators" do
      expect(doc.css("[href]").length).to eq(2)
      expect(doc.css("[class=\"big\"]").length).to eq(1)       # exact
      expect(doc.css("[class~=\"big\"]").length).to eq(2)      # token
      expect(doc.css("[href^=\"/p\"]").length).to eq(1)        # prefix
      expect(doc.css("[href$=\"1\"]").length).to eq(1)         # suffix
      expect(doc.css("[href*=\"q\"]").length).to eq(1)         # substring
    end
  end

  describe "combinators and lists" do
    let(:doc) { Makiri::XML("<r><a><b/></a><c/><d/><a/></r>") }

    it "descendant / child / adjacent / general sibling" do
      expect(doc.css("r b").length).to eq(1)            # descendant
      expect(doc.css("r > a").length).to eq(2)          # child
      expect(doc.css("c + d").length).to eq(1)          # adjacent
      expect(doc.css("a ~ d").length).to eq(1)          # general sibling
      expect(doc.css("c + a").length).to eq(0)          # d follows c, not a
    end

    it "comma lists union in document order with dedup" do
      expect(doc.css("c, d").length).to eq(2)
      expect(doc.css("a, a").length).to eq(2)           # not double-counted
    end
  end

  describe "structural and functional pseudo-classes" do
    let(:doc) { Makiri::XML("<r><a/><b/><a/><c/><a/></r>") }

    it ":first-child / :last-child / :only-child / :empty / :root" do
      # r is the document node's first child; a is r's first child - both match.
      expect(doc.css(":first-child").map(&:name)).to eq(%w[r a])
      expect(doc.css("r > :first-child").map(&:name)).to eq(%w[a])
      expect(doc.css("r > :last-child").map(&:name)).to eq(%w[a])
      expect(Makiri::XML("<r><lone/></r>").css("r > :only-child").map(&:name)).to eq(%w[lone])
      expect(doc.css("r:root").length).to eq(1)
      expect(doc.css("a:empty").length).to eq(3)
    end

    it ":nth-child(an+b) including keywords" do
      expect(doc.css("r > :nth-child(2)").map(&:name)).to eq(%w[b])
      expect(doc.css("r > :nth-child(2n)").map(&:name)).to eq(%w[b c]) # positions 2,4
      expect(doc.css("r > :nth-child(odd)").map(&:name)).to eq(%w[a a a]) # 1,3,5
    end

    it "of-type pseudo-classes (typed)" do
      expect(doc.css("a:first-of-type").length).to eq(1)
      expect(doc.css("a:last-of-type").length).to eq(1)
      expect(doc.css("a:nth-of-type(2)").length).to eq(1)
      expect(doc.css("a:only-of-type").length).to eq(0) # 3 a's
    end

    it ":not / :is / :where" do
      d = Makiri::XML(%(<r><a class="x"/><a/><b/></r>))
      expect(d.css("a:not(.x)").length).to eq(1)
      expect(d.css(":is(a, b)").length).to eq(3)
      expect(d.css(":where(b)").length).to eq(1)
    end

    it ":has with every combinator (descendant / child / adjacent / general sibling)" do
      d = Makiri::XML(%(<r><b/><c/><wrap><deep/></wrap><y/></r>))
      expect(d.css("wrap:has(deep)").length).to eq(1)   # descendant
      expect(d.css("wrap:has(> deep)").length).to eq(1) # child
      expect(d.css("b:has(+ c)").length).to eq(1)       # adjacent: c immediately follows b
      expect(d.css("b:has(+ wrap)").length).to eq(0)    # wrap is not immediately after b
      expect(d.css("b:has(~ y)").length).to eq(1)       # general sibling
      expect(d.css("r:has(nope)").length).to eq(0)
    end
  end

  describe "first-match short-circuit (prefixed name tests)" do
    let(:doc) do
      Makiri::XML("<feed xmlns=\"urn:a\">" +
                  (1..50).map { |i| "<entry><id>#{i}</id></entry>" }.join + "</feed>")
    end

    it "#at_css and #at_xpath stay byte-identical to the full set's first" do
      expect(doc.at_css("entry")).to eq(doc.css("entry").first)
      expect(doc.at_xpath("//a:entry", "a" => "urn:a"))
        .to eq(doc.xpath("//a:entry", "a" => "urn:a").first)
      expect(doc.at_css("entry").at_css("id").text).to eq("1")
    end

    it "raises on an unbound prefix rather than returning nil" do
      expect { doc.at_xpath("//z:entry") }.to raise_error(Makiri::Error)
    end
  end

  describe "#matches?" do
    let(:doc) { Makiri::XML(%(<r><a class="x"/><b/></r>)) }

    it "tests the receiver against a selector (combinators included)" do
      a = doc.at_css("a")
      expect(a.matches?("a.x")).to be(true)
      expect(a.matches?("r > a")).to be(true)
      expect(a.matches?("b")).to be(false)
      expect(doc.at_css("b").matches?("a + b")).to be(true)
    end
  end

  describe "NodeSet#css" do
    it "runs per node and unions the results" do
      doc = Makiri::XML("<r><g><i/></g><g><i/><i/></g></r>")
      expect(doc.css("g").css("i").length).to eq(3)
    end
  end

  describe "fail-closed errors" do
    let(:doc) { Makiri::XML("<r><a/></r>") }

    it "raises CSS::SyntaxError on a malformed selector" do
      expect { doc.css("a[") }.to raise_error(Makiri::CSS::SyntaxError)
      expect { doc.css(">>") }.to raise_error(Makiri::CSS::SyntaxError)
    end

    it "raises CSS::SyntaxError on an unsupported construct" do
      expect { doc.css("a::before") }.to raise_error(Makiri::CSS::SyntaxError)
      expect { doc.css("*:first-of-type") }.to raise_error(Makiri::CSS::SyntaxError) # untyped of-type
    end
  end
end
