# frozen_string_literal: true

# What a browser-DOM layer over Makiri needs of it and the Nokogiri-shaped API
# did not give: the DOM's factories and setters with the DOM's naming and
# namespace rules, and selectors matched as browsers match them. The requests
# came from dommy running WPT on 0.11.0.rc1.
RSpec.describe "browser-DOM interop" do
  SVG_NS = "http://www.w3.org/2000/svg" unless defined?(SVG_NS)
  XHTML_NS = "http://www.w3.org/1999/xhtml" unless defined?(XHTML_NS)
  XML_NS = "http://www.w3.org/XML/1998/namespace" unless defined?(XML_NS)
  XMLNS_NS = "http://www.w3.org/2000/xmlns/" unless defined?(XMLNS_NS)

  describe "HTML Document#create_element_ns" do
    let(:doc) { Makiri::HTML("<body></body>") }

    it "makes an SVG element that behaves like a parsed one" do
      made = doc.create_element_ns(SVG_NS, "feGaussianBlur")
      made["viewBox"] = "0 0 1 1"
      doc.body << made
      parsed = Makiri::HTML("<svg><feGaussianBlur viewBox='0 0 1 1'/></svg>").at_css("feGaussianBlur")

      [made, parsed].each do |el|
        expect([el.name, el.namespace_uri]).to eq(["feGaussianBlur", SVG_NS])
        expect(el["viewBox"]).to eq("0 0 1 1")
        expect(el["viewbox"]).to be_nil
      end
      expect(doc.at_css("[viewBox]")).to eq(made)
      expect(doc.at_css("feGaussianBlur")).to eq(made)
      expect(doc.body.inner_html).to eq(%(<feGaussianBlur viewBox="0 0 1 1"></feGaussianBlur>))
    end

    it "gives []= setAttribute's meaning on the element it made" do
      el = doc.create_element_ns(SVG_NS, "svg")
      %w[viewBox xmlns xlink:href v-on:click : a:].each { |n| el[n] = "v" }
      expect(el.attribute_nodes.map { [_1.name, _1.namespace_uri] })
        .to eq([["viewBox", nil], ["xmlns", nil], ["xlink:href", nil], ["v-on:click", nil], [":", nil], ["a:", nil]])
    end

    it "splits the name at its first colon and holds each half to the DOM's rule" do
      el = doc.create_element_ns("http://example.com/", "0:a")
      expect([el.name, el.namespace_uri]).to eq(["0:a", "http://example.com/"])
      el = doc.create_element_ns("http://example.com/", "f:o:o")
      expect(el.name).to eq("f:o:o")
      expect { doc.create_element_ns("http://example.com/", "a b") }.to raise_error(ArgumentError)
      expect { doc.create_element_ns("http://example.com/", ":a") }.to raise_error(ArgumentError)
      expect { doc.create_element_ns("http://example.com/", "a:") }.to raise_error(ArgumentError)
      expect { doc.create_element_ns(nil, "") }.to raise_error(ArgumentError)
    end

    it "holds the namespace to the DOM's rule, not Namespaces in XML's" do
      expect { doc.create_element_ns(nil, "p:x") }.to raise_error(Makiri::Error, /does not fit/)
      expect { doc.create_element_ns("urn:x", "xml:x") }.to raise_error(Makiri::Error, /does not fit/)
      expect { doc.create_element_ns("urn:x", "xmlns") }.to raise_error(Makiri::Error, /does not fit/)
      expect { doc.create_element_ns(XMLNS_NS, "x") }.to raise_error(Makiri::Error, /does not fit/)
      expect(doc.create_element_ns(XMLNS_NS, "xmlns").namespace_uri).to eq(XMLNS_NS)
      expect(doc.create_element_ns(XML_NS, "xml:x").namespace_uri).to eq(XML_NS)
      # the DOM stops short of "the XML namespace only under xml"
      expect(doc.create_element_ns(XML_NS, "x").namespace_uri).to eq(XML_NS)
    end

    it "reads nil and an empty namespace alike as no namespace" do
      [nil, ""].each do |ns|
        el = doc.create_element_ns(ns, "x")
        expect([el.name, el.namespace_uri]).to eq(["x", nil])
      end
    end

    it "makes an XHTML element the HTML element it names" do
      tpl = doc.create_element_ns(XHTML_NS, "template")
      tpl.inner_html = "<b>x</b>"
      expect(tpl.children.size).to eq(0)
      expect(tpl.content_fragment.children.size).to eq(1)
      div = doc.create_element_ns(XHTML_NS, "div")
      div["ID"] = "a"
      expect(div.attribute_nodes.map(&:name)).to eq(["id"])
    end

    # Lexbor makes an element from its lower-cased name, so `BR` would be the
    # void `br` (a child appended to it vanished from to_html), `SCRIPT` a
    # raw-text element. The DOM makes an unknown element; Makiri refuses.
    it "refuses an upper-case HTML name that lower-cases to a known element" do
      %w[BR SCRIPT TEMPLATE DIV h:BR].each do |name|
        expect { doc.create_element_ns(XHTML_NS, name) }.to raise_error(Makiri::Error, /upper-case name/)
      end
      el = doc.create_element_ns(XHTML_NS, "MY-EL")
      el << doc.create_text_node("kept")
      expect(el.to_html).to eq("<MY-EL>kept</MY-EL>")
      expect(doc.create_element_ns(SVG_NS, "BR").name).to eq("BR")
      xml = Makiri::XML(%(<r xmlns:h="#{XHTML_NS}"><h:BR>t</h:BR></r>))
      expect { doc.import_node(xml.root.element_children.first, true) }
        .to raise_error(Makiri::Error, /upper-case name/)
    end

    # The same through import_node of an UNPREFIXED XHTML element, which was
    # made by its lower-cased name: `BR` came out the void `br` with its text
    # gone, and `Foo` renamed `foo`.
    it "imports an unprefixed upper-case XHTML name as createElementNS would" do
      %w[BR INPUT].each do |name|
        xml = Makiri::XML(%(<#{name} xmlns="#{XHTML_NS}">t</#{name}>))
        expect { doc.import_node(xml.root, true) }.to raise_error(Makiri::Error, /upper-case name/)
      end
      { "Foo" => "Foo", "MY-EL" => "MY-EL", "div" => "div" }.each do |name, want|
        el = doc.import_node(Makiri::XML(%(<#{name} xmlns="#{XHTML_NS}">t</#{name}>)).root, true)
        expect([el.name, el.namespace_uri, el.text]).to eq([want, XHTML_NS, "t"])
      end
    end

    it "keeps a prefix" do
      el = doc.create_element_ns(SVG_NS, "s:rect")
      expect([el.name, el.namespace_uri, el.to_html]).to eq(["s:rect", SVG_NS, "<s:rect></s:rect>"])
    end
  end

  describe "set_attribute_ns and the XML namespace" do
    # WPT Element-removeAttributeNS.html / attributes.html ("XML-namespaced
    # attributes don't need an xml prefix"): the DOM's validate and extract
    # binds `xml` to the XML namespace, not the namespace to `xml`.
    it "takes it under another prefix on an HTML element, keyed by local name" do
      el = Makiri::HTML("<p></p>").at_css("p")
      el.set_attribute_ns(XML_NS, "a:bb", "pass")
      attr = el.attribute_nodes.first
      expect([attr.name, attr.namespace_uri, attr.local_name, attr.prefix, attr.value])
        .to eq(["a:bb", XML_NS, "bb", "a", "pass"])
      el.remove_attribute_ns(XML_NS, "a:bb")
      expect(el.attribute_nodes.size).to eq(1)
      el.remove_attribute_ns(XML_NS, "bb")
      expect(el.attribute_nodes).to be_empty
    end

    it "takes it with no prefix, on HTML and XML elements alike" do
      [Makiri::HTML("<p></p>").at_css("p"), Makiri::XML("<r/>").root].each do |el|
        el.set_attribute_ns(XML_NS, "bb", "v")
        expect(el.attribute_nodes.map { [_1.name, _1.namespace_uri, _1.local_name] }).to eq([["bb", XML_NS, "bb"]])
      end
    end

    it "still refuses xml under another namespace" do
      [Makiri::HTML("<p></p>").at_css("p"), Makiri::XML("<r/>").root].each do |el|
        expect { el.set_attribute_ns("urn:x", "xml:bb", "v") }.to raise_error(Makiri::Error, /does not fit/)
      end
    end
  end
end
