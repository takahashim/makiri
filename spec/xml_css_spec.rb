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

    it "selects by a prefixed attribute (namespace-only selector)" do
      # An attribute-only selector has no bare type, so the explicit ns hash does
      # not change its element matching.
      expect(atom.css("[dc|role=\"lead\"]", "dc" => "urn:dc").length).to eq(1)
    end

    it "disables the default-namespace binding once an explicit ns hash is given" do
      # Nokogiri-compatible: with an explicit namespaces hash, a bare type
      # selector resolves to NO namespace (it no longer auto-binds the default),
      # so the default-ns entries are not matched...
      expect(atom.css("entry[dc|role]", "dc" => "urn:dc").length).to eq(0)
      # ...and the default namespace must be registered under a prefix to match.
      expect(atom.css("a|entry[dc|role]", "a" => "urn:atom", "dc" => "urn:dc").length).to eq(1)
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

    it ":not / :is / :where (compound args)" do
      d = Makiri::XML(%(<r><a class="x"/><a/><b/></r>))
      expect(d.css("a:not(.x)").length).to eq(1)
      expect(d.css(":is(a, b)").length).to eq(3)
      expect(d.css(":where(b)").length).to eq(1)
    end

    it ":not / :is / :where with combinator arguments" do
      d = Makiri::XML(%(<r><nav><a id="n"/></nav><sec><a id="s"/><a id="k" class="k"/></sec><a id="t"/></r>))
      ids = ->(sel) { d.css(sel).map { |e| e["id"] } }
      expect(ids.("a:not(nav a)")).to eq(%w[s k t])          # exclude descendants of nav
      expect(ids.(":is(nav a, sec a)")).to eq(%w[n s k])     # union of two complex args
      expect(ids.(":is(sec > a.k)")).to eq(%w[k])            # child + compound
      expect(ids.("a:not(sec a):not(nav a)")).to eq(%w[t])   # chained :not
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
    end
  end

  describe "untyped of-type pseudo-classes (no explicit type selector)" do
    # The "type" is each element's own expanded name (local name + namespace
    # URI). Pure XPath 1.0 cannot express that self-comparison (no current()), so
    # the engine resolves it natively; the result matches Lexbor's HTML matcher.
    # Nokogiri (XML and HTML5) mistranslates these to first-/only-child
    # (//*[position()=1] / //*[last()=1]) - Makiri is correct where Nokogiri is
    # not, so this is pinned here rather than in the Nokogiri differential.
    let(:doc) do
      Makiri::XML("<r><a>1</a><b>2</b><a>3</a><b>4</b><c>5</c></r>")
    end

    it ":first-of-type matches the first sibling of each element type" do
      expect(doc.css(":first-of-type").map(&:name)).to eq(%w[r a b c])
      expect(doc.css("r > :first-of-type").map(&:text)).to eq(%w[1 2 5]) # a(1), b(2), c(5)
    end

    it ":last-of-type matches the last sibling of each element type" do
      expect(doc.css("r > :last-of-type").map(&:text)).to eq(%w[3 4 5]) # a(3), b(4), c(5)
    end

    it ":only-of-type matches an element that is the sole one of its type" do
      expect(doc.css("r > :only-of-type").map(&:text)).to eq(%w[5]) # only <c>
    end

    it ":nth-of-type / :nth-last-of-type count within the element type" do
      expect(doc.css("r > :nth-of-type(2)").map(&:text)).to eq(%w[3 4])      # 2nd a, 2nd b
      expect(doc.css("r > :nth-last-of-type(1)").map(&:text)).to eq(%w[3 4 5])
      expect(doc.css("r > :nth-of-type(odd)").map(&:text)).to eq(%w[1 2 5])  # a(1),b(2),c(5)
    end

    it "*:first-of-type (explicit universal type) behaves like the untyped form" do
      expect(doc.css("*:first-of-type").map(&:name)).to eq(%w[r a b c])
    end

    it "treats the same local name in different namespaces as different types" do
      nsdoc = Makiri::XML(%(<r xmlns:x="urn:x"><a/><x:a/><a/></r>))
      # no-namespace <a> and <x:a> are distinct types: the first no-ns a and the
      # lone x:a are each first-of-type (plus the root).
      expect(nsdoc.css(":first-of-type").map(&:name)).to eq(%w[r a x:a])
      expect(nsdoc.css(":last-of-type").map(&:name)).to eq(%w[r x:a a])
    end

    it "agrees with Lexbor's HTML matcher on the same content" do
      # Explicit close tags so HTML parses the siblings flat (an empty <a/> is not
      # self-closing in HTML); the XML side here matches the same selector.
      markup = "<r><a>1</a><b>2</b><a>3</a><c>4</c></r>"
      r = Makiri::HTML("<html><body>#{markup}</body></html>").at_css("r")
      x = Makiri::XML(markup).at_css("r")
      %w[:first-of-type :last-of-type :only-of-type :nth-of-type(2)].each do |sel|
        expect(r.css(sel).map(&:name)).to eq(x.css(sel).map(&:name)), "selector #{sel}"
      end
    end
  end

  describe "compound correctness (cases a random differential caught Nokogiri::XML failing)" do
    let(:doc) do
      Makiri::XML(%(<catalog><book id="b1" class="lead big"><title lang="en">A</title></book>) +
                  %(<book id="b2" class="big"><title lang="fr">B</title></book><note/></catalog>))
    end

    it ":not(type.class) honours the whole compound (not just the type)" do
      # No element has class 'x', so :not(*.x) / :not(book.x) match everything,
      # books included. (Nokogiri drops the .x and emits not(self::book).)
      expect(doc.css(":not(*.x)").map(&:name)).to eq(%w[catalog book title book title note])
      expect(doc.css(":not(book.x)").map(&:name)).to include("book")
      expect(doc.css("book:not(.lead)").map { |e| e["id"] }).to eq(%w[b2])
    end

    it "ANDs an attribute operator with a sibling class in one compound" do
      # [lang|="en"].lead needs BOTH; the lang=en title has no class, so no match.
      # (Nokogiri emits `@lang=.. or starts-with(..) and contains(..class..)`,
      # whose operator precedence wrongly matches the title on lang alone.)
      expect(doc.css('[lang|="en"].lead')).to be_empty
      expect(doc.css('title[lang|="en"]').map { |e| e.text }).to eq(%w[A])
    end
  end

  describe "the |el (no-namespace) selector fails closed" do
    # Lexbor's parser cannot represent `|el` distinctly from `*|el` (both arrive
    # as ns="*"), so rather than silently matching any namespace, Makiri rejects
    # a leading-pipe type selector. A bare `el` or XPath `//el` matches the
    # no-namespace element correctly.
    let(:doc) { Makiri::XML(%(<r><wrap/><x xmlns:p="urn:b"><p:wrap/></x></r>)) }

    it "raises CSS::SyntaxError on |el (any position), not a wrong any-namespace match" do
      expect { doc.css("|wrap") }.to raise_error(Makiri::CSS::SyntaxError)
      expect { doc.css("r > |wrap") }.to raise_error(Makiri::CSS::SyntaxError)
      expect { doc.css(":is(|wrap)") }.to raise_error(Makiri::CSS::SyntaxError)
      expect { doc.at_css("|wrap") }.to raise_error(Makiri::CSS::SyntaxError)
    end

    it "still supports *|el (any namespace), bare el, and the [a|=v] operator" do
      expect(doc.css("*|wrap").length).to eq(2)                   # any namespace
      expect(doc.css("wrap").length).to eq(1)                     # bare: no-namespace here
      expect(doc.xpath("//wrap").length).to eq(1)
      expect(doc.css(%([id|="x"])).length).to eq(0)               # |= operator unaffected
    end
  end

  describe ":lexbor-contains() text containment (parity with the HTML side)" do
    # Makiri's HTML CSS exposes Lexbor's :lexbor-contains() jQuery-style text
    # filter; XML matches it by lowering to XPath contains() on the element's
    # string-value, so the same selector works (and agrees) on both hosts.
    let(:doc) do
      Makiri::XML("<r><i>apple PIE</i><i>banana</i><i>cherry pie</i></r>")
    end

    it "matches the substring case-sensitively" do
      expect(doc.css(%(i:lexbor-contains("pie"))).map(&:text)).to eq(["cherry pie"])
      expect(doc.css(%(i:lexbor-contains("PIE"))).map(&:text)).to eq(["apple PIE"])
    end

    it "is ASCII case-insensitive with the ` i` flag" do
      expect(doc.css(%(i:lexbor-contains("pie" i))).map(&:text))
        .to eq(["apple PIE", "cherry pie"])
    end

    it "matches a direct child text node like XPath child::text()[contains()]" do
      expect(doc.css(%(i:lexbor-contains("pie"))).map(&:text))
        .to eq(doc.xpath(%(//i[text()[contains(., "pie")]])).map(&:text))
    end

    it "scans only immediate child text nodes, not the deep string-value" do
      # Faithful to Lexbor's matcher: <r> contains "cherry" only via a descendant
      # text node, not a direct child one, so <r> does NOT match - only <i> does.
      # (This is what makes HTML and XML agree; the deep string-value would also
      # match every ancestor.)
      nested = Makiri::XML("<r><i>cherry pie</i></r>")
      expect(nested.css(%(:lexbor-contains("cherry"))).map(&:name)).to eq(%w[i])
      html = Makiri::HTML("<html><body><r><i>cherry pie</i></r></body></html>")
      expect(html.css(%(:lexbor-contains("cherry"))).map(&:name))
        .to eq(nested.css(%(:lexbor-contains("cherry"))).map(&:name))
    end
  end
end
