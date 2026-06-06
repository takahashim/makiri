# frozen_string_literal: true

# Nokogiri-compatibility convenience API: #attributes, #search/#at,
# #content=, Document#body/#head, NodeSet#css/#xpath/#search/#remove, and the
# Element.new / Text.new constructors.
RSpec.describe "Makiri Nokogiri-compat API" do
  let(:doc) do
    Makiri::HTML(<<~HTML)
      <html><body>
        <div id="m" class="c x" data-n="1">
          <p>one</p>
          <p>two</p>
          <a href="/l">L</a>
        </div>
      </body></html>
    HTML
  end
  let(:div) { doc.at_css("#m") }

  describe "Node#attributes" do
    it "returns a name => Attribute hash" do
      attrs = div.attributes
      expect(attrs.keys).to eq(%w[id class data-n])
      expect(attrs.values).to all(be_a(Makiri::Attr))
      expect(attrs.transform_values(&:value)).to eq("id" => "m", "class" => "c x", "data-n" => "1")
    end

    it "is empty for non-elements" do
      expect(div.at_css("p").child.attributes).to eq({})
    end
  end

  describe "Node#search / #at" do
    it "treats location-path strings as XPath" do
      expect(doc.search("//p").map(&:text)).to eq(%w[one two])
      expect(doc.search("//a/@href").map(&:value)).to eq(["/l"])
      expect(div.search(".//p").length).to eq(2)
    end

    it "treats other strings as CSS" do
      expect(doc.search("p").map(&:text)).to eq(%w[one two])
      expect(doc.search("div.c a").map(&:text)).to eq(%w[L])
    end

    it "#at returns the first match for either syntax" do
      expect(doc.at("p").text).to eq("one")
      expect(doc.at("//a").text).to eq("L")
    end
  end

  describe "Node#content=" do
    it "replaces an element's children with escaped text" do
      div.content = "new & <text>"
      expect(div.text).to eq("new & <text>")
      expect(div.inner_html).to eq("new &amp; &lt;text&gt;")
      expect(div.element_children.length).to eq(0)
    end

    it "is reflected by queries afterwards" do
      div.content = "x"
      expect(doc.css("p").length).to eq(0)
    end
  end

  describe "Document#body / #head" do
    it "returns the structural elements" do
      expect(doc.body.name).to eq("body")
      expect(doc.head.name).to eq("head")
    end
  end

  describe "NodeSet#css / #xpath / #search" do
    let(:nested) do
      Makiri::HTML("<html><body><div><a>1</a></div><div><a>2</a><b>x</b></div></body></html>")
    end

    it "applies the query to each node and unions the results" do
      divs = nested.css("div")
      expect(divs.css("a").map(&:text)).to eq(%w[1 2])
      expect(divs.xpath(".//a").map(&:text)).to eq(%w[1 2])
      expect(divs.search("a").map(&:text)).to eq(%w[1 2])
    end

    it "returns an empty set unchanged" do
      empty = nested.css(".none")
      expect(empty.css("a")).to be_empty
    end
  end

  describe "NodeSet#remove" do
    it "detaches every node in the set" do
      nested = Makiri::HTML("<html><body><a>1</a><a>2</a><b>keep</b></body></html>")
      nested.css("a").remove
      expect(nested.css("a").length).to eq(0)
      expect(nested.css("b").length).to eq(1)
    end
  end

  describe "node-class constructors (shared base, delegating to the document)" do
    it "constructs detached nodes bound to the document" do
      el = Makiri::Element.new("section", doc)
      el << Makiri::Text.new("hi", doc)
      expect(el).to be_a(Makiri::Element)
      expect(el.name).to eq("section")
      doc.body.add_child(el)
      expect(doc.at_css("section").text).to eq("hi")
    end

    it "Comment.new / ProcessingInstruction.new work for HTML too (document first)" do
      expect(Makiri::Comment.new(doc, " c ")).to be_a(Makiri::Comment)
      expect(Makiri::ProcessingInstruction.new(doc, "php", "echo")).to be_a(Makiri::ProcessingInstruction)
    end

    it "raise a consistent TypeError on a non-document, for HTML and XML alike" do
      expect { Makiri::HTML::Element.new("e", "not a doc") }.to raise_error(TypeError)
      expect { Makiri::XML::Element.new("e", "not a doc") }.to raise_error(TypeError)
      expect { Makiri::HTML::Comment.new("not a doc", "c") }.to raise_error(TypeError)
    end
  end
end
