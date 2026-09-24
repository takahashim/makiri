# frozen_string_literal: true

require "spec_helper"

# Cross-kind Document#import_node: translate a subtree between the HTML (Lexbor)
# and XML (mkr_xml) representations, preserving structure and namespaces. The
# import is a DETACHED deep/shallow copy owned by the target document; the source
# is untouched.
RSpec.describe "cross-kind import_node" do
  describe "HTML -> XML" do
    let(:xml)  { Makiri::XML("<root/>") }
    let(:html) { Makiri::HTML("<div id='a' class='c'>hi<!--cm--><span>x</span></div>") }

    it "deep-translates an element subtree (name, attributes, children, order)" do
      el = xml.import_node(html.at_css("div"), true)
      expect(el).to be_a(Makiri::XML::Element)
      expect(el.name).to eq("div")
      expect(el["id"]).to eq("a")
      expect(el["class"]).to eq("c")
      expect(el.children.map(&:class)).to eq(
        [Makiri::XML::Text, Makiri::XML::Comment, Makiri::XML::Element]
      )
      expect(el.at_xpath(".//*[local-name()='span']").text).to eq("x")
    end

    it "shallow import copies the node + attributes but no children" do
      el = xml.import_node(html.at_css("div"), false)
      expect(el.name).to eq("div")
      expect(el["id"]).to eq("a")
      expect(el.children).to be_empty
    end

    it "translates text, comment, and PI leaf nodes" do
      # HTML parses "<?pi?>" as a bogus comment, so build a real PI via the DOM.
      h = Makiri::HTML("<body>t<!--c--></body>")
      body = h.at_css("body")
      pi = h.create_processing_instruction("pi", "data")
      expect(xml.import_node(body.children[0], true)).to be_a(Makiri::XML::Text)
      expect(xml.import_node(body.children[1], true)).to be_a(Makiri::XML::Comment)
      pix = xml.import_node(pi, true)
      expect(pix).to be_a(Makiri::XML::ProcessingInstruction)
    end

    it "leaves the source untouched and returns a node owned by the target doc" do
      div = html.at_css("div")
      el  = xml.import_node(div, true)
      expect(div.document).to be(html)        # source unchanged
      expect(el.document).to be(xml)          # copy owned by target
      el.unlink rescue nil
    end

    it "can be appended into the XML tree after import" do
      el = xml.import_node(html.at_css("span"), true)
      xml.root.add_child(el)
      expect(xml.root.at_xpath(".//*[local-name()='span']")).not_to be_nil
    end

    # DOM importNode / adoptNode never re-validate an existing node's name (the
    # source is already well-formed for its representation). An HTML element name
    # that is a valid DOM name but NOT a well-formed XML QName must therefore be
    # taken verbatim as a DOM-loose name, not rejected - the same non-serializable
    # escape hatch as create_loose_dom_element (WPT dom/nodes/Document-adoptNode).
    describe "DOM-lenient element names (not well-formed XML QNames)" do
      let(:hdoc) { Makiri::HTML::Document.parse("<div></div>") }

      # Names the DOM accepts but XML does not. ("0:a" and "a b" were here too,
      # but the DOM's own rule refuses them, and create_element now applies it.)
      it "imports a lenient-named element verbatim instead of raising" do
        %w[:good:times: x< f}oo xmlns:foo].each do |nm|
          el  = hdoc.create_element(nm)
          imp = xml.import_node(el, true)
          expect(imp).to be_a(Makiri::XML::Element)
          expect(imp.name).to eq(nm)
        end
      end

      it "preserves the element's namespace (HTML elements are XHTML)" do
        imp = xml.import_node(hdoc.create_element(":good:times:"), true)
        expect(imp.namespace&.href).to eq("http://www.w3.org/1999/xhtml")
      end

      it "marks the import non-serializable (a DOM-loose name is not valid XML)" do
        imp = xml.import_node(hdoc.create_element(":good:times:"), true)
        expect { imp.to_xml }.to raise_error(Makiri::Error, /DOM-loose/)
      end

      it "keeps a valid QName on the strict, serializable path" do
        imp = xml.import_node(hdoc.create_element("section"), true)
        expect(imp.name).to eq("section")
        expect(imp.to_xml).to include("section")   # serializes (not loose)
      end

      it "deep-imports lenient-named elements with children" do
        parent = hdoc.create_element(":p:")
        parent.add_child(hdoc.create_element("span"))
        imp = xml.import_node(parent, true)
        expect(imp.name).to eq(":p:")
        expect(imp.children.map(&:name)).to eq(["span"])
      end

      it "can be appended into the XML tree after import" do
        imp = xml.import_node(hdoc.create_element(":good:times:"), true)
        xml.root.add_child(imp)
        expect(xml.root.children.map(&:name)).to include(":good:times:")
      end

      it "still copies valid attributes on a loose-named element" do
        el = hdoc.create_element(":x:")
        el["id"] = "a"
        imp = xml.import_node(el, true)
        expect(imp["id"]).to eq("a")
      end
    end
  end

  describe "XML -> HTML" do
    let(:html) { Makiri::HTML("<body></body>") }
    let(:xml)  { Makiri::XML("<r a='1'><c>txt</c><!--cm--><?pi d?></r>") }

    it "deep-translates an element subtree" do
      el = html.import_node(xml.at_xpath("//r"), true)
      expect(el).to be_a(Makiri::HTML::Element)
      expect(el.name).to eq("r")
      expect(el["a"]).to eq("1")
      expect(el.at_css("c").text).to eq("txt")
    end

    it "shallow import copies the node + attributes but no children" do
      el = html.import_node(xml.at_xpath("//r"), false)
      expect(el.name).to eq("r")
      expect(el["a"]).to eq("1")
      expect(el.children).to be_empty
    end

    it "rejects an XML CDATA section (HTML has no CDATA): fail-closed" do
      cd = Makiri::XML("<r><![CDATA[hi]]></r>").at_xpath("//r").children.first
      expect(cd).to be_a(Makiri::XML::CDATASection)
      expect { html.import_node(cd, true) }.to raise_error(Makiri::Error)
    end

    it "fails closed for a subtree containing a CDATA descendant" do
      r = Makiri::XML("<r><c/><![CDATA[x]]></r>").at_xpath("//r")
      expect { html.import_node(r, true) }.to raise_error(Makiri::Error)
    end

    it "can be appended into the HTML tree after import" do
      el = html.import_node(xml.at_xpath("//c"), true)
      html.at_css("body").add_child(el)
      expect(html.at_css("body c")).not_to be_nil
    end
  end

  describe "<template> content (kept in a separate fragment by HTML)" do
    it "HTML -> XML carries template contents as ordinary children (no silent drop)" do
      xml = Makiri::XML("<root/>")
      tpl = Makiri::HTML("<template><p>x</p></template>").at_css("template")
      el = xml.import_node(tpl, true)
      expect(el.name).to eq("template")
      expect(el.at_xpath(".//*[local-name()='p']")&.text).to eq("x")
    end

    it "XML -> HTML preserves a template element's children" do
      html = Makiri::HTML("<body/>")
      t = Makiri::XML("<template><p>x</p></template>").at_xpath("//template")
      el = html.import_node(t, true)
      expect(el.name).to eq("template")
      expect(el.to_html).to include("<p>x</p>")
    end

    it "XML -> HTML routes an HTML-namespaced template's content into the content fragment" do
      xhtml = "http://www.w3.org/1999/xhtml"
      t = Makiri::XML(%(<template xmlns="#{xhtml}"><p>x</p></template>))
            .at_xpath("//*[local-name()='template']")
      el = Makiri::HTML("<body/>").import_node(t, true)
      expect(el.children).to be_empty                              # not normal children (DOM-correct)
      expect(el.content_fragment.children.map(&:name)).to eq(["p"]) # HTMLTemplateElement.content
    end
  end

  describe "namespace fidelity" do
    SVG   = "http://www.w3.org/2000/svg"
    XLINK = "http://www.w3.org/1999/xlink"
    XHTML = "http://www.w3.org/1999/xhtml"

    it "XML -> HTML preserves element and attribute namespaces" do
      xdoc = Makiri::XML(%(<svg xmlns="#{SVG}" xmlns:xlink="#{XLINK}"><a xlink:href="u"/></svg>))
      el = Makiri::HTML("<body/>").import_node(xdoc.root, true)
      expect(el.namespace_uri).to eq(SVG)
      child = el.children.first
      expect(child.namespace_uri).to eq(SVG)            # inherited default ns
      expect(child.attribute_nodes.first.namespace_uri).to eq(XLINK)
    end

    it "XML -> HTML preserves a namespace Lexbor does not know (interned)" do
      xn = Makiri::XML(%(<e xmlns="urn:elem" xmlns:c="urn:attr" c:k="v"/>)).root
      el = Makiri::HTML("<body/>").import_node(xn, true)
      expect(el.namespace_uri).to eq("urn:elem")             # element ns interned, not lost
      ck = el.attribute_nodes.find { |a| a.name == "c:k" }
      expect(ck.namespace_uri).to eq("urn:attr")             # attribute ns too (symmetric)
    end

    it "HTML -> XML preserves a foreign (SVG + xlink) subtree through linking" do
      svg = Makiri::HTML("<div><svg><a xlink:href='u'><rect/></a></svg></div>").at_css("svg")
      xml = Makiri::XML(%(<root xmlns="urn:x"/>))
      imp = xml.import_node(svg, true)
      xml.root.add_child(imp) # triggers namespace re-resolution at insertion
      expect(imp.namespace_uri).to eq(SVG)
      expect(imp.at_xpath(".//*[local-name()='rect']").namespace_uri).to eq(SVG)
      expect(imp.to_xml).to include(%(xmlns="#{SVG}"))
    end

    it "HTML -> XML keeps HTML elements in the XHTML namespace" do
      div = Makiri::HTML("<div><p>x</p></div>").at_css("div")
      xml = Makiri::XML("<r/>")
      imp = xml.import_node(div, true)
      xml.root.add_child(imp)
      expect(imp.namespace_uri).to eq(XHTML)
      expect(imp.children.first.namespace_uri).to eq(XHTML) # inherited, declared once at root
    end
  end

  describe "same-document import_node still works on both kinds" do
    it "XML -> XML (deep + shallow)" do
      xml = Makiri::XML("<r><a x='1'><b/></a></r>")
      a = xml.at_xpath("//a")
      deep = xml.import_node(a, true)
      expect(deep.name).to eq("a")
      expect(deep["x"]).to eq("1")
      expect(deep.children.map(&:name)).to eq(["b"])
      shallow = xml.import_node(a, false)
      expect(shallow.children).to be_empty
    end

    it "HTML -> HTML (deep)" do
      html = Makiri::HTML("<div><p>x</p></div>")
      imp = html.import_node(html.at_css("div"), true)
      expect(imp.at_css("p").text).to eq("x")
    end
  end

  describe "namespace declarations crossing from HTML to XML" do
    # A foreign element's xmlns:xlink is a declaration already; declaring its
    # "prefix" wrote xmlns:xmlns, which no parser accepts.
    it "copies a foreign element's declarations without declaring xmlns" do
      html = Makiri.HTML(%(<svg xmlns:xlink="http://www.w3.org/1999/xlink"><a xlink:href="#u"/></svg>))
      xml = Makiri::XML("<r/>")
      xml.root.add_child(xml.import_node(html.at_css("svg"), true))
      expect(xml.to_xml).not_to include("xmlns:xmlns")
      back = Makiri::XML(xml.to_xml)
      expect(back.at_xpath("//@*[local-name()='href']").namespace_uri).to eq("http://www.w3.org/1999/xlink")
    end

    # In HTML an xmlns attribute is only an attribute; copied, it became a
    # declaration and moved the element out of XHTML.
    it "leaves out an HTML element's xmlns attribute" do
      html = Makiri.HTML(%(<div xmlns="urn:bogus"></div>))
      xml = Makiri::XML("<r/>")
      div = xml.import_node(html.at_css("div"), true)
      xml.root.add_child(div)
      expect(div.namespace_uri).to eq("http://www.w3.org/1999/xhtml")
    end
  end

  describe "names with a colon and no namespace" do
    svg_ns = "http://www.w3.org/2000/svg"

    # Lexbor stores a plain attribute under its element's namespace; the copy
    # read that, and a parsed q:y inside <svg> came out in SVG.
    it "refuses an HTML attribute whose prefix has no namespace instead of inventing one" do
      html = Makiri.HTML(%(<svg q:y="2"></svg>))
      expect(html.xpath("namespace-uri(//@*)")).to eq("")
      expect { Makiri::XML("<r/>").import_node(html.at_xpath("//*[local-name()='svg']"), true) }
        .to raise_error(Makiri::Error, /does not fit the qualified name/)
    end

    it "keeps xml:lang in the XML namespace" do
      html = Makiri.HTML(%(<div xml:lang="en"></div>))
      xml = Makiri::XML("<r/>")
      xml.root << xml.import_node(html.at_css("div"), true)
      lang = Makiri::XML(xml.to_xml).at_xpath("//@*[local-name()='lang']")
      expect([lang.value, lang.namespace_uri]).to eq(["en", "http://www.w3.org/XML/1998/namespace"])
    end

    # fb:like is one DOM local name. Made strictly, fb became a prefix bound to
    # nothing and the copy could not even be inserted.
    it "copies an HTML element named with a colon as a DOM-loose name" do
      html = Makiri.HTML(%(<div><fb:like></fb:like></div>))
      xml = Makiri::XML("<r/>")
      copy = xml.import_node(html.at_css("div"), true)
      xml.root << copy
      expect(xml.xpath("local-name(//*[@* or not(*)])")).to eq("fb:like")
      expect { xml.to_xml }.to raise_error(Makiri::Error, /DOM-loose/)
    end

    it "gives an SVG attribute set in SVG's own namespace that namespace" do
      html = Makiri.HTML("<svg></svg>")
      html.at_xpath("//*[local-name()='svg']").set_attribute_ns(svg_ns, "q:x", "1")
      expect(html.xpath("namespace-uri(//@*[local-name()='x'])")).to eq(svg_ns)
    end
  end

  describe "prefix collisions and namespaced attributes crossing into XML" do
    # A declaration is one attribute per prefix, so declaring an attribute's
    # prefix overwrote the element's own: p:e in urn:p moved into urn:other.
    it "keeps the element's namespace when an attribute uses its prefix for another URI" do
      src = Makiri::XML(%(<r xmlns:p="urn:p"><p:e><p:f/></p:e></r>))
      e = Makiri.HTML("<div></div>").import_node(src.at_xpath("//*[local-name()='e']"), true)
      e.set_attribute_ns("urn:other", "p:x", "1")
      xml = Makiri::XML(%(<root xmlns:p="urn:p"/>))
      xml.root << xml.import_node(e, true)
      reread = Makiri::XML(xml.to_xml)
      expect(reread.xpath("count(//q:e/q:f)", "q" => "urn:p")).to eq(1.0)
      expect(reread.xpath("string(//@*[namespace-uri()='urn:other'])")).to eq("1")
      expect(xml.to_xml.scan('xmlns:p="urn:p"').length).to eq(2) # root and e, not f
    end

    it "keeps each attribute's own namespace, prefixed or not" do
      div = Makiri.HTML("<div></div>").at_css("div")
      div.set_attribute_ns("urn:a", "q:x", "1")
      div.set_attribute_ns("urn:b", "q:y", "2")
      div.set_attribute_ns("urn:a", "z", "3")
      xml = Makiri::XML("<r/>")
      xml.root << xml.import_node(div, true)
      attrs = Makiri::XML(xml.to_xml).xpath("//@*").map { |a| [a.local_name, a.namespace_uri] }
      expect(attrs).to eq([%w[x urn:a], %w[y urn:b], %w[z urn:a]])
    end

    it "refuses a malformed attribute name as a malformed name" do
      %w[:class a:b:c].each do |name|
        html = Makiri.HTML(%(<div #{name}="1"></div>))
        expect { Makiri::XML("<r/>").import_node(html.at_css("div"), true) }.to raise_error(ArgumentError)
      end
    end
  end

  describe "an XML element's case crossing into HTML" do
    # createElement lower-cases; an element outside XHTML keeps its name as
    # written, as the parser keeps SVG's.
    it "keeps the case of a name outside XHTML" do
      xml = Makiri::XML(%(<r><svg xmlns="http://www.w3.org/2000/svg"><linearGradient/></svg><Foo xmlns="urn:p"/></r>))
      html = Makiri.HTML("<div></div>")
      xml.root.element_children.each { |e| html.at_css("div") << html.import_node(e, true) }
      expect(html.at_css("div").inner_html).to include("<linearGradient>", "<Foo ")
      expect(html.xpath("count(//s:linearGradient)", "s" => "http://www.w3.org/2000/svg")).to eq(1.0)
    end
  end

  describe "a prefixed XML element crossing into HTML" do
    it "keeps its prefix out of its local name, and comes back as it was" do
      xml = Makiri::XML(%(<r xmlns:p="urn:p"><p:e p:a="1" b="2"><p:f/></p:e></r>))
      html = Makiri.HTML("<div></div>")
      e = html.import_node(xml.at_xpath("//*[local-name()='e']"), true)
      html.at_css("div") << e
      expect(html.xpath("local-name(//*[@b])")).to eq("e")
      expect(html.xpath("//q:e/@q:a", "q" => "urn:p").size).to eq(1)

      back = Makiri::XML("<root/>")
      back.root << back.import_node(e, true)
      reread = Makiri::XML(back.to_xml)
      expect(reread.xpath("//q:e/q:f", "q" => "urn:p").size).to eq(1)
    end
  end
end
