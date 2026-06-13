# frozen_string_literal: true

require "spec_helper"

# Cross-kind Document#import_node: translate a subtree between the HTML (Lexbor)
# and XML (mkr_xml) representations. Phase 1-3: structure is translated with
# null-namespace fidelity (namespace fidelity is a later phase). The import is a
# DETACHED deep/shallow copy owned by the target document; the source is untouched.
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
      expect(el.at_xpath(".//span").text).to eq("x")
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
      expect(xml.root.at_xpath(".//span")).not_to be_nil
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
      expect(el.at_xpath(".//p")&.text).to eq("x")
    end

    it "XML -> HTML preserves a template element's children" do
      html = Makiri::HTML("<body/>")
      t = Makiri::XML("<template><p>x</p></template>").at_xpath("//template")
      el = html.import_node(t, true)
      expect(el.name).to eq("template")
      expect(el.to_html).to include("<p>x</p>")
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
