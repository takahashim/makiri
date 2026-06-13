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
