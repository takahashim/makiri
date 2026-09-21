# frozen_string_literal: true

# M7: CSS selector queries, delegated to Lexbor's lxb_selectors engine.
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
  end

  # Which arguments reach the CSS parser is `lexbor::contains_guard`'s decision.
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

    it "still serves a well-formed one" do
      d = Makiri::HTML("<p>hello</p><p>bye</p>")
      expect { d.css(':lexbor-contains())') }.to raise_error(Makiri::CSS::SyntaxError)
      expect(d.css('p:lexbor-contains("hello")').length).to eq(1)
      expect(d.css('p:lexbor-contains("HELLO" i)').length).to eq(1)
      expect(d.css("p:lexbor-contains(hello)").length).to eq(1)
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

    it "works on XML too" do
      x = Makiri::XML("<r><a>hello</a><b>bye</b></r>")
      expect(x.css(%(:lexbor-contains("hello"))).length).to eq(1)
      expect(x.css(%(:lexbor-contains("HELLO" i))).length).to eq(1)
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
