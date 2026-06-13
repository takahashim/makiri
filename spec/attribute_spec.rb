# frozen_string_literal: true

# M4: attribute nodes and the attr->owner index. The interesting invariant is
# that an Attribute wraps only the bare Lexbor attr pointer (Lexbor never links
# it back to its element), so #parent must resolve through the lazily-built
# compat index rather than a stored owner.
RSpec.describe Makiri::Attr do
  let(:doc) do
    Makiri::HTML(<<~HTML)
      <html><body>
        <div id="outer" class="a b" data-n="1">
          <span id="inner" class="a b">hi</span>
        </div>
      </body></html>
    HTML
  end

  def find(node, name)
    return node if node.respond_to?(:name) && node.name == name
    return nil unless node.respond_to?(:children)

    node.children.each do |c|
      hit = find(c, name)
      return hit if hit
    end
    nil
  end

  let(:div)  { find(doc.root, "div") }
  let(:span) { find(doc.root, "span") }

  describe "Element#attribute_nodes" do
    it "returns Attribute nodes in document order" do
      attrs = div.attribute_nodes
      expect(attrs).to be_a(Makiri::NodeSet)
      expect(attrs).to all(be_a(Makiri::Attr)) # HTML::Attr leaf, is_a? Attr
      expect(attrs.map(&:name)).to eq(%w[id class data-n])
      expect(attrs.map(&:value)).to eq(["outer", "a b", "1"])
    end

    it "is empty for non-element nodes" do
      text = span.child
      expect(text).to be_a(Makiri::Text)
      expect(text.attribute_nodes.to_a).to eq([])
    end
  end

  describe "#name / #value" do
    it "exposes the attribute name and value" do
      id = div.attribute_nodes.first
      expect(id.name).to eq("id")
      expect(id.value).to eq("outer")
    end

    it "classifies as an attribute" do
      expect(div.attribute_nodes.first).to be_attribute
    end
  end

  describe "#parent / #element via the attr->owner index" do
    it "resolves an attribute back to its owning element" do
      id = div.attribute_nodes.first
      expect(id.parent).to eq(div)
      expect(id.element).to eq(div)
    end

    it "distinguishes owners of identically-named attributes" do
      div_class  = div.attribute_nodes.find  { |a| a.name == "class" }
      span_class = span.attribute_nodes.find { |a| a.name == "class" }

      # Same name, same value, different owners.
      expect(div_class.value).to eq(span_class.value)
      expect(div_class.parent).to eq(div)
      expect(span_class.parent).to eq(span)
      expect(div_class.parent).not_to eq(span_class.parent)
    end

    it "is stable across separately-wrapped attribute nodes" do
      a = div.attribute_nodes.first
      b = div.attribute_nodes.first
      expect(a).to eq(b)
      expect(a.parent).to eq(b.parent)
    end

    it "round-trips element -> attribute -> element" do
      div.attribute_nodes.each do |attr|
        expect(attr.parent).to eq(div)
      end
    end
  end

  describe "lazy build + memory safety", :gc_compact do
    it "builds the index only when first needed and stays correct under GC" do
      attrs = div.attribute_nodes.to_a # index not built yet
      GC.stress = true
      begin
        expect(attrs.map { |a| a.parent.name }).to all(eq("div"))
        GC.start
        expect(attrs.first.parent).to eq(div)
      ensure
        GC.stress = false
      end
    end

    it "survives dropping many documents that built the index" do
      gc_churn_iters(1000).times do
        d = Makiri::HTML('<p id="z" data-x="y">t</p>')
        p_node = find(d.root, "p")
        p_node.attribute_nodes.each { |a| a.parent } # forces the index build
        expect(p_node.attribute_nodes.first.parent).to eq(p_node)
      end
      GC.start
      expect(true).to be(true)
    end
  end
end
