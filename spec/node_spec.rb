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

  describe "namespace (WHATWG DOM Element/Attr accessors)" do
    let(:svg_doc) do
      Makiri::HTML(<<~HTML)
        <html><body><svg xmlns="http://www.w3.org/2000/svg">
          <a xlink:href="u" xml:lang="en" class="c"><path d="z"/></a>
        </svg></body></html>
      HTML
    end
    let(:path) { svg_doc.at_xpath('//*[local-name()="path"]') }
    let(:a)    { svg_doc.at_xpath('//*[local-name()="a"]') }
    def attr(node, name) = node.attribute_nodes.find { |x| x.name == name }

    describe "#local_name" do
      it "is the un-prefixed name for elements" do
        expect(div.local_name).to eq("div")
        expect(path.local_name).to eq("path")
      end

      it "drops the prefix for prefixed attributes" do
        expect(attr(a, "xlink:href").local_name).to eq("href")
        expect(attr(a, "class").local_name).to eq("class")
      end

      it "is nil for non-element/attribute nodes (DOM)" do
        expect(ps[0].child.local_name).to be_nil
        expect(doc.local_name).to be_nil
      end
    end

    describe "#namespace_uri" do
      it "is the XHTML namespace for HTML elements (DOM-faithful, not nil)" do
        # An HTML element is in the HTML namespace per the DOM, like browsers
        # and namespace-uri(); only Nokogiri::HTML's libxml2 returns nil.
        expect(div.namespace_uri).to eq("http://www.w3.org/1999/xhtml")
        expect(body.namespace_uri).to eq("http://www.w3.org/1999/xhtml")
      end

      it "is the SVG namespace for SVG elements" do
        expect(path.namespace_uri).to eq("http://www.w3.org/2000/svg")
      end

      it "agrees with the namespace-uri() XPath function" do
        expect(div.namespace_uri).to eq(div.at_xpath("namespace-uri(.)"))
        expect(path.namespace_uri).to eq(path.at_xpath("namespace-uri(.)"))
      end

      it "is nil for an unprefixed attribute" do
        expect(attr(a, "class").namespace_uri).to be_nil
      end

      it "is the parser-assigned namespace for a prefixed attribute" do
        expect(attr(a, "xlink:href").namespace_uri).to eq("http://www.w3.org/1999/xlink")
        expect(attr(a, "xml:lang").namespace_uri).to eq("http://www.w3.org/XML/1998/namespace")
      end

      it "is nil for non-element/attribute nodes (DOM)" do
        expect(ps[0].child.namespace_uri).to be_nil
        expect(doc.namespace_uri).to be_nil
      end
    end

    describe "#prefix" do
      it "is nil for unprefixed HTML5-parsed nodes" do
        expect(div.prefix).to be_nil
        expect(path.prefix).to be_nil
        expect(attr(a, "class").prefix).to be_nil
      end

      it "is the prefix segment for prefixed attributes" do
        expect(attr(a, "xlink:href").prefix).to eq("xlink")
        expect(attr(a, "xml:lang").prefix).to eq("xml")
      end

      it "is nil for non-element/attribute nodes (DOM)" do
        expect(ps[0].child.prefix).to be_nil
        expect(doc.prefix).to be_nil
      end
    end
  end

  describe "#tag_name (DOM tagName)" do
    it "is the uppercase qualified name for HTML elements" do
      expect(div.tag_name).to eq("DIV")
      expect(body.tag_name).to eq("BODY")
    end

    it "differs from #name, which stays lowercase" do
      expect(div.name).to eq("div")
    end

    it "keeps the original case for SVG/MathML elements" do
      path = Makiri::HTML(%(<svg xmlns="http://www.w3.org/2000/svg"><path/></svg>))
             .at_xpath('//*[local-name()="path"]')
      expect(path.tag_name).to eq("path")
    end

    it "is nil for non-element nodes" do
      expect(ps[0].child.tag_name).to be_nil
      expect(doc.tag_name).to be_nil
    end
  end

  describe "#target (DOM ProcessingInstruction.target)" do
    it "is the target of a created processing instruction" do
      pi = doc.create_processing_instruction("xml-stylesheet", "x")
      expect(pi.target).to eq("xml-stylesheet")
    end

    it "is nil for non-PI nodes" do
      expect(div.target).to be_nil
      expect(ps[0].child.target).to be_nil
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

  describe "#dup / #clone (Ruby copy protocol)" do
    it "#dup is a deep, detached, independent copy by default" do
      copy = div.dup
      expect(copy.name).to eq("div")
      expect(copy.parent).to be_nil
      expect(copy).not_to eql(div)
      expect(copy.element_children.map(&:name)).to eq(%w[p p])
      copy["id"] = "y"
      expect(div["id"]).to eq("x") # independent
    end

    it "#dup(0) is a shallow copy (Nokogiri level argument)" do
      expect(div.dup(0).children).to be_empty
    end

    it "#clone is a deep copy and honours the freeze: keyword" do
      expect(div.clone.element_children.map(&:name)).to eq(%w[p p])
      expect(div.clone).not_to be_frozen
      expect(div.clone(freeze: false)).not_to be_frozen
      frozen = div.clone(freeze: true)
      expect(frozen).to be_frozen
      expect { frozen["id"] = "z" }.to raise_error(FrozenError)
    end

    it "Document#dup is an independent whole-document copy" do
      d2 = doc.dup
      expect(d2).to be_a(Makiri::Document)
      expect(d2.at_css("#x")).not_to be_nil
      expect(d2.at_css("#x")).not_to eql(div)
      d2.at_css("#x")["id"] = "z"
      expect(doc.at_css("#x")).not_to be_nil # original untouched
    end
  end

  describe "a frozen node is immutable" do
    it "raises FrozenError from every mutator" do
      node = div.dup       # detached copy we can freeze freely
      child = doc.create_element("span")
      node.freeze
      expect(node).to be_frozen
      expect { node["k"] = "v" }.to raise_error(FrozenError)
      expect { node.delete("id") }.to raise_error(FrozenError)
      expect { node.content = "x" }.to raise_error(FrozenError)
      expect { node.name = "section" }.to raise_error(FrozenError)
      expect { node.add_child(child) }.to raise_error(FrozenError)
      expect { node << child }.to raise_error(FrozenError)
      expect { node.inner_html = "<b>x</b>" }.to raise_error(FrozenError)
    end

    it "still allows mutation of non-frozen nodes" do
      div["new"] = "1"
      expect(div["new"]).to eq("1")
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
