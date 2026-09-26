# frozen_string_literal: true

require "spec_helper"

# The namespace model: a node's namespace URI is its IDENTITY, decided once (by
# the parser, or by the context it is first inserted into) and carried from then
# on. Moving a node does not change it; the serializer emits whatever xmlns
# declarations the output needs to reproduce it.
#
# This is the WHATWG DOM model, and every expectation below was measured against
# Chrome 152 (DOMParser + XMLSerializer) rather than derived from the spec text.
# Makiri previously re-derived the URI from the declarations around the node on
# every insertion, which made a move silently change `namespace_uri` and let
# `#delete("xmlns:p")` produce XML that Makiri itself could not re-parse.
#
# libxml2/Nokogiri is NOT the reference here: it keeps the URI on an in-document
# move but does not emit the declaration, so its own tree and its own output
# disagree. See the plan's §10.19.
RSpec.describe "Makiri::XML namespace model" do
  def resolved(node, out = [])
    node.children.each do |c|
      next unless c.node_type == 1

      out << [c.name, c.namespace_uri]
      resolved(c, out)
    end
    out
  end

  # The invariant that matters: the tree agrees with what re-parsing its own
  # output computes. It is the serializer, not the tree, that keeps it true.
  def expect_round_trips(doc)
    expect(resolved(doc)).to eq(resolved(Makiri::XML(doc.root.to_xml)))
  end

  describe "a move keeps the namespace" do
    let(:doc) do
      Makiri::XML(%(<r xmlns:p="urn:a"><p:x/><mid xmlns:p="urn:c"><hole/></mid></r>))
    end

    it "does not re-derive the URI from the new context" do
      x = doc.root.children[0]
      doc.root.children[1].children[0].add_child(x)

      expect(x.namespace_uri).to eq("urn:a")
      expect(doc.at_xpath("//p:x", "p" => "urn:a")).not_to be_nil
      expect(doc.at_xpath("//p:x", "p" => "urn:c")).to be_nil
    end

    it "declares the prefix at the moved node so the output re-parses the same" do
      doc.root.children[1].children[0].add_child(doc.root.children[0])

      expect(doc.root.to_xml).to eq(
        %(<r xmlns:p="urn:a"><mid xmlns:p="urn:c"><hole><p:x xmlns:p="urn:a"/></hole></mid></r>)
      )
      expect_round_trips(doc)
    end

    it "declares nothing when the destination already binds the prefix the same way" do
      d = Makiri::XML(%(<r xmlns:p="urn:a"><p:x/><mid xmlns:p="urn:a"><hole/></mid></r>))
      d.root.children[1].children[0].add_child(d.root.children[0])

      expect(d.root.to_xml).to eq(
        %(<r xmlns:p="urn:a"><mid xmlns:p="urn:a"><hole><p:x/></hole></mid></r>)
      )
    end

    it "declares on the subtree root only, not on every descendant" do
      d = Makiri::XML(
        %(<r xmlns:p="urn:a"><p:x><p:y><p:z/></p:y></p:x><mid xmlns:p="urn:c"><hole/></mid></r>)
      )
      d.root.children[1].children[0].add_child(d.root.children[0])

      expect(d.root.to_xml).to eq(
        %(<r xmlns:p="urn:a"><mid xmlns:p="urn:c">) +
        %(<hole><p:x xmlns:p="urn:a"><p:y><p:z/></p:y></p:x></hole></mid></r>)
      )
    end

    it "carries the default namespace too" do
      d = Makiri::XML(%(<r xmlns="urn:a"><x/><mid xmlns="urn:c"><hole/></mid></r>))
      x = d.root.children[0]
      d.root.children[1].children[0].add_child(x)

      expect(x.namespace_uri).to eq("urn:a")
      expect(d.root.to_xml).to eq(
        %(<r xmlns="urn:a"><mid xmlns="urn:c"><hole><x xmlns="urn:a"/></hole></mid></r>)
      )
    end

    # "No namespace" is a decision, not an absence: moving such an element under
    # a default namespace must not quietly put it in one.
    it "keeps no-namespace with xmlns=\"\"" do
      d = Makiri::XML(%(<r><x/><mid xmlns="urn:c"><hole/></mid></r>))
      x = d.root.children[0]
      d.root.children[1].children[0].add_child(x)

      expect(x.namespace_uri).to be_nil
      expect(d.root.to_xml).to eq(%(<r><mid xmlns="urn:c"><hole><x xmlns=""/></hole></mid></r>))
    end

    it "leaves a node that carries its own declaration alone" do
      d = Makiri::XML(
        %(<r xmlns:p="urn:a"><p:x xmlns:p="urn:own"><p:y/></p:x><mid xmlns:p="urn:c"><hole/></mid></r>)
      )
      d.root.children[1].children[0].add_child(d.root.children[0])

      expect(d.root.to_xml).to eq(
        %(<r xmlns:p="urn:a"><mid xmlns:p="urn:c">) +
        %(<hole><p:x xmlns:p="urn:own"><p:y/></p:x></hole></mid></r>)
      )
    end
  end

  describe "a declaration change does not move existing nodes" do
    it "leaves descendants where they were when a declaration is removed" do
      doc = Makiri::XML(%(<r xmlns:p="urn:a"><mid xmlns:p="urn:b"><p:x/></mid></r>))
      doc.root.children.first.delete("xmlns:p")

      expect(doc.at_xpath("//p:x", "p" => "urn:b")).not_to be_nil
      expect_round_trips(doc)
    end

    it "leaves descendants where they were when a declaration is added" do
      doc = Makiri::XML(%(<r xmlns:p="urn:a"><mid><p:x/></mid></r>))
      doc.root.children.first["xmlns:p"] = "urn:b"

      expect(doc.at_xpath("//p:x", "p" => "urn:a")).not_to be_nil
      expect_round_trips(doc)
    end

    # The old model produced <r><p:x/></r> here - XML that Makiri could not
    # re-parse. Carrying the URI and declaring it at the node fixes it.
    it "still serializes to well-formed XML after the last declaration goes" do
      doc = Makiri::XML(%(<r xmlns:p="urn:a"><p:x><p:y/></p:x></r>))
      doc.root.delete("xmlns:p")

      expect(doc.root.to_xml).to eq(%(<r><p:x xmlns:p="urn:a"><p:y/></p:x></r>))
      expect_round_trips(doc)
    end
  end

  describe "copies keep the namespace they had" do
    let(:doc) do
      Makiri::XML(%(<r xmlns:p="urn:a"><p:x/><mid xmlns:p="urn:c"><hole/></mid></r>))
    end

    it "clone_node keeps it and declares it at the insertion point" do
      clone = doc.root.children[0].clone_node(true)
      doc.root.children[1].children[0].add_child(clone)

      expect(clone.namespace_uri).to eq("urn:a")
      expect(doc.root.to_xml).to eq(
        %(<r xmlns:p="urn:a"><p:x/><mid xmlns:p="urn:c"><hole><p:x xmlns:p="urn:a"/></hole></mid></r>)
      )
    end

    it "import_node keeps it (DOM importNode does not re-resolve)" do
      src = Makiri::XML(%(<a xmlns:p="urn:a"><p:x><p:y/></p:x></a>))
      tgt = Makiri::XML(%(<b xmlns:p="urn:c"><hole/></b>))
      imp = tgt.import_node(src.root.children.first, true)

      expect(imp.namespace_uri).to eq("urn:a")
      tgt.root.children.first.add_child(imp)
      expect(imp.namespace_uri).to eq("urn:a")
      expect(tgt.root.to_xml).to eq(
        %(<b xmlns:p="urn:c"><hole><p:x xmlns:p="urn:a"><p:y/></p:x></hole></b>)
      )
    end

    # An attribute's namespace state (explicit, or pending its insertion) is
    # its own field; a copy that carried only the node flags dropped it, so
    # the copy's attribute came back in no namespace.
    def attr_namespaces(el)
      el.attribute_nodes.map { |a| [a.name, a.namespace_uri] }
    end

    it "clone_node keeps an attribute's explicit namespace" do
      doc = Makiri::XML("<r/>")
      e = doc.create_element("e")
      e.set_attribute_ns("urn:a", "x", "1")
      doc.root.add_child(clone = e.clone_node(true))

      expect(attr_namespaces(clone)).to eq([%w[x urn:a]])
      expect(doc.root.to_xml).to eq(%(<r><e xmlns:ns1="urn:a" ns1:x="1"/></r>))
    end

    it "import_node keeps a detached element's explicit attribute namespace" do
      src = Makiri::XML("<s/>")
      e = src.create_element("e")
      e.set_attribute_ns("urn:a", "x", "1")
      tgt = Makiri::XML("<t/>")
      tgt.root.add_child(imp = tgt.import_node(e, true))

      expect(attr_namespaces(imp)).to eq([%w[x urn:a]])
    end

    it "clone_node keeps an attribute's pending prefix for its insertion point" do
      doc = Makiri::XML(%(<r xmlns:p="urn:p"><host/><gone/></r>))
      gone = doc.root.children[1]
      gone.remove
      gone["p:a"] = "v"
      doc.root.children[0].add_child(clone = gone.clone_node(true))

      expect(attr_namespaces(clone)).to eq([%w[p:a urn:p]])
    end

    it "imports into a document that does not bind the prefix at all" do
      src = Makiri::XML(%(<a xmlns:p="urn:a"><p:x/></a>))
      tgt = Makiri::XML(%(<b/>))
      tgt.root.add_child(tgt.import_node(src.root.children.first, true))

      expect(tgt.root.to_xml).to eq(%(<b><p:x xmlns:p="urn:a"/></b>))
    end
  end

  describe "a node that has no namespace yet takes one from its context" do
    # The factories build detached, unresolved nodes, so a subtree can be
    # assembled bottom-up and attached, giving the same tree as building it
    # top-down. Only the FIRST placement decides; later moves carry.
    it "resolves on first insertion, then carries" do
      doc = Makiri::XML(%(<r xmlns:p="urn:p"><host/><other xmlns:p="urn:q"><slot/></other></r>))
      wrap = doc.create_element("p:wrap")
      wrap.add_child(doc.create_element("p:inner"))
      doc.root.children[0].add_child(wrap)

      expect(wrap.namespace_uri).to eq("urn:p")
      expect(wrap.children.first.namespace_uri).to eq("urn:p")

      doc.root.children[1].children[0].add_child(wrap)
      expect(wrap.namespace_uri).to eq("urn:p")          # carried, not re-derived
      expect_round_trips(doc)
    end

    it "refuses an unbound prefix, leaving the subtree untouched" do
      doc = Makiri::XML(%(<r xmlns:p="urn:p"><host/></r>))
      wrap = doc.create_element("p:wrap")
      wrap.add_child(doc.create_element("q:inner"))      # q is bound nowhere

      expect { doc.root.children[0].add_child(wrap) }
        .to raise_error(Makiri::Error, /not bound/)

      # all-or-nothing: neither node took a namespace from the attempt
      expect(wrap.namespace_uri).to be_nil
      expect(wrap.children.first.namespace_uri).to be_nil
      expect(wrap.parent).to be_nil
    end
  end
end
