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

  # Selectors 4 / the HTML Standard ("case-sensitivity of selectors"): a type
  # selector is lower-cased for an HTML element and compared to its localName,
  # and compared as written to any other element's. Lexbor folded case on
  # every element.
  describe "type selector case in an HTML document" do
    let(:doc) do
      d = Makiri::HTML(%(<p></p><svg><feGaussianBlur/><foreignObject><div></div></foreignObject></svg><math><mi/></math>))
      d.body << d.create_element_ns(XHTML_NS, "MY-EL")
      d
    end

    {
      "feGaussianBlur" => %w[feGaussianBlur],
      "fegaussianblur" => [],
      "FEGAUSSIANBLUR" => [],
      "foreignobject" => [],
      "foreignObject" => %w[foreignObject],
      "MI" => [],
      "mi" => %w[mi],
      "P" => %w[p],
      "div" => %w[div],
      "DIV" => %w[div],
      # an HTML element named in upper case matches no type selector
      "my-el" => [],
      "MY-EL" => []
    }.each do |sel, names|
      it "matches #{sel.inspect} as a browser does" do
        expect(doc.css(sel).map(&:name)).to eq(names)
      end
    end

    it "answers matches? and :is() by the same rule" do
      blur = doc.at_css("feGaussianBlur")
      expect(blur.matches?("feGaussianBlur")).to be(true)
      expect(blur.matches?("fegaussianblur")).to be(false)
      expect(doc.css(":is(fegaussianblur, mi)").map(&:name)).to eq(%w[mi])
      expect(doc.css("svg|fegaussianblur", "svg" => SVG_NS)).to be_empty
      expect(doc.css("svg|feGaussianBlur", "svg" => SVG_NS).size).to eq(1)
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

  # WPT XMLSerializer-serializeToString.html, "Check if a prefix bound to an
  # empty namespace URI ("no namespace") serialize": the DOM holds
  # setAttributeNS(XMLNS, "xmlns:foo", "") as an attribute. Namespaces in XML
  # forbids the declaration, so it is kept as one that binds nothing.
  describe "XML set_attribute_ns with a declaration XML forbids" do
    let(:doc) { Makiri::XML(%(<root xmlns="" xmlns:foo="urn:bar"><c/></root>)) }
    let(:root) { doc.root }

    it "holds it as an XMLNS attribute that binds nothing" do
      root.set_attribute_ns(XMLNS_NS, "xmlns:foo", "")
      expect(root.attribute_nodes.map { [_1.name, _1.namespace_uri, _1.value] })
        .to eq([["xmlns", XMLNS_NS, ""], ["xmlns:foo", XMLNS_NS, ""]])
      expect(root.namespace_definitions.map(&:prefix)).to eq([nil])
      expect { root.at_xpath("c") << doc.create_element("foo:e") }.to raise_error(Makiri::Error, /not bound/)
      expect(root.xpath("@*").map(&:name)).to eq(["xmlns:foo"])
      expect { doc.to_xml }.to raise_error(Makiri::Error, /namespace declaration XML forbids/)
    end

    it "binds again once it is given a value it can hold" do
      root.set_attribute_ns(XMLNS_NS, "xmlns:foo", "")
      root.set_attribute_ns(XMLNS_NS, "xmlns:foo", "urn:q")
      expect(root.namespace_definitions.map { [_1.prefix, _1.href] }).to eq([[nil, ""], ["foo", "urn:q"]])
      expect(doc.to_xml).to include(%(xmlns:foo="urn:q"))
    end

    it "judges an existing attribute by its own name" do
      # xmlns:xmlns has the default declaration's key (XMLNS, "xmlns")
      root.set_attribute_ns(XMLNS_NS, "xmlns:xmlns", "urn:y")
      expect(root.namespace_definitions.map { [_1.prefix, _1.href] }).to include([nil, "urn:y"])
    end

    it "leaves []= refusing it: that names a declaration to make" do
      expect { root["xmlns:foo"] = "" }.to raise_error(Makiri::Error, /not permitted/)
      root.set_attribute_ns(XMLNS_NS, "xmlns:foo", "")
      expect { root["xmlns:foo"] = "" }.to raise_error(Makiri::Error, /not permitted/)
    end

    # Whether it binds is read from its value, whichever setter gave it: []=
    # once left a valid URI binding nothing, because only set_attribute_ns
    # recomputed a flag.
    it "binds or not by its value, whichever setter gave the value" do
      root.set_attribute_ns(XMLNS_NS, "xmlns:foo", "")
      root["xmlns:foo"] = "urn:foo"
      expect(root.namespace_definitions.map { [_1.prefix, _1.href] }).to include(["foo", "urn:foo"])
      root.at_xpath("c") << doc.create_element("foo:x")
      expect(doc.to_xml).to include(%(xmlns:foo="urn:foo"))
      root.set_loose_dom_attribute("xmlns:foo", "")
      expect(root.namespace_definitions.map(&:prefix)).not_to include("foo")
      root.set_loose_dom_attribute("xmlns:foo", "urn:z")
      expect(root.namespace_definitions.map { [_1.prefix, _1.href] }).to include(["foo", "urn:z"])
    end
  end

  # The DOM's setAttributeNS takes any valid attribute local name, which is
  # far looser than an NCName: `a}b` is one. XML's set_attribute_ns refused it
  # as no XML name; it is now held DOM-loose, as the HTML side already took it.
  describe "XML set_attribute_ns with a name XML cannot write" do
    let(:doc) { Makiri::XML("<r/>") }
    let(:root) { doc.root }

    it "splits it by the DOM's rule and keys it by namespace and local name" do
      root.set_attribute_ns("urn:u", "p:a}b", "v")
      attr = root.attribute_nodes.last
      expect([attr.name, attr.namespace_uri, attr.prefix, attr.local_name]).to eq(["p:a}b", "urn:u", "p", "a}b"])
      root.set_attribute_ns("urn:u", "a}b", "w") # the same (namespace, local name)
      expect(root.attribute_nodes.map { [_1.name, _1.value] }).to eq([["p:a}b", "w"]])
      root.set_attribute_ns("urn:u", "q:a:b", "v")
      expect(root.attribute_nodes.last.local_name).to eq("a:b")
    end

    it "refuses to serialize it, and not once it is gone" do
      root.set_attribute_ns("urn:u", "p:a}b", "v")
      expect { doc.to_xml }.to raise_error(Makiri::Error, /DOM-loose attribute/)
      expect { doc.canonicalize }.to raise_error(Makiri::Error, /DOM-loose attribute/)
      root.remove_attribute_ns("urn:u", "a}b")
      expect(doc.to_xml).to eq(%(<?xml version="1.0"?>\n<r/>\n))
    end

    it "still refuses what the DOM refuses" do
      expect { root.set_attribute_ns("urn:u", "a b", "v") }.to raise_error(ArgumentError)
      expect { root.set_attribute_ns("urn:u", ":a", "v") }.to raise_error(ArgumentError)
      expect { root.set_attribute_ns(nil, "p:a}b", "v") }.to raise_error(Makiri::Error, /does not fit/)
    end
  end

  describe "XML Element#set_loose_dom_attribute" do
    let(:doc) { Makiri::XML(%(<r xmlns:p="urn:p"><c/></r>)) }
    let(:root) { doc.root }

    def attrs_of(el)
      el.attribute_nodes.map { [_1.name, _1.namespace_uri, _1.local_name] }
    end

    it "makes a no-namespace attribute named by the whole qualified name" do
      %w[xmlns xml:lang xlink:href v-on:click : foo:bar].each { |n| root.set_loose_dom_attribute(n, "v") }
      expect(attrs_of(root).drop(1)).to eq(
        [["xmlns", nil, "xmlns"], ["xml:lang", nil, "xml:lang"], ["xlink:href", nil, "xlink:href"],
         ["v-on:click", nil, "v-on:click"], [":", nil, ":"], ["foo:bar", nil, "foo:bar"]]
      )
      expect(root["xlink:href"]).to eq("v")
    end

    it "does not make a declaration of an attribute named xmlns" do
      root.set_loose_dom_attribute("xmlns", "urn:d")
      root.set_loose_dom_attribute("xmlns:q", "urn:q")
      expect(root.namespace_definitions.map { [_1.prefix, _1.href] }).to eq([["p", "urn:p"]])
      added = doc.create_element("d")
      root.at_xpath("c") << added
      expect(added.namespace_uri).to be_nil
      expect { root.at_xpath("c").add_child(doc.create_element("q:e")) }.to raise_error(Makiri::Error, /not bound/)
      expect(root.xpath("@*").map(&:name)).to eq(%w[xmlns xmlns:q])
    end

    it "sets the value of the first attribute with that qualified name" do
      root.set_loose_dom_attribute("xmlns:p", "urn:p2")
      expect(root.namespace_definitions.map { [_1.prefix, _1.href] }).to eq([["p", "urn:p2"]])
      root.set_loose_dom_attribute("xmlns", "a")
      root["xmlns"] = "b"
      root.set_loose_dom_attribute("xmlns", "c")
      expect(attrs_of(root).map(&:first)).to eq(%w[xmlns:p xmlns])
      expect(root["xmlns"]).to eq("c")
    end

    it "makes a plain attribute of a name XML can write" do
      root.set_loose_dom_attribute("plain", "v")
      expect(doc.to_xml).to include(%(plain="v"))
    end

    it "refuses to serialize an attribute XML cannot write, and not after it is gone" do
      root.set_loose_dom_attribute("v-on:click", "f")
      expect { doc.to_xml }.to raise_error(Makiri::Error, /DOM-loose/)
      expect { root.to_xml }.to raise_error(Makiri::Error, /DOM-loose/)
      expect { doc.canonicalize }.to raise_error(Makiri::Error, /DOM-loose/)
      expect { root.dup.to_xml }.to raise_error(Makiri::Error, /DOM-loose/)
      root.remove_attribute_ns(nil, "v-on:click")
      expect(doc.to_xml).to eq(%(<?xml version="1.0"?>\n<r xmlns:p="urn:p"><c/></r>\n))
    end

    it "crosses into HTML as the no-namespace attribute it is" do
      root.set_loose_dom_attribute("xmlns", "urn:d")
      html = Makiri::HTML("<body></body>")
      el = html.import_node(root, false)
      expect(el.attribute_nodes.map { [_1.name, _1.namespace_uri] })
        .to eq([["xmlns:p", XMLNS_NS], ["xmlns", nil]])
    end

    it "holds the name to the DOM's rule and the value to XML's characters" do
      expect { root.set_loose_dom_attribute("a b", "v") }.to raise_error(ArgumentError, /invalid DOM attribute name/)
      expect { root.set_loose_dom_attribute("a=b", "v") }.to raise_error(ArgumentError)
      expect { root.set_loose_dom_attribute("", "v") }.to raise_error(ArgumentError)
      expect { root.set_loose_dom_attribute("a", "\u0001") }.to raise_error(Makiri::Error, /not permitted in XML/)
    end

    it "leaves set_attribute_ns the DOM's setAttributeNS" do
      expect { root.set_attribute_ns(nil, "foo:bar", "v") }.to raise_error(Makiri::Error, /does not fit/)
      expect { root.set_attribute_ns(nil, "xmlns", "v") }.to raise_error(Makiri::Error, /does not fit/)
    end
  end
end
