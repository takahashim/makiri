# frozen_string_literal: true

require "spec_helper"

# Phase 2 XML mutation: building new subtrees - Document factories and node
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

    it "creates an element with an attributes hash and/or text content (Nokogiri-style)" do
      expect(doc.create_element("box", "id" => "1", "class" => "c").to_xml)
        .to eq(%(<box id="1" class="c"/>))
      expect(doc.create_element("a", "hi", href: "/x").to_xml) # content + symbol-key attrs
        .to eq(%(<a href="/x">hi</a>))
      expect { doc.create_element("x", "xmlns:q" => "") }.to raise_error(Makiri::Error) # validated
    end

    it "reads an attributes Hash as a Hash, not through a redefined #to_a" do
      odd = Class.new(Hash) { def to_a = [1] }.new
      odd["id"] = "1"
      expect(doc.create_element("box", odd).to_xml).to eq(%(<box id="1"/>))
    end

    it "creates text, comment, CDATA and PI nodes" do
      expect(doc.create_text_node("t")).to be_a(Makiri::XML::Text)
      expect(doc.create_comment(" c ")).to be_a(Makiri::XML::Comment)
      expect(doc.create_cdata("a < b")).to be_a(Makiri::XML::CDATASection)
      pi = doc.create_processing_instruction("xml-stylesheet", %(href="a.xsl"))
      expect(pi).to be_a(Makiri::XML::ProcessingInstruction)
      expect(pi.name).to eq("xml-stylesheet")
      # DOM createProcessingInstruction takes any Name, colon included.
      expect(doc.create_processing_instruction("a:b", "d").name).to eq("a:b")
    end

    it "creates a PI target with a colon but refuses to serialize it" do
      # Namespaces in XML §7 makes a PI target an NCName, so the parser rejects
      # `<?a:b?>`; the DOM still creates one, and - as DOM Parsing's serializer
      # does - writing it out as XML fails closed.
      pi = doc.create_processing_instruction("a:b", "d")
      doc.root.add_child(pi)
      expect { pi.to_xml }.to raise_error(Makiri::Error, /target with a colon/)
      expect { doc.to_xml }.to raise_error(Makiri::Error, /target with a colon/)
      expect { doc.canonicalize }.to raise_error(Makiri::Error, /target with a colon/)
    end

    it "fails closed on invalid factory input" do
      expect { doc.create_element("1bad") }.to raise_error(ArgumentError)
      %w[f:o:o : foo: f<oo f}oo].each do |name|
        expect { doc.create_element(name) }.to raise_error(ArgumentError)
      end
      expect { doc.create_text_node("xy") }.to raise_error(Makiri::Error) # non-XML-Char
      expect { doc.create_processing_instruction("xml", "x") }.to raise_error(ArgumentError) # reserved
      expect { doc.create_processing_instruction("ok", "a?>b") }.to raise_error(Makiri::Error) # "?>"
    end

    it "creates DOM-loose elements for browser-DOM interop without loosening XML factories" do
      el = doc.create_loose_dom_element("f:o:o", "f", "o:o", "http://example.com/")
      expect(el).to be_a(Makiri::XML::Element)
      expect(el.name).to eq("f:o:o")
      expect(el.prefix).to eq("f")
      expect(el.local_name).to eq("o:o")
      expect(el.namespace_uri).to eq("http://example.com/")

      cases = [
        [":", nil, ":", nil],
        [":foo", nil, ":foo", nil],
        ["foo:", nil, "foo:", nil],
        ["prefix::local", "prefix", ":local", "http://example.com/"],
        ["0:a", "0", "a", "http://example.com/"],
        ["f<oo", nil, "f<oo", nil],
        ["f}oo", nil, "f}oo", nil],
        ["\uFFFFfoo", nil, "\uFFFFfoo", nil]
      ]
      cases.each do |qname, prefix, local, ns|
        n = doc.create_loose_dom_element(qname, prefix, local, ns)
        expect(n.name).to eq(qname)
        expect(n.prefix).to eq(prefix)
        expect(n.local_name).to eq(local)
        expect(n.namespace_uri).to eq(ns)
      end
    end

    it "keeps DOM-loose namespace metadata on insertion but refuses XML serialization" do
      loose = doc.create_loose_dom_element("f:o:o", "f", "o:o", "http://example.com/")
      doc.root.add_child(loose)
      expect(loose.parent).to eq(doc.root)
      expect(loose.namespace_uri).to eq("http://example.com/")
      expect { loose.to_xml }.to raise_error(Makiri::Error, /DOM-loose/)
      expect { doc.to_xml }.to raise_error(Makiri::Error, /DOM-loose/)
      expect { doc.canonicalize }.to raise_error(Makiri::Error, /DOM-loose/)
    end

    it "preserves DOM-loose metadata across clone and import" do
      loose = doc.create_loose_dom_element("prefix::local", "prefix", ":local", "urn:dom")
      clone = loose.clone_node
      expect(clone.name).to eq("prefix::local")
      expect(clone.prefix).to eq("prefix")
      expect(clone.local_name).to eq(":local")
      expect(clone.namespace_uri).to eq("urn:dom")
      expect { clone.to_xml }.to raise_error(Makiri::Error, /DOM-loose/)

      other = Makiri::XML("<root/>")
      imported = other.root.add_child(loose)
      expect(imported.document).to equal(other)
      expect(imported.name).to eq("prefix::local")
      expect(imported.prefix).to eq("prefix")
      expect(imported.local_name).to eq(":local")
      expect(imported.namespace_uri).to eq("urn:dom")
      expect { other.to_xml }.to raise_error(Makiri::Error, /DOM-loose/)
    end

    it "rejects an embedded NUL in XML data (U+0000 is not a legal XML char)" do
      # Unlike the HTML DOM, XML 1.0 cannot represent U+0000, so the data-family
      # NUL relaxation deliberately does NOT apply here: text/CDATA/comment content
      # and attribute values keep rejecting NUL, fail-closed, to stay well-formed.
      expect { doc.create_text_node("a\x00b") }.to raise_error(Makiri::Error)
      expect { doc.create_cdata("a\x00b") }.to raise_error(Makiri::Error)
      expect { doc.create_comment("a\x00b") }.to raise_error(Makiri::Error)
      expect { doc.root["k"] = "a\x00b" }.to raise_error(Makiri::Error)
      expect { doc.root.content = "a\x00b" }.to raise_error(Makiri::Error)
    end

    it "rejects content that would serialize to invalid XML (comment '--', CDATA ']]>')" do
      expect { doc.create_comment("a--b") }.to raise_error(Makiri::Error)      # "--" in a comment
      expect { doc.create_comment("trailing-") }.to raise_error(Makiri::Error) # a trailing "-"
      expect { doc.create_cdata("a]]>b") }.to raise_error(Makiri::Error)       # "]]>" closes the section
      # the same rules apply through #content=, not just the factories
      expect { doc.create_comment("ok").content = "x--y" }.to raise_error(Makiri::Error)
      expect { doc.create_cdata("ok").content = "x]]>y" }.to raise_error(Makiri::Error)
      # valid content is still accepted and round-trips
      expect(doc.create_comment("a-b").to_xml).to eq("<!--a-b-->")
      expect(doc.create_cdata("a < b").to_xml).to eq("<![CDATA[a < b]]>")
    end

    it "rejects binding a namespace prefix to the empty namespace" do
      expect { doc.root["xmlns:q"] = "" }.to raise_error(Makiri::Error)
      doc.root["xmlns"] = ""          # undeclaring the DEFAULT namespace is allowed
      doc.root["xmlns:q"] = "urn:q"   # a real prefix binding is fine
      expect(doc.root["xmlns:q"]).to eq("urn:q")
    end

    it "exposes node-class .new delegating to the document (Nokogiri argument order)" do
      # Element/Text take the document LAST; Comment/CDATA/PI take it FIRST.
      expect(Makiri::XML::Element.new("e", doc).name).to eq("e")
      expect(Makiri::XML::Text.new("hello", doc).text).to eq("hello")
      expect(Makiri::XML::Comment.new(doc, " c ").to_xml).to eq("<!-- c -->")
      expect(Makiri::XML::CDATASection.new(doc, "a < b").to_xml).to eq("<![CDATA[a < b]]>")
      pi = Makiri::XML::ProcessingInstruction.new(doc, "xml-stylesheet", %(href="s.xsl"))
      expect(pi).to be_a(Makiri::XML::ProcessingInstruction)
      expect(pi.to_xml).to eq(%(<?xml-stylesheet href="s.xsl"?>))
    end

    it "Node#cdata? is true only for a CDATA node (cf. text?/comment?)" do
      cdata = Makiri::XML::CDATASection.new(doc, "x")
      expect(cdata.cdata?).to be(true)
      expect(cdata.text?).to be(false)
      expect(cdata.comment?).to be(false)
      expect(Makiri::XML::Text.new("x", doc).cdata?).to be(false)
    end

    it "keeps the factory validation and Document type-check on the .new delegators" do
      expect { Makiri::XML::Element.new("e", "not a doc") }.to raise_error(TypeError)
      expect { Makiri::XML::Comment.new("not a doc", "x") }.to raise_error(TypeError)
      expect { Makiri::XML::Comment.new(doc, "a--b") }.to raise_error(Makiri::Error) # "--"
      expect { Makiri::XML::CDATASection.new(doc, "a]]>b") }.to raise_error(Makiri::Error)  # "]]>"
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

    it "treats inserting/replacing a node relative to itself as a no-op (no self-cycle)" do
      a = doc.at_xpath("//d:a", "d" => "urn:d")
      %i[before after add_previous_sibling add_next_sibling replace].each { |m| a.public_send(m, a) }
      # the sibling list stays well-formed: serialization terminates, re-parses identically
      expect(doc.root.children.map(&:name)).to eq(%w[a])
      expect(doc.to_xml).to eq(Makiri::XML(doc.to_xml).to_xml)
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

  # Inserting a node from another document ADOPTS it, as the DOM says
  # appendChild does and as both Chrome and Nokogiri do: it leaves the document
  # it came from. Makiri's arenas each own their nodes, so the node cannot be
  # relinked across them - it is copied here and removed there, which is the
  # same thing from the outside except that the copy is a different object.
  describe "cross-document insertion (adopt)" do
    it "brings the node over and takes it out of its old document" do
      other = Makiri::XML(%(<o><deep a="1"><x/></deep></o>))
      src = other.at_xpath("//deep")
      imported = doc.root.add_child(src)

      expect(imported.document).to equal(doc)
      expect(imported["a"]).to eq("1")
      # <deep> came from a document with no default namespace, so it is in NO
      # namespace - a decision the adoption carries. Landing it under this
      # document's xmlns="urn:d" must not quietly move it there, so the
      # serializer says so with xmlns="" (Chrome does the same).
      expect(doc.to_xml).to include(%(<deep xmlns="" a="1"><x/></deep>))
      expect(imported.namespace_uri).to be_nil

      expect(other.to_xml).not_to include("deep")      # gone from the source
      expect(other.at_xpath("//deep")).to be_nil
    end

    it "hands back the node that is now in the tree, not the argument" do
      other = Makiri::XML("<o><deep/></o>")
      src = other.at_xpath("//deep")
      imported = doc.root.add_child(src)

      expect(imported.pointer_id).not_to eq(src.pointer_id)
      expect(imported.document).to equal(doc)
      imported["new"] = "v"
      expect(doc.to_xml).to include(%(new="v"))
    end

    # Fail-closed: a rejected insert must not empty the source document.
    it "leaves the source alone when the insert is refused" do
      other = Makiri::XML("<o><deep/></o>")
      target = Makiri::XML("<r/>")

      expect { target.add_child(other.at_xpath("//deep")) }
        .to raise_error(Makiri::Error, /single root/)
      expect(other.to_xml).to include("<deep/>")
    end

    it "empties a fragment it splices, like a same-document one" do
      other = Makiri::XML("<o/>")
      frag = other.fragment("<m/><n/>")
      doc.root.add_child(frag)

      # both came from a document with no default namespace and keep it
      expect(doc.to_xml).to include(%(<m xmlns=""/><n xmlns=""/>))
      expect(frag.children.length).to eq(0)
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

    # Inserting a detached element re-derived every attribute's namespace from
    # its prefix, so a namespace given with set_attribute_ns was lost.
    it "keeps a namespace given with set_attribute_ns when the element is inserted" do
      xml = Makiri::XML(%(<r xmlns:q="urn:b"/>))
      el = xml.create_element("b")
      el.set_attribute_ns("urn:a", "x", "1")
      el.set_attribute_ns("urn:a", "q:y", "2")
      xml.root << el
      expect(el.attribute_nodes.map(&:namespace_uri)).to eq(%w[urn:a urn:a])
      expect(Makiri::XML(xml.to_xml).xpath("//@*").map(&:namespace_uri)).to eq(%w[urn:a urn:a])
    end

    it "says which namespace-declaration rule a refused declaration breaks" do
      {
        ["xmlns:xmlns", "urn:x"] => /xmlns cannot be declared/,
        ["xmlns:xml", "urn:x"] => /xml can only be bound to/,
        ["xmlns:p", "http://www.w3.org/2000/xmlns/"] => /cannot be bound to another prefix/,
        ["xmlns", "http://www.w3.org/XML/1998/namespace"] => /cannot be the default namespace/,
        ["xmlns:p", ""] => /a prefix cannot be bound to the empty namespace/
      }.each do |(name, value), message|
        expect { doc.root[name] = value }.to raise_error(Makiri::Error, message)
      end
    end
  end

  describe "Document.new (build from scratch)" do
    it "creates an empty document and builds it up with root= / add_child" do
      d = Makiri::XML::Document.new
      expect(d).to be_a(Makiri::XML::Document)
      expect(d.root).to be_nil
      d.root = d.create_element("feed", "xmlns" => "urn:a")
      d.root.add_child(d.create_element("entry", "hi"))
      expect(d.to_xml).to include(%(<feed xmlns="urn:a"><entry>hi</entry></feed>))
      expect(d.root.name).to eq("feed")
    end

    it "root= replaces an existing root; the single-root rule still holds" do
      d = Makiri::XML::Document.new
      d.add_child(d.create_element("a"))
      expect { d.add_child(d.create_element("b")) }.to raise_error(Makiri::Error, /single root/)
      d.root = d.create_element("c") # replace, not a second root
      expect(d.root.name).to eq("c")
      expect(d.xpath("/c").length).to eq(1)
    end

    it "scopes root= to XML (an HTML5 document has a fixed structure)" do
      expect(Makiri::HTML("<p/>")).not_to respond_to(:root=)
      expect(Makiri::Document.instance_methods).not_to include(:root=)
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
