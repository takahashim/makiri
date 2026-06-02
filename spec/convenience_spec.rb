# frozen_string_literal: true

# Convenience navigation/query helpers: Node#root / #ancestors / #path and the
# NodeSet set operations + accessors.
RSpec.describe "Makiri convenience API" do
  let(:doc) do
    Makiri::HTML(<<~HTML)
      <html><body>
        <div id="m">
          <p>a</p>
          <p>b</p>
          <span>s</span>
        </div>
      </body></html>
    HTML
  end

  describe "Node#root" do
    it "returns the document root element from any node" do
      expect(doc.at_css("span").root.name).to eq("html")
      expect(doc.root.name).to eq("html")
    end
  end

  describe "Node#ancestors" do
    it "lists ancestor elements nearest-first" do
      p = doc.css("p").last
      expect(p.ancestors.map(&:name)).to eq(%w[div body html])
    end

    it "is empty for the root element" do
      expect(doc.root.ancestors.to_a).to eq([])
    end
  end

  describe "Node#path" do
    it "indexes among same-name siblings and round-trips through at_xpath" do
      second_p = doc.css("p").last
      expect(second_p.path).to eq("/html/body/div/p[2]")
      expect(doc.at_xpath(second_p.path)).to eq(second_p)
    end

    it "omits the index for a unique element" do
      span = doc.at_css("span")
      expect(span.path).to eq("/html/body/div/span")
      expect(doc.at_xpath(span.path)).to eq(span)
    end

    it "uses @name for attributes" do
      attr = doc.at_css("#m").attribute_nodes.first
      expect(attr.path).to eq("/html/body/div/@id")
    end

    it "round-trips a text node" do
      text = doc.at_css("p").child
      expect(doc.at_xpath(text.path)).to eq(text)
    end
  end

  describe "NodeSet operations" do
    let(:ps)    { doc.css("p") }
    let(:spans) { doc.css("span") }
    let(:all)   { doc.css("p, span") }

    it "unions with | (deduped)" do
      expect((ps | spans).map(&:name)).to eq(%w[p p span])
      expect((ps | ps).length).to eq(2)
    end

    it "concatenates with + (duplicates kept)" do
      expect((ps + ps).length).to eq(4)
    end

    it "intersects with &" do
      expect((all & ps).map(&:name)).to eq(%w[p p])
    end

    it "differences with -" do
      expect((all - ps).map(&:name)).to eq(%w[span])
    end

    it "rejects a non-NodeSet operand" do
      expect { ps | [1, 2] }.to raise_error(TypeError)
    end

    it "stays correct on large operands (pointer-hash path, not O(n^2))" do
      # Above the hash threshold the operators switch from a linear scan to a
      # pointer hash; exercise both the result and that path here.
      big = Makiri::HTML("<ul>#{(1..2000).map { |i| "<li>#{i}</li>" }.join}</ul>")
      lis = big.css("li")
      expect(lis.length).to eq(2000)
      expect((lis | lis).length).to eq(2000)          # union dedup
      expect((lis & lis).length).to eq(2000)          # intersection
      expect((lis - lis).length).to eq(0)             # difference
      half = big.css("li:nth-child(-n+1000)")         # half-overlap
      expect((lis & half).length).to eq(half.length)
      expect((lis - half).length).to eq(2000 - half.length)
    end
  end

  describe "NodeSet accessors" do
    it "exposes first / last / at" do
      ps = doc.css("p")
      expect(ps.first.text).to eq("a")
      expect(ps.last.text).to eq("b")
      expect(ps.at(1).text).to eq("b")
      expect(ps.at(-1).text).to eq("b")
    end
  end
end
