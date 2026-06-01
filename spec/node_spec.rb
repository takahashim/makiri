# frozen_string_literal: true

RSpec.describe Makiri::Node do
  let(:doc) do
    Makiri::HTML(<<~HTML)
      <html><body>
        <div id="x" class="a b">
          <p>one</p>
          <!-- c -->
          <p>two</p>
        </div>
      </body></html>
    HTML
  end
  let(:body) { doc.root.element_children.find { |e| e.name == "body" } }
  let(:div)  { body.first_element_child }
  let(:ps)   { div.element_children }

  describe "#name" do
    it "is the lowercase tag name for elements" do
      expect(div.name).to eq("div")
    end

    it "uses the un-prefixed DOM names for other node kinds" do
      text = ps[0].child
      expect(text).to be_a(Makiri::Text)
      expect(text.name).to eq("text")
    end
  end

  describe "type predicates" do
    it "classifies nodes" do
      expect(div).to be_element
      expect(ps[0].child).to be_text
      expect(doc).to be_document
      expect(div.element?).to be(true)
      expect(div.text?).to be(false)
    end

    it "exposes node_type" do
      expect(div.node_type).to eq(1)        # ELEMENT
      expect(ps[0].child.node_type).to eq(3) # TEXT
      expect(doc.node_type).to eq(9)         # DOCUMENT
    end
  end

  describe "text content" do
    it "concatenates descendant text" do
      expect(ps[0].text).to eq("one")
      expect(div.text).to match(/one/)
      expect(div.text).to match(/two/)
    end

    it "aliases content / inner_text" do
      expect(ps[0].content).to eq("one")
      expect(ps[0].inner_text).to eq("one")
    end

    it "returns the whole document's text from the Document node" do
      # DOM makes Document#textContent null; we return the root element's text.
      expect(doc.text).to match(/one/)
      expect(doc.text).to match(/two/)
      expect(doc.text).to eq(doc.root.text)
    end

    it "concatenates only descendant text, excluding comments" do
      d = Makiri::HTML("<html><body><div>a<!--skip--><span>b</span>c</div></body></html>")
      expect(d.at_css("div").text).to eq("abc")
    end

    it "gathers text from a deeply nested subtree (iterative, no stack blowup)" do
      open = "<div>x".dup * 500
      d = Makiri::HTML("<html><body>#{open}#{"</div>" * 500}</body></html>")
      expect(d.at_css("body").text).to eq("x" * 500)
    end
  end

  describe "attributes (read-only)" do
    it "reads by name" do
      expect(div["id"]).to eq("x")
      expect(div["class"]).to eq("a b")
      expect(div["missing"]).to be_nil
    end

    it "coerces symbol keys" do
      expect(div[:id]).to eq("x")
    end

    it "reports key?" do
      expect(div.key?("id")).to be(true)
      expect(div.key?("nope")).to be(false)
    end

    it "lists keys and values in document order" do
      expect(div.keys).to eq(%w[id class])
      expect(div.values).to eq(["x", "a b"])
    end

    it "returns nil/empty for non-elements" do
      text = ps[0].child
      expect(text["id"]).to be_nil
      expect(text.keys).to eq([])
    end
  end

  describe "navigation" do
    it "walks parent/children" do
      expect(div.parent).to eq(body)
      expect(div.children).to be_a(Makiri::NodeSet)
      expect(ps[0].parent).to eq(div)
    end

    it "walks element-only children" do
      expect(ps.length).to eq(2)
      expect(ps.map(&:text)).to eq(%w[one two])
    end

    it "walks sibling elements, skipping comments/whitespace" do
      expect(ps[0].next_element).to eq(ps[1])
      expect(ps[1].previous_element).to eq(ps[0])
    end

    it "exposes first/last element child" do
      expect(div.first_element_child).to eq(ps[0])
      expect(div.last_element_child).to eq(ps[1])
    end

    it "returns the owning document" do
      expect(div.document).to equal(doc)
    end
  end

  describe "identity" do
    it "compares by underlying node pointer" do
      expect(div).to eq(body.first_element_child)
      expect(div).not_to eq(ps[0])
    end

    it "satisfies the hash/eql? contract" do
      a = body.first_element_child
      b = body.first_element_child
      expect(a).to eql(b)
      expect(a.hash).to eq(b.hash)
      expect({ a => :v }[b]).to eq(:v)
      expect([a, b, ps[0]].uniq.size).to eq(2)
    end

    it "does not equal a non-node" do
      expect(div == "div").to be(false)
    end
  end
end
