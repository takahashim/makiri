# frozen_string_literal: true

# M7: CSS selector queries, matched by `lexbor::css_match` (a safe-Rust
# port of Lexbor's `lxb_selectors` state machine - selector PARSING still goes
# through Lexbor's own CSS parser, matching does not).
RSpec.describe "Makiri CSS" do
  let(:doc) do
    Makiri::HTML(<<~HTML)
      <html><body>
        <div id="main" class="container">
          <p class="x">one</p>
          <p class="y z">two</p>
          <a href="/l1">L1</a>
          <a href="/l2" class="x">L2</a>
          <ul><li>a</li><li>b</li><li>c</li></ul>
        </div>
      </body></html>
    HTML
  end

  # The pseudo-classes Lexbor answers from an attribute's presence keep
  # Lexbor's rule - the attribute by local name, in any namespace - as
  # `:any-link` does (`xlink:href`). Only a DOM write can make a namespaced
  # one; the parser never does on an `<input>`.
  describe "attribute-presence pseudo-classes" do
    it "find the attribute by its local name in any namespace, as Lexbor does" do
      d = Makiri::HTML("<input id=a><input id=b><textarea id=c></textarea>")
      d.at_css("#a").set_attribute_ns("urn:x", "x:required", "")
      d.at_css("#b").set_attribute_ns("urn:x", "READONLY", "")
      d.at_css("#c").set_attribute_ns("urn:x", "x:placeholder", "p")
      expect(d.css(":required").map { |e| e["id"] }).to eq(%w[a])
      expect(d.css(":optional").map { |e| e["id"] }).to eq(%w[b c])
      expect(d.css(":read-write").map { |e| e["id"] }).to eq(%w[a c])
      expect(d.css(":placeholder-shown").map { |e| e["id"] }).to eq(%w[c])
    end
  end

  describe "#css" do
    it "matches by type, class, and id" do
      expect(doc.css("p").map(&:text)).to eq(%w[one two])
      expect(doc.css(".x").map(&:text)).to eq(%w[one L2])
      expect(doc.css("#main")).to be_a(Makiri::NodeSet)
      expect(doc.css("#main").length).to eq(1)
    end

    it "supports combinators" do
      expect(doc.css("div.container > p").map(&:text)).to eq(%w[one two])
      expect(doc.css("ul li").map(&:text)).to eq(%w[a b c])
    end

    it "supports attribute selectors" do
      expect(doc.css("a[href]").map { |n| n["href"] }).to eq(%w[/l1 /l2])
      expect(doc.css('a[href="/l2"]').first.text).to eq("L2")
    end

    it "supports structural pseudo-classes" do
      expect(doc.css("li:nth-child(2)").map(&:text)).to eq(%w[b])
      expect(doc.css("li:first-child").map(&:text)).to eq(%w[a])
      expect(doc.css("li:last-child").map(&:text)).to eq(%w[c])
    end

    it "deduplicates across a selector list and stays in document order" do
      # <p class="x"> and <a class="x"> both match .x; p also matches p.
      expect(doc.css("p, .x").map(&:text)).to eq(%w[one two L2])
    end

    it "returns an empty NodeSet when nothing matches" do
      result = doc.css(".nope")
      expect(result).to be_a(Makiri::NodeSet)
      expect(result).to be_empty
    end

    it "searches descendants only, excluding the context node" do
      main = doc.at_css("#main")
      expect(main.css("div").length).to eq(0)       # #main is a div, excluded
      expect(main.css("p").map(&:text)).to eq(%w[one two])
    end
  end

  describe "#at_css" do
    it "returns the first match" do
      expect(doc.at_css("p").text).to eq("one")
      expect(doc.at_css(".x").text).to eq("one")
    end

    it "returns nil when nothing matches" do
      expect(doc.at_css(".nope")).to be_nil
    end
  end

  # NOKOGIRI_DIFFERENCES.md: a selector under a node matches the way
  # `Element#querySelectorAll` does in a browser - against the whole document,
  # with only the results filtered to descendants of the context node. So the
  # context node itself CAN satisfy an earlier compound in the selector, unlike
  # Nokogiri (and `Makiri::XML`, which lowers to an XPath scoped from the node).
  # Pinned here so a change of matcher (as the move from Lexbor's engine to
  # `lexbor::css_match` was) reproduces this, not "fix" it into XPath-style
  # scoping.
  describe "query scope (whole-document matching, not scoped to the context node)" do
    it "lets the context node itself satisfy an earlier compound" do
      c = doc.at_css("#main") # #main is itself a div
      expect(c.css("div p").map(&:text)).to eq(%w[one two])
    end
  end

  # Lexbor's matcher has no `:scope` (NOKOGIRI_DIFFERENCES.md). Pin the exact
  # failure so a change of matcher decides, rather than discovers, whether to
  # keep rejecting it or to implement it.
  describe ":scope" do
    it "is rejected as an unsupported CSS selector" do
      c = doc.at_css("#main")
      expect { c.css(":scope p") }.to raise_error(Makiri::CSS::SyntaxError)
      expect { c.at_css(":scope") }.to raise_error(Makiri::CSS::SyntaxError)
      expect { c.matches?(":scope") }.to raise_error(Makiri::CSS::SyntaxError)
    end
  end

  # NOKOGIRI_DIFFERENCES.md: `#matches?` answers for a detached node; Nokogiri
  # raises there because it implements `#matches?` as a search from
  # `ancestors.last`, which a detached node has none of.
  describe "detached nodes" do
    it "answers css/at_css/matches? without an owner tree" do
      detached = doc.create_element("p")
      expect(detached.css("p")).to be_empty       # no descendants to search
      expect(detached.at_css("p")).to be_nil
      expect(detached.matches?("p")).to be(true)  # self-match needs no tree
      expect(detached.matches?("div p")).to be(false) # no ancestor to satisfy "div"
    end
  end

  # Tag and attribute names are resolved to Lexbor's interned ids once per
  # query, in the queried document - so a name the document only gained by
  # mutation after the parse (a new custom element, a new attribute name) must
  # still be found, by every entry point, in either case.
  describe "names added after the parse" do
    it "finds a new element name and a new attribute name" do
      d = Makiri::HTML("<!doctype html><body><p>x</p></body>")
      el = d.create_element("late-el")
      el["brand-new"] = "v"
      d.at_css("body") << el
      %w[late-el LATE-EL [brand-new] [BRAND-NEW] late-el[brand-new=v] body>late-el].each do |sel|
        expect(d.css(sel).map(&:name)).to eq(%w[late-el]), sel
        expect(d.at_css(sel)&.name).to eq("late-el"), sel
        expect(el.matches?(sel)).to be(true), sel
      end
      expect(d.css("late-el-2, [brand-old]")).to be_empty
    end
  end

  # An attribute selector's name is ASCII case-insensitive only on an HTML
  # element in an HTML document (the HTML Standard's rule for Selectors); on
  # SVG/MathML it is case-sensitive. Lexbor's own matcher folded case
  # everywhere, so `[viewbox]` found an SVG `viewBox` there.
  describe "attribute name case on foreign elements" do
    it "folds case on HTML elements only" do
      d = Makiri::HTML(%(<!doctype html><body><p Data-X="1"></p><svg viewBox="0 0 1 1" data-x="2"></svg></body>))
      expect(d.css("[DATA-X]").map(&:name)).to eq(%w[p])
      expect(d.css("[data-x]").map(&:name)).to eq(%w[p svg])
      expect(d.css("[viewBox]").map(&:name)).to eq(%w[svg])
      expect(d.css("[viewbox]")).to be_empty
    end
  end

  # SVG (foreign content) was under-covered relative to Makiri::XML's CSS
  # specs; these close that gap.
  describe "SVG (foreign content) type selectors" do
    let(:svg_doc) { Makiri::HTML("<html><body><svg><circle r='1'/></svg></body></html>") }

    it "matches an SVG element by its local (unprefixed) type name" do
      expect(svg_doc.css("circle").map(&:name)).to eq(%w[circle])
      expect(svg_doc.css("svg circle").map(&:name)).to eq(%w[circle])
    end

    # query_args_spec.rb pins the loose namespace ignore for a bare `svg|path`;
    # this is the same loose ignore inside compound/functional selectors.
    it "keeps the namespace binding ignored inside :is() / :has() / attribute selectors" do
      ns = { "svg" => "http://www.w3.org/2000/svg" }
      expect(svg_doc.css(":is(svg|circle)", ns).map(&:name)).to eq(%w[circle])
      expect(svg_doc.css("body:has(svg|circle)", ns).map(&:name)).to eq(%w[body])
      expect(svg_doc.css("[svg|r]", ns).map(&:name)).to eq(%w[circle])
    end
  end

  # HTML matching (`lexbor::css_match`) caps one compound chain at 64
  # compounds, the same bound `Makiri::XML` has (see `xml_css_spec.rb`'s
  # "fails closed on an over-long :is()/:not() argument"). Over the cap is a
  # raise, found before any node is matched, wherever the chain is nested -
  # never a silently dropped alternative (which made `:not(<65 compounds>)`
  # match every element). Selector NESTING stays unbounded: see the `:is()`
  # probe below.
  describe "resource limits" do
    def chain(n) = (["div"] * n).join(" > ")

    let(:deep) { Makiri::HTML("<body>#{'<div>' * 70}x#{'</div>' * 70}</body>") }
    let(:shapes) do
      [->(c) { c }, ->(c) { ":is(#{c})" }, ->(c) { "body :not(#{c})" },
       ->(c) { "body:has(#{c})" }, ->(c) { ":nth-child(1 of #{c})" }]
    end

    it "answers a 64-compound chain wherever it is nested" do
      expect(deep.css(chain(64)).length).to eq(7)
      expect(deep.css(":is(#{chain(64)})").length).to eq(7)
      expect(deep.css("body:has(#{chain(64)})").length).to eq(1)
    end

    it "raises the same error from css, at_css and matches? at 65 compounds, at every nesting" do
      target = deep.at_css("div")
      shapes.each do |shape|
        sel = shape.call(chain(65))
        expect { deep.css(sel) }.to raise_error(Makiri::Error, /too complex/), sel
        expect { deep.at_css(sel) }.to raise_error(Makiri::Error, /too complex/), sel
        expect { target.matches?(sel) }.to raise_error(Makiri::Error, /too complex/), sel
      end
    end

    it "does not let an earlier matching alternative hide an over-long one" do
      expect { deep.at_css("div, #{chain(65)}") }.to raise_error(Makiri::Error, /too complex/)
    end

    it "does not stack-overflow on deeply nested :is()" do
      # Kept small for suite speed; `lexbor::tests::css_match` runs the same
      # shape at 500,000 levels.
      d = Makiri::HTML("<html><body><a>x</a></body></html>")
      depth = 2000
      nested = (":is(" * depth) + "a" + (")" * depth)
      expect(d.css(nested).map(&:name)).to eq(%w[a])
    end
  end

  describe "errors" do
    it "raises CSS::SyntaxError on a malformed selector" do
      expect { doc.css(">>>bad") }.to raise_error(Makiri::CSS::SyntaxError)
      expect { doc.css("div[") }.to raise_error(Makiri::CSS::SyntaxError)
    end

    it "rejects a selector with invalid UTF-8 / an embedded NUL (verify_text)" do
      # The CSS boundary verifies the selector text before parsing - invalid
      # bytes never reach the engine.
      expect { doc.css("a\xFF".b) }.to raise_error(Makiri::Error)
      expect { doc.css("a\x00b") }.to raise_error(Makiri::Error)
      expect { doc.at_css("a\x00") }.to raise_error(Makiri::Error)
      expect { doc.at_css("p")&.matches?("p\x00") }.to raise_error(Makiri::Error)
    end

    # Lexbor parses the column combinator but its traversal cannot run it and
    # returns an error status. That status used to be dropped, so all three
    # answered "nothing matched" - a wrong answer, not an empty one.
    it "raises when the traversal cannot run the selector" do
      d = Makiri::HTML("<table><col><tr><td>x</td></tr></table>")
      expect { d.css("col || td") }.to raise_error(Makiri::Error, /could not be run/)
      expect { d.at_css("col || td") }.to raise_error(Makiri::Error, /could not be run/)
      expect { d.at_css("td").matches?("col || td") }
        .to raise_error(Makiri::Error, /could not be run/)
      expect(d.css("td").length).to eq(1)
    end

    # `:current()` is not in Selectors Level 4 (the time-dimensional
    # pseudo-classes were deferred to Level 5). Lexbor matches `:current(S)`
    # as `:is(S)`; answering that, or "nothing", would be a guess, so it is
    # refused like `:lexbor-contains()`. The argument-less form is already a
    # syntax error, from Lexbor's parser.
    it "refuses :current() as unsupported" do
      d = Makiri::HTML("<p><a>x</a></p>")
      expect { d.css(":current(a)") }.to raise_error(Makiri::Error, /could not be run/)
      expect { d.at_css("a:not(:current(a))") }.to raise_error(Makiri::Error, /could not be run/)
      expect { d.at_css("a").matches?(":current(a)") }.to raise_error(Makiri::Error, /could not be run/)
      expect { d.css(":current(a b)") }.to raise_error(Makiri::Error, /could not be run/)
      expect { d.css(":current") }.to raise_error(Makiri::CSS::SyntaxError)
      expect { Makiri::XML("<r><a/></r>").css(":current(a)") }.to raise_error(Makiri::CSS::SyntaxError)
    end

    # The unsupported construct is found before matching, so the answer does
    # not depend on document content, alternative order, an earlier simple
    # selector's mismatch, or at_css's first-match stop.
    it "raises for an unsupported construct wherever it sits in the selector" do
      d = Makiri::HTML("<table><col><tr><td>x</td></tr></table><p>hello</p>")
      p = d.at_css("p")
      [%(nosuch:lexbor-contains("x")), %(p:lexbor-contains("x")),
       %(p, nosuch:lexbor-contains("x")), %(nosuch:lexbor-contains("x"), p),
       %(:not(p:lexbor-contains("x"))), %(body:has(p:lexbor-contains("x"))),
       %(:nth-child(1 of p:lexbor-contains("x"))),
       "p, col || td", "col || td, p", "body:has(col || td)",
       ":current(p)", "p, :current(nosuch)", ":not(:current(p))"].each do |sel|
        expect { d.css(sel) }.to raise_error(Makiri::Error, /could not be run/), sel
        expect { d.at_css(sel) }.to raise_error(Makiri::Error, /could not be run/), sel
        expect { p.matches?(sel) }.to raise_error(Makiri::Error, /could not be run/), sel
      end
    end
  end

  # Which arguments reach the CSS parser is `lexbor::contains_guard`'s
  # decision, unchanged - `:lexbor-contains()` still PARSES on the HTML side
  # (or is rejected as a syntax error, same as before). MATCHING it is what
  # changed: `lexbor::css_match` deliberately does not implement it (a Lexbor
  # extension, not CSS), so a well-formed
  # `:lexbor-contains()` now raises `Makiri::Error` ("could not be run") on
  # HTML instead of ever answering - the same fail-closed treatment as an
  # unsupported combinator (`col || td`, below), and deliberately NOT a
  # silent empty result, which would be indistinguishable from "no element
  # matches". XML is unaffected (`Makiri::XML` still lowers it to XPath
  # `contains()` - see xml_css_spec.rb).
  describe ":lexbor-contains()" do
    it "keeps answering after a rejected one" do
      d = Makiri::HTML("<p>x")
      expect(d.css("p").length).to eq(1)
      expect { d.css(':lexbor-contains())') }.to raise_error(Makiri::CSS::SyntaxError)
      expect { d.css(":#{'a' * 17}") }.to raise_error(Makiri::CSS::SyntaxError)
      expect(d.css("p").length).to eq(1)
    end

    it "keeps answering when rejected and valid queries interleave" do
      d = Makiri::HTML("<div id='m'><p class='c'>one</p><p>two</p></div>")
      bad = [':lexbor-contains())', ':lexbor-contains()x', ':lexbor-contains( ))',
             'p:lexbor-contains()")', ':lexbor-contains(']
      50.times do |i|
        expect { d.css(bad[i % bad.length]) }.to raise_error(Makiri::CSS::SyntaxError)
        expect(d.css("p").length).to eq(2)
        expect(d.at_css("#m .c").text).to eq("one")
        # a fresh selector each round, so the cache both fills and is flushed
        expect(d.css("div > p:nth-of-type(#{(i % 2) + 1})").length).to eq(1)
      end
    end

    it "parses a well-formed one, but no longer matches with it" do
      d = Makiri::HTML("<p>hello</p><p>bye</p>")
      expect { d.css(':lexbor-contains())') }.to raise_error(Makiri::CSS::SyntaxError)
      expect { d.css('p:lexbor-contains("hello")') }.to raise_error(Makiri::Error, /could not be run/)
      expect { d.css('p:lexbor-contains("HELLO" i)') }.to raise_error(Makiri::Error, /could not be run/)
      expect { d.css("p:lexbor-contains(hello)") }.to raise_error(Makiri::Error, /could not be run/)
      # The document is unaffected - the very next query still answers.
      expect(d.css("p").length).to eq(2)
    end

    # The escapes matter: the parser decodes them, so none of the last three
    # contain the substring "lexbor-contains" at all.
    it "rejects every malformed form, escapes included" do
      d = Makiri::HTML("<p>hello</p>")
      x = Makiri::XML("<r><a>hello</a></r>")
      [':lexbor-contains()', ':lexbor-contains())', ':lexbor-contains(*)',
       ':lexbor-contains(#x)', ':lexbor-contains(123)', ':lexbor-contains(foo(bar))',
       %(:lexbor-contains("s" junk)), ':lexbor-contains(id junk)',
       %q(:lexbor\\-contains(#x)), %q(:\\6C exbor-contains(#x)),
       ':LEXBOR-CONTAINS(#x)'].each do |sel|
        expect { d.css(sel) }.to raise_error(Makiri::CSS::SyntaxError), sel
        expect { x.css(sel) }.to raise_error(Makiri::CSS::SyntaxError), sel
        expect(d.css("p").length).to eq(1), "document still answers after #{sel}"
      end
    end

    # Where a string ends is the tokenizer's call: CR, FF and LF end one, and
    # the selector after it is read on.
    it "rejects one after a string a newline ends" do
      d = Makiri::HTML("<p title='x'>hello</p>")
      ["\r", "\n", "\r\n", "\f"].each do |nl|
        sel = %([title="x#{nl}], p:lexbor-contains(#x))
        expect { d.css(sel) }.to raise_error(Makiri::CSS::SyntaxError), sel.dump
      end
      # A well-formed one still parses cleanly after those rejections - it
      # just no longer matches with it (see above).
      expect { d.css(%(p:lexbor-contains("hello"))) }.to raise_error(Makiri::Error, /could not be run/)
    end

    it "works on XML too" do
      x = Makiri::XML("<r><a>hello</a><b>bye</b></r>")
      expect(x.css(%(:lexbor-contains("hello"))).length).to eq(1)
      expect(x.css(%(:lexbor-contains("HELLO" i))).length).to eq(1)
    end

    it "handles a long needle (Lexbor >v3.0.0 heap-overflow fix in the parser)" do
      # Mirrors xml_css_spec.rb's equivalent test - HTML and XML reach the same
      # Lexbor CSS parser here (`lexbor::contains_guard`), so pin it on the HTML
      # side too rather than relying on the XML test alone. It PARSES this
      # (the fix is in the parser, unaffected by matching support), then
      # raises the same "could not be run" every well-formed :lexbor-contains()
      # does on HTML - not a crash, not a truncated/wrong match.
      needle = "A" * 200
      big = Makiri::HTML("<p>#{needle}</p><p>x</p>")
      expect { big.css(%(p:lexbor-contains("#{needle}"))) }
        .to raise_error(Makiri::Error, /could not be run/)
    end
  end

  describe "memory safety", :gc_compact do
    it "stays correct under GC stress and compaction" do
      GC.stress = true
      begin
        nodes = doc.css("p").to_a
        expect(nodes.map(&:text)).to eq(%w[one two])
        GC.compact
        expect(doc.at_css("#main").css("a").length).to eq(2)
      ensure
        GC.stress = false
      end
    end

    it "survives many queries across dropped documents" do
      gc_churn_iters(300).times do |i|
        d = Makiri::HTML("<html><body><p class='c#{i}'>#{i}</p></body></html>")
        expect(d.at_css(".c#{i}").text).to eq(i.to_s)
      end
      GC.start
    end
  end
end
