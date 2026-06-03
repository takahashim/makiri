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

    it "#pointer_id is the shared node pointer (Nokogiri-compatible)" do
      a = body.first_element_child
      b = body.first_element_child
      expect(a.pointer_id).to be_a(Integer)
      expect(a.pointer_id).to eq(b.pointer_id)
      expect(a.pointer_id).not_to eq(ps[0].pointer_id)
      # Same value the hash/eql? contract is built on.
      expect(a.pointer_id).to eq(a.hash)
    end
  end

  describe "#clone_node" do
    it "shallow-clones by default (DOM cloneNode's deep defaults to false)" do
      clone = div.clone_node
      expect(clone.name).to eq("div")
      expect(clone["id"]).to eq("x")
      expect(clone["class"]).to eq("a b")
      expect(clone.children).to be_empty
    end

    it "treats a missing, nil, or false argument as shallow" do
      expect(div.clone_node.children).to be_empty
      expect(div.clone_node(nil).children).to be_empty
      expect(div.clone_node(false).children).to be_empty
    end

    it "deep-clones the subtree with a truthy deep argument" do
      clone = div.clone_node(true)
      expect(clone.element_children.map(&:name)).to eq(%w[p p])
      expect(clone.text).to eq(div.text)
    end

    it "returns a detached node distinct from the original" do
      clone = div.clone_node(true)
      expect(clone).not_to eql(div)
      expect(clone.parent).to be_nil
    end

    it "is owned by the same document and editable independently" do
      clone = div.clone_node
      clone["id"] = "y"
      expect(div["id"]).to eq("x")
      expect(clone.document).to equal(doc)
    end

    it "carries <template> contents on a deep clone (import_node alone omits them)" do
      tdoc = Makiri::HTML('<template id="t"><p>hi</p><span>x</span></template>')
      clone = tdoc.at_css("#t").clone_node(true)
      expect(clone.content_fragment.children.map(&:name)).to eq(%w[p span])
    end
  end

  describe "Document#import_node" do
    let(:other) { Makiri::HTML("<html><body></body></html>") }

    it "shallow-imports by default (DOM importNode's deep defaults to false)" do
      imp = other.import_node(div)
      expect(imp.name).to eq("div")
      expect(imp["id"]).to eq("x")
      expect(imp.children).to be_empty
    end

    it "deep-imports the subtree with a truthy deep argument" do
      imp = other.import_node(div, true)
      expect(imp.element_children.map(&:name)).to eq(%w[p p])
    end

    it "owns the copy in the receiver document and leaves the source untouched" do
      imp = other.import_node(div, true)
      expect(imp.document).to equal(other)
      expect(imp.parent).to be_nil
      expect(div.document).to equal(doc)
      expect(div.element_children.map(&:name)).to eq(%w[p p])
      other.at_css("body").add_child(imp)
      expect(other.at_css("#x")).not_to be_nil
    end
  end

  describe "#content_fragment (template contents)" do
    let(:tdoc) do
      Makiri::HTML('<body><template id="t"><p>hi</p><span>x</span></template><div>plain</div></body>')
    end
    let(:tpl) { tdoc.at_css("#t") }

    it "exposes a <template>'s contents as a DocumentFragment" do
      cf = tpl.content_fragment
      expect(cf).to be_a(Makiri::DocumentFragment)
      expect(cf.children.map(&:name)).to eq(%w[p span])
    end

    it "is queryable (the contents are NOT children of the template itself)" do
      expect(tpl.children).to be_empty            # matches the DOM: empty
      expect(tpl.content_fragment.css("p").map(&:text)).to eq(["hi"])
      expect(tpl.content_fragment.xpath(".//span").map(&:text)).to eq(["x"])
    end

    it "is an empty fragment for an empty template" do
      cf = Makiri::HTML("<template></template>").at_css("template").content_fragment
      expect(cf).to be_a(Makiri::DocumentFragment)
      expect(cf.children).to be_empty
    end

    it "is nil for a non-template element" do
      expect(tdoc.at_css("div").content_fragment).to be_nil
    end
  end
end
