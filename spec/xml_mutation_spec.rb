# frozen_string_literal: true

require "spec_helper"

# Phase 1 XML mutation: in-place edits over the custom node arena
# (remove/unlink, []=, delete, content=, name=). Detach-never-destroy, strict
# (validated names + XML-Char values), and every edit is reflected by both
# serialization and the native XPath engine.
RSpec.describe "Makiri::XML mutation (Phase 1)" do
  describe "#[]= (set / replace an attribute)" do
    let(:doc) { Makiri::XML(%(<r xmlns:p="urn:p"><a id="1"/></r>)) }
    let(:a)   { doc.at_xpath("//a") }

    it "adds a new attribute and replaces an existing one by name" do
      a["class"] = "lead"
      expect(a["class"]).to eq("lead")
      a["id"] = "2"
      expect(a["id"]).to eq("2")
      expect(a.attribute_nodes.length).to eq(2) # replaced in place, not duplicated
      expect(a.to_xml).to eq(%(<a id="2" class="lead"/>))
    end

    it "refuses a non-String value rather than converting it" do
      expect { a["n"] = 42 }.to raise_error(TypeError)
      expect(a.key?("n")).to be(false)
    end

    it "escapes special characters on serialization (stored verbatim)" do
      a["t"] = %(x<y&"z)
      expect(a["t"]).to eq(%(x<y&"z))
      expect(a.to_xml).to eq(%(<a id="1" t="x&lt;y&amp;&quot;z"/>))
    end

    it "resolves a prefixed attribute's namespace against in-scope declarations" do
      a["p:k"] = "v"
      node = a.attribute_nodes.find { |n| n.name == "p:k" }
      expect(node.namespace_uri).to eq("urn:p")
    end

    it "adds an xmlns declaration that then binds a prefix for later edits" do
      a["xmlns:q"] = "urn:q"
      defn = a.namespace_definitions.find { |n| n.prefix == "q" }
      expect(defn.href).to eq("urn:q")
      a["q:m"] = "1"
      expect(a.attribute_nodes.find { |n| n.name == "q:m" }.namespace_uri).to eq("urn:q")
    end

    it "fails closed on an unbound prefix and a bad name, and holds a non-XML-Char value" do
      expect { a["z:k"] = "v" }.to raise_error(Makiri::Error, /not bound/)
      expect { a["1bad"] = "v" }.to raise_error(ArgumentError)
      a["k"] = "bad\u0001char" # held, as the DOM holds it; to_xml refuses it
      expect { a.document.to_xml }.to raise_error(Makiri::Error, /character data XML cannot hold/)
    end

    it "raises when setting an attribute on a non-element node" do
      d = Makiri::XML("<r>text</r>")
      t = d.at_xpath("//r/text()")
      expect { t["x"] = "y" }.to raise_error(Makiri::Error)
    end
  end

  describe "#delete / #remove_attribute" do
    let(:doc) { Makiri::XML(%(<r a="1" b="2"/>)) }

    it "removes the named attribute and is a no-op when absent" do
      r = doc.root
      r.delete("a")
      expect(r["a"]).to be_nil
      expect(r["b"]).to eq("2")
      expect(r.delete("nope")).to eq(r) # no-op, chainable
    end
  end

  describe "#content=" do
    it "replaces an element's children with a single text node" do
      doc = Makiri::XML("<r><a/><b>x</b>tail</r>")
      doc.root.content = "only text"
      expect(doc.root.children.length).to eq(1)
      expect(doc.root.text).to eq("only text")
      expect(doc.root.to_xml).to eq("<r>only text</r>")
    end

    it "empties an element when set to an empty string" do
      doc = Makiri::XML("<r><a/></r>")
      doc.root.content = ""
      expect(doc.root.children).to be_empty
      expect(doc.root.to_xml).to eq("<r/>")
    end

    it "stores text verbatim and escapes it on serialization" do
      doc = Makiri::XML("<r/>")
      doc.root.content = "a < b & c"
      expect(doc.root.text).to eq("a < b & c")
      expect(doc.root.to_xml).to eq("<r>a &lt; b &amp; c</r>")
    end

    it "sets a leaf node's data (text / comment)" do
      doc = Makiri::XML("<r>old<!--c--></r>")
      doc.at_xpath("//r/text()").content = "new"
      doc.at_xpath("//r/comment()").content = " z "
      expect(doc.root.to_xml).to eq("<r>new<!-- z --></r>")
    end

    it "holds a NUL or a non-XML-Char value, which to_xml refuses" do
      doc = Makiri::XML("<r/>")
      doc.root.content = "x\u0000y" # the DOM holds it, as HTML does
      expect { doc.to_xml }.to raise_error(Makiri::Error, /character data XML cannot hold/)
      doc.root.content = "x\u0001y" # the DOM holds it
      expect { doc.to_xml }.to raise_error(Makiri::Error, /character data XML cannot hold/)
    end
  end

  describe "#remove / #unlink" do
    it "detaches a node from the tree (reflected by serialization and XPath)" do
      doc = Makiri::XML("<r><a/><b/><c/></r>")
      doc.at_xpath("//b").remove
      expect(doc.root.to_xml).to eq("<r><a/><c/></r>")
      expect(doc.xpath("//b").length).to eq(0)
      expect(doc.xpath("//r/*").length).to eq(2)
    end

    it "keeps a removed node usable (detach-never-destroy)" do
      doc = Makiri::XML("<r><a>x</a></r>")
      a = doc.at_xpath("//a")
      a.remove
      expect(a.text).to eq("x")     # the wrapper still aliases live arena memory
      expect(a.parent).to be_nil
    end

    it "can remove the root element" do
      doc = Makiri::XML("<r><a/></r>")
      doc.root.remove
      expect(doc.to_xml).to start_with(%(<?xml version="1.0"?>))
      expect(doc.xpath("//r").length).to eq(0)
    end

    it "refuses to remove the document node" do
      expect { Makiri::XML("<r/>").remove }.to raise_error(Makiri::Error)
    end
  end

  describe "round-trips through #to_xml after editing" do
    it "re-parses to an equivalent tree" do
      doc = Makiri::XML(%(<feed xmlns="urn:a"><e id="1"/></feed>))
      e = doc.at_xpath("//x:e", "x" => "urn:a")
      e["id"] = "9"
      e.content = "hi"
      out = doc.to_xml
      reparsed = Makiri::XML(out)
      n = reparsed.at_xpath("//x:e", "x" => "urn:a")
      expect(n["id"]).to eq("9")
      expect(n.text).to eq("hi")
      expect(n.namespace_uri).to eq("urn:a")
    end
  end

  describe "#element_children" do
    let(:doc) { Makiri::XML(%(<r xmlns:p="urn:p"><p:a id="1"><b/>t<c/><!--x--><![CDATA[d]]></p:a></r>)) }

    it "returns only the child element nodes, in document order, as a NodeSet" do
      a = doc.at_xpath("//*[local-name()='a']")
      ec = a.element_children
      expect(ec).to be_a(Makiri::NodeSet)
      expect(ec.map(&:name)).to eq(%w[b c]) # text / comment / CDATA excluded
      expect(doc.root.element_children.map(&:name)).to eq(["p:a"])
    end

    it "is an empty NodeSet for a leaf element" do
      leaf = doc.create_element("Leaf")
      expect(leaf.element_children).to be_a(Makiri::NodeSet)
      expect(leaf.element_children).to be_empty
    end
  end

  describe "#clone_node" do
    let(:src) do
      Makiri::XML(%(<R xmlns:p="urn:p"><p:Child Attr="v">hi<![CDATA[c<d]]></p:Child></R>)).root
    end

    it "deep-clones, preserving name case, namespaces, attributes and the CDATA type" do
      dup = src.clone_node(true)
      expect(dup.name).to eq("R")
      expect(dup.parent).to be_nil # detached
      child = dup.element_children.first
      expect([child.name, child.local_name, child.namespace_uri]).to eq(["p:Child", "Child", "urn:p"])
      expect(child.attribute_nodes.map { |x| [x.name, x.value] }).to eq([["Attr", "v"]])
      kinds = child.children.map { |n| n.cdata? ? :cdata : (n.text? ? :text : n.name) }
      expect(kinds).to eq(%i[text cdata])
    end

    it "shallow-clones (deep = false): keeps attributes, drops children" do
      child = src.element_children.first
      sh = child.clone_node(false)
      expect(sh.name).to eq("p:Child")
      expect(sh.children).to be_empty
      expect(sh.attribute_nodes.map(&:name)).to eq(["Attr"])
    end

    it "is an independent copy (editing the clone does not touch the original)" do
      dup = src.clone_node(true)
      dup.element_children.first["Attr"] = "CHANGED"
      expect(src.element_children.first["Attr"]).to eq("v")
    end

    it "backs Node#dup / #clone for XML nodes (deep by default, level 0 = shallow)" do
      expect(src.dup.name).to eq("R")
      expect(src.dup.element_children.first.name).to eq("p:Child") # deep
      expect(src.dup(0).children).to be_empty                       # shallow
    end
  end

  describe "a node placed beside itself" do
    # The guard ORDER differs between the verbs and is load-bearing: the sibling
    # verbs answer "no-op" before asking for a parent, replace asks first - so
    # replacing a DETACHED node by itself is a hierarchy error while inserting a
    # detached node beside itself is not. A refactor that unified the guards
    # silently changed this and nothing caught it.
    it "is a no-op for the sibling verbs, attached or not" do
      doc = Makiri::XML("<r><a/></r>")
      attached = doc.root.children.first
      detached = doc.create_element("d")
      [attached, detached].each do |n|
        expect { n.add_previous_sibling(n) }.not_to raise_error
        expect { n.add_next_sibling(n) }.not_to raise_error
      end
      expect(doc.root.to_xml).to eq("<r><a/></r>")
    end

    it "is a no-op for replace when attached, and a hierarchy error when not" do
      doc = Makiri::XML("<r><a/></r>")
      attached = doc.root.children.first
      expect { attached.replace(attached) }.not_to raise_error
      expect(doc.root.to_xml).to eq("<r><a/></r>")

      detached = doc.create_element("d")
      expect { detached.replace(detached) }.to raise_error(Makiri::Error)
    end
  end

  describe "moving a node to a position it already holds" do
    # `a.add_next_sibling(b)` where b ALREADY follows a. Site::after used to be
    # built as Site::before(next(a)) - which is b itself - and insert_at detaches
    # the incoming node before reading its neighbours, so the anchor left the
    # chain and b was spliced before ITSELF: b.next == b, a self-referential ring
    # that every later walk loops on forever. An anchor on the TARGET survives
    # the detach, because the target is not the node that moves.
    def chain(doc)
      names = []
      n = doc.root.child
      while n && names.length < 8
        names << n.name
        n = n.next
      end
      names
    end

    it "is a no-op for the next sibling, not a sibling ring" do
      doc = Makiri::XML("<r><a/><b/></r>")
      k = doc.root.children
      k[0].add_next_sibling(k[1])
      expect(chain(doc)).to eq(%w[a b])
      expect(doc.root.to_xml).to eq("<r><a/><b/></r>")
    end

    it "is a no-op for the previous sibling too" do
      doc = Makiri::XML("<r><a/><b/></r>")
      k = doc.root.children
      k[1].add_previous_sibling(k[0])
      expect(chain(doc)).to eq(%w[a b])
    end

    it "still moves a node that is NOT already in that position" do
      doc = Makiri::XML("<r><a/><b/><c/></r>")
      k = doc.root.children
      k[0].add_next_sibling(k[2])
      expect(chain(doc)).to eq(%w[a c b])
    end
  end

  # The parser holds a document to Namespaces in XML §3; the mutators used to
  # check only `xmlns:p=""`, so each of these wrote a tree to_xml could not
  # re-read.
  describe "the namespace rules the parser enforces" do
    xml_ns = "http://www.w3.org/XML/1998/namespace"

    {
      "xmlns:q" => xml_ns, "xmlns:xml" => "urn:other", "xmlns:xmlns" => "urn:x",
      "xmlns" => "http://www.w3.org/2000/xmlns/", "xmlns:p" => ""
    }.each do |name, uri|
      it "refuses #{name}=#{uri.inspect}" do
        doc = Makiri::XML("<r/>")
        expect { doc.root[name] = uri }.to raise_error(Makiri::Error, /namespace declaration not permitted/)
        expect(Makiri::XML(doc.to_xml).root.attribute_nodes).to be_empty
      end
    end

    it "still accepts xml bound to its own URI" do
      doc = Makiri::XML("<r/>")
      doc.root["xmlns:xml"] = xml_ns
      expect { Makiri::XML(doc.to_xml) }.not_to raise_error
    end

    it "refuses a second attribute with the same namespace and local name" do
      doc = Makiri::XML(%(<r xmlns:p="urn:p" xmlns:q="urn:p" p:a="1"/>))
      expect { doc.root["q:a"] = "2" }.to raise_error(Makiri::Error, /already has an attribute/)
    end

    # []= on an existing attribute re-derived its namespace from the scope, so
    # an attribute given urn:a could land on another's key and the output did
    # not parse. It changes the value alone now, as setAttribute does.
    it "changes only the value of an existing attribute, keeping its namespace" do
      doc = Makiri::XML(%(<r xmlns:q="urn:b" xmlns:p="urn:b"><e p:x="2"/></r>))
      e = doc.at_xpath("//e")
      e.set_attribute_ns("urn:a", "q:x", "1")
      e["q:x"] = "3"
      expect(e.attribute_nodes.map { |a| [a.name, a.namespace_uri, a.value] })
        .to eq([["p:x", "urn:b", "2"], ["q:x", "urn:a", "3"]])
      expect { Makiri::XML(doc.to_xml) }.not_to raise_error
    end
  end
end
