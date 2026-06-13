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

    it "XML -> HTML maps an unknown namespace to the null namespace (fail-soft)" do
      xn = Makiri::XML(%(<r xmlns="urn:custom"/>)).root
      el = Makiri::HTML("<body/>").import_node(xn, true)
      expect(el.namespace_uri).to be_nil
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
end
