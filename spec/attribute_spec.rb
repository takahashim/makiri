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

  describe "Element#attribute_by_qualified_name" do
    it "returns the attribute node with that exact qualified name" do
      attr = div.attribute_by_qualified_name("class")
      expect(attr).to be_a(Makiri::Attr)
      expect(attr.value).to eq("a b")
    end

    it "does not answer a prefixed attribute for its local name, unlike #[]" do
      div["xml:b"] = "vv"
      expect(div.attribute_by_qualified_name("xml:b").value).to eq("vv")
      expect(div.attribute_by_qualified_name("b")).to be_nil
    end

    it "is nil for an absent name and for non-element nodes" do
      expect(div.attribute_by_qualified_name("nope")).to be_nil
      expect(span.child.attribute_by_qualified_name("id")).to be_nil
    end

    it "accepts a Symbol and ignores a non-string name, as #[] does" do
      expect(div.attribute_by_qualified_name(:id).value).to eq("outer")
      expect(div.attribute_by_qualified_name(nil)).to be_nil
      expect(div.attribute_by_qualified_name(123)).to be_nil
    end

    it "rejects a NUL byte in the name" do
      expect { div.attribute_by_qualified_name("id\0x") }.to raise_error(Makiri::Error)
    end

    it "finds XML attributes by their qualified name too" do
      xdoc = Makiri::XML(%(<r xmlns:x="u"><e id="1" x:b="vv"/></r>))
      e = xdoc.at_css("e")
      expect(e.attribute_by_qualified_name("x:b").value).to eq("vv")
      expect(e.attribute_by_qualified_name("b")).to be_nil
      expect(e.attribute_by_qualified_name("id")).to be_a(Makiri::Attr)
    end
  end

  describe "Element#attribute_value_by_qualified_name" do
    it "returns the value for the same match, and nil when there is none" do
      div["xml:b"] = "vv"
      expect(div.attribute_value_by_qualified_name("class")).to eq("a b")
      expect(div.attribute_value_by_qualified_name("xml:b")).to eq("vv")
      expect(div.attribute_value_by_qualified_name("b")).to be_nil
      expect(div.attribute_value_by_qualified_name("nope")).to be_nil
    end

    it "distinguishes an empty value from an absent attribute" do
      div["empty"] = ""
      expect(div.attribute_value_by_qualified_name("empty")).to eq("")
      expect(div.attribute_value_by_qualified_name("absent")).to be_nil
    end

    it "is nil for non-element nodes" do
      expect(span.child.attribute_value_by_qualified_name("id")).to be_nil
    end

    it "reads XML attributes too" do
      e = Makiri::XML(%(<r xmlns:x="u"><e id="1" x:b="vv"/></r>)).at_css("e")
      expect(e.attribute_value_by_qualified_name("x:b")).to eq("vv")
      expect(e.attribute_value_by_qualified_name("b")).to be_nil
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

    it "keeps a coerced non-String argument alive while its bytes are borrowed" do
      # A Symbol goes through rb_String, which makes a fresh String only the
      # borrowed view holds; the view's guard must keep it reachable.
      GC.stress = true
      begin
        div[:"data-sym"] = :value
        expect(div[:"data-sym"]).to eq("value")
        expect(div.key?(:"data-sym")).to be(true)
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
