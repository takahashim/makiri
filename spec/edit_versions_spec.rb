# frozen_string_literal: true

require "spec_helper"

# The edit-version contract, held to one table for both representations: which
# version an edit moves follows from what it changes (bridge `EditKind`) - a
# child list moves tree_version, an attribute attribute_version, character data
# neither - and an edit refused before it starts moves neither.
RSpec.describe "edit versions, HTML and XML alike" do
  documents = {
    "HTML" => -> { Makiri::HTML(%(<div><p a="1">t<!--c--></p><i></i></div>)).then { |d| [d, d.at_css("p")] } },
    "XML" => -> { Makiri::XML(%(<div><p a="1">t<!--c--></p><i/></div>)).then { |d| [d, d.root.children.first] } },
  }

  # [what, edit, tree_version moves?, attribute_version moves?]
  edits = [
    ["add_child", ->(d, p) { p.add_child(d.create_element("n")) }, true, false],
    ["add_previous_sibling", ->(d, p) { p.add_previous_sibling(d.create_element("n")) }, true, false],
    ["add_next_sibling", ->(d, p) { p.add_next_sibling(d.create_element("n")) }, true, false],
    ["replace", ->(d, p) { p.children.first.replace(d.create_element("n")) }, true, false],
    ["remove an element", ->(_d, p) { p.remove }, true, false],
    ["element content=", ->(_d, p) { p.content = "x" }, true, false],
    ["adopt from another document", lambda { |d, p|
      other = d.is_a?(Makiri::XML::Document) ? Makiri::XML("<o><s/></o>").root : Makiri::HTML("<s></s>").at_css("s")
      p.add_child(other.is_a?(Makiri::XML::Element) ? other.children.first : other)
    }, true, false,],
    ["[]=", ->(_d, p) { p["b"] = "2" }, false, true],
    ["[]= to the same value", ->(_d, p) { p["a"] = "1" }, false, true],
    ["delete", ->(_d, p) { p.delete("a") }, false, true],
    ["set_attribute_ns", ->(_d, p) { p.set_attribute_ns("urn:k", "k:b", "1") }, false, true],
    ["remove_attribute_ns", ->(_d, p) { p.remove_attribute_ns(nil, "a") }, false, true],
    ["set_loose_dom_attribute", ->(_d, p) { p.set_loose_dom_attribute("v:x", "1") }, false, true],
    ["Attr#content=", ->(_d, p) { p.attribute_nodes.first.content = "z" }, false, true],
    ["Attr#remove", ->(_d, p) { p.attribute_nodes.first.remove }, false, true],
    ["Text#content=", ->(_d, p) { p.children.first.content = "x" }, false, false],
    ["Comment#content=", ->(_d, p) { p.children.last.content = "x" }, false, false],
    ["an edit of a frozen node", lambda { |_d, p|
      p.freeze
      begin
        p["b"] = "2"
      rescue FrozenError
        nil
      end
    }, false, false,],
  ]

  documents.each do |kind, make|
    context kind do
      edits.each do |what, edit, tree, attrs|
        it "#{what}: tree_version #{tree ? "moves" : "stays"}, attribute_version #{attrs ? "moves" : "stays"}" do
          doc, para = make.call
          before = [doc.tree_version, doc.attribute_version]
          edit.call(doc, para)
          after = [doc.tree_version, doc.attribute_version]
          expect([after[0] > before[0], after[1] > before[1]]).to eq([tree, attrs])
        end
      end
    end
  end

  # An XML insertion that decides an attribute's namespace changes what that
  # attribute reads, so it is an attribute edit too; moving a node whose
  # attributes are already decided is not.
  context "XML insertion" do
    let(:doc) { Makiri::XML(%(<r xmlns:p="urn:p"/>)) }

    it "moves attribute_version when it decides a pending attribute's namespace" do
      e = doc.create_element("e")
      e["p:a"] = "1"
      attr = e.attribute_nodes.first
      expect(attr.namespace_uri).to be_nil
      before = doc.attribute_version
      doc.root.add_child(e)
      expect(attr.namespace_uri).to eq("urn:p")
      expect(doc.attribute_version).to be > before
    end

    it "leaves attribute_version when the attributes are already decided" do
      e = doc.create_element("e")
      e["a"] = "1"
      doc.root.add_child(e)
      before = doc.attribute_version
      doc.root.add_child(doc.create_element("f"))
      doc.root.add_child(e)
      expect(doc.attribute_version).to eq(before)
    end
  end
end
