# frozen_string_literal: true

require "spec_helper"

# Phase 2 XML mutation: building new subtrees — Document factories and node
# insertion (add_child/<<, before/after, replace), with namespace resolution at
# the insertion point, cross-document import (deep copy), and fail-closed guards.
RSpec.describe "Makiri::XML building (Phase 2)" do
  let(:doc) { Makiri::XML(%(<r xmlns:p="urn:p" xmlns="urn:d"><a/></r>)) }

  describe "Document factories" do
    it "creates an element (optionally with text content)" do
      el = doc.create_element("box")
      expect(el).to be_a(Makiri::XML::Element)
      expect(el.name).to eq("box")
      expect(el.parent).to be_nil          # detached until inserted
      expect(doc.create_element("p", "hi").text).to eq("hi")
    end

    it "creates text, comment, CDATA and PI nodes" do
      expect(doc.create_text_node("t")).to be_a(Makiri::XML::Text)
      expect(doc.create_comment(" c ")).to be_a(Makiri::XML::Comment)
      expect(doc.create_cdata("a < b")).to be_a(Makiri::XML::CData)
      pi = doc.create_processing_instruction("xml-stylesheet", %(href="a.xsl"))
      expect(pi).to be_a(Makiri::XML::ProcessingInstruction)
      expect(pi.name).to eq("xml-stylesheet")
    end

    it "fails closed on invalid factory input" do
      expect { doc.create_element("1bad") }.to raise_error(ArgumentError)
      expect { doc.create_text_node("xy") }.to raise_error(Makiri::Error) # non-XML-Char
      expect { doc.create_processing_instruction("xml", "x") }.to raise_error(ArgumentError) # reserved
      expect { doc.create_processing_instruction("ok", "a?>b") }.to raise_error(Makiri::Error) # "?>"
    end

    it "exposes Element.new / Text.new delegating to the document" do
      expect(Makiri::XML::Element.new("e", doc).name).to eq("e")
      expect(Makiri::XML::Text.new("hello", doc).text).to eq("hello")
      expect { Makiri::XML::Element.new("e", "not a doc") }.to raise_error(TypeError)
    end
  end

  describe "#add_child / #<<" do
    it "appends a created element and resolves its namespace against context" do
      child = doc.create_element("p:item")
      returned = doc.root.add_child(child)
      expect(returned).to eq(child)
      expect(child.parent).to eq(doc.root)
      expect(child.namespace_uri).to eq("urn:p")       # p resolved from the in-scope xmlns:p
      expect(doc.root.children.last.name).to eq("p:item")
    end

    it "resolves an unprefixed element to the in-scope default namespace" do
      child = doc.create_element("plain")
      doc.root.add_child(child)
      expect(child.namespace_uri).to eq("urn:d")       # the default xmlns
    end

    it "<< appends and returns the receiver (chainable)" do
      r = doc.root
      expect(r << doc.create_element("x")).to equal(r)
      expect(r.children.map(&:name)).to include("x")
    end

    it "is reflected by serialization and the XPath engine" do
      doc.root.add_child(doc.create_element("p:added"))
      expect(doc.root.to_xml).to include("<p:added/>")
      expect(doc.xpath("//p:added", "p" => "urn:p").length).to eq(1)
    end
  end

  describe "#before / #after / #replace" do
    it "places siblings in the right order" do
      a = doc.at_xpath("//d:a", "d" => "urn:d")
      a.before(doc.create_element("before"))
      a.after(doc.create_element("after"))
      expect(doc.root.children.map(&:name)).to eq(%w[before a after])
    end

    it "replaces a node in place, detaching the old one (still usable)" do
      a = doc.at_xpath("//d:a", "d" => "urn:d")
      a.add_child(doc.create_text_node("keep"))
      repl = doc.create_element("z")
      result = a.replace(repl)
      expect(result).to eq(repl)
      expect(doc.root.children.map(&:name)).to eq(%w[z])
      expect(a.parent).to be_nil          # detached
      expect(a.text).to eq("keep")        # detach-never-destroy: still readable
    end
  end

  describe "namespace resolution is fail-closed" do
    it "rejects inserting an element whose prefix is unbound, leaving the tree untouched" do
      before_xml = doc.root.to_xml
      orphan = doc.create_element("zzz:item")  # zzz is bound nowhere
      expect { doc.root.add_child(orphan) }.to raise_error(Makiri::Error, /not bound/)
      expect(orphan.parent).to be_nil
      expect(doc.root.to_xml).to eq(before_xml)
    end

    it "lets a moved subtree re-resolve against its new context" do
      # build <p:wrap><p:inner/></p:wrap> under the p-namespaced context, then move
      wrap = doc.create_element("p:wrap")
      inner = doc.create_element("p:inner")
      wrap.add_child(inner) # wrap not yet in a tree: resolves against wrap's own (empty) scope
      doc.root.add_child(wrap)
      expect(inner.namespace_uri).to eq("urn:p")
    end
  end

  describe "cross-document import (deep copy)" do
    it "imports a node from another document and keeps the original intact" do
      other = Makiri::XML(%(<o><deep a="1"><x/></deep></o>))
      src = other.at_xpath("//deep")
      imported = doc.root.add_child(src)

      expect(imported.document).to equal(doc)
      expect(imported).not_to equal(src)
      expect(imported["a"]).to eq("1")
      expect(doc.to_xml).to include(%(<deep a="1"><x/></deep>))
      # the source document is untouched
      expect(other.to_xml).to include(%(<deep a="1"><x/></deep>))
      expect(src.document).to equal(other)
    end

    it "produces an independent copy (editing the import does not touch the source)" do
      other = Makiri::XML("<o><deep/></o>")
      imported = doc.root.add_child(other.at_xpath("//deep"))
      imported["new"] = "v"
      expect(other.at_xpath("//deep")["new"]).to be_nil
    end
  end

  describe "fail-closed guards" do
    it "rejects a cycle, a second document root, and a foreign-representation node" do
      expect { doc.root.add_child(doc.root) }.to raise_error(Makiri::Error, /subtree/)
      expect { doc.add_child(doc.create_element("second")) }
        .to raise_error(Makiri::Error, /single root/)
      html_node = Makiri::HTML("<p/>").at_xpath("//p")
      expect { doc.root.add_child(html_node) }.to raise_error(TypeError)
    end

    it "honours Object#freeze on the receiver" do
      r = doc.root
      r.freeze
      expect { r.add_child(doc.create_element("x")) }.to raise_error(FrozenError)
    end
  end

  describe "round-trip" do
    it "builds a namespaced tree that re-parses to the same structure" do
      d = Makiri::XML(%(<feed xmlns="urn:a" xmlns:dc="urn:dc"/>))
      entry = d.create_element("entry")
      entry["dc:id"] = "42"
      entry.add_child(d.create_element("title", "Hello"))
      d.root.add_child(entry)

      re = Makiri::XML(d.to_xml)
      n = re.at_xpath("//a:entry", "a" => "urn:a")
      expect(n.namespace_uri).to eq("urn:a")
      expect(n.at_xpath("@dc:id", "dc" => "urn:dc").value).to eq("42")
      expect(n.at_xpath("a:title", "a" => "urn:a").text).to eq("Hello")
    end
  end
end
