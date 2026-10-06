# frozen_string_literal: true

require "spec_helper"

# The API an HTML and an XML Document answer alike, for a caller building a
# DOM over both: createElementNS, the attribute version, "no namespace is nil",
# and the HTML Document's empty constructor and compat mode.
RSpec.describe "HTML and XML Document APIs" do
  XHTML = "http://www.w3.org/1999/xhtml"
  XMLNS = "http://www.w3.org/2000/xmlns/"

  documents = {
    "HTML" => -> { Makiri::HTML("<p>") },
    "XML" => -> { Makiri::XML::Document.new },
  }

  describe "Document#create_element_ns" do
    documents.each do |kind, make|
      context kind do
        let(:doc) { make.call }

        it "splits at the first colon and keeps the case" do
          el = doc.create_element_ns("urn:u", "a:B:c")
          expect([el.name, el.prefix, el.local_name, el.namespace_uri]).to eq(["a:B:c", "a", "B:c", "urn:u"])
          el = doc.create_element_ns(nil, "fooBar")
          expect([el.name, el.prefix, el.local_name, el.namespace_uri]).to eq(["fooBar", nil, "fooBar", nil])
        end

        it "takes the DOM's names, which are looser than XML's" do
          expect(doc.create_element_ns(nil, "f}oo").name).to eq("f}oo")
        end

        it "raises ArgumentError for a name the DOM refuses" do
          ["1bad", "a b", ":x", "x:", ""].each do |q|
            expect { doc.create_element_ns("urn:u", q) }.to raise_error(ArgumentError), q.inspect
          end
        end

        it "raises Makiri::Error for a namespace that does not fit the name" do
          [[nil, "p:x"], ["urn:u", "xml:x"], ["urn:u", "xmlns:x"], [XMLNS, "x"]].each do |ns, q|
            expect { doc.create_element_ns(ns, q) }.to raise_error(Makiri::Error, /does not fit/), q
          end
          expect(doc.create_element_ns(XMLNS, "xmlns:x").namespace_uri).to eq(XMLNS)
        end

        it "reads nil and an empty namespace alike as no namespace" do
          [nil, ""].each do |ns|
            expect(doc.create_element_ns(ns, "x").namespace_uri).to be_nil
          end
        end
      end
    end

    context "XML" do
      let(:doc) { Makiri::XML('<r xmlns:p="urn:other"/>') }

      it "keeps the namespace it was given wherever the element is inserted" do
        el = doc.create_element_ns("urn:u", "p:x")
        doc.root << el
        expect(el.namespace_uri).to eq("urn:u")
        expect(doc.root.to_xml).to eq(%(<r xmlns:p="urn:other"><p:x xmlns:p="urn:u"/></r>))
        bare = doc.create_element_ns(nil, "y")
        el << bare
        expect(bare.namespace_uri).to be_nil
      end

      # Namespaces in XML binds the XML namespace to `xml` and no other prefix,
      # and reserves the XMLNS namespace for declarations: the DOM makes such
      # elements, so the serializers write the one and refuse the other rather
      # than writing a declaration that does not parse.
      it "writes an element in the XML namespace as xml:local, and refuses one in XMLNS" do
        xml_ns = "http://www.w3.org/XML/1998/namespace"
        r = Makiri::XML('<r xmlns:p="urn:p"/>')
        %w[x xml:x p:x].each { |q| r.root << r.create_element_ns(xml_ns, q) }
        expect(r.root.to_xml).to eq(%(<r xmlns:p="urn:p"><xml:x/><xml:x/><xml:x/></r>))
        expect(r.canonicalize).to eq(%(<r xmlns:p="urn:p"><xml:x></xml:x><xml:x></xml:x><xml:x></xml:x></r>))
        expect(Makiri::XML(r.to_xml).root.children.map(&:namespace_uri).uniq).to eq([xml_ns])

        x = Makiri::XML::Document.new
        x.root = x.create_element_ns(XMLNS, "xmlns")
        expect { x.to_xml }.to raise_error(Makiri::Error, /XMLNS namespace/)
        expect { x.canonicalize }.to raise_error(Makiri::Error, /XMLNS namespace/)
      end

      it "makes an element XML cannot name, which to_xml then refuses" do
        doc.root << doc.create_element_ns(nil, "f}oo")
        expect { doc.to_xml }.to raise_error(Makiri::Error, /DOM-loose/)
      end

      # The DOM's createElement takes its argument as a local name whole, which
      # only create_loose_dom_element can make in an XML Document.
      it "leaves createElement's unsplit names to create_loose_dom_element" do
        %w[foo: f::oo xmlns:foo a:b].each do |n|
          el = doc.create_loose_dom_element(n, nil, n, nil)
          expect([el.name, el.prefix, el.local_name, el.namespace_uri]).to eq([n, nil, n, nil])
        end
        expect { doc.create_element_ns(nil, "foo:") }.to raise_error(ArgumentError)
        expect { doc.create_element("foo:") }.to raise_error(ArgumentError)
      end

      it "makes what create_loose_dom_element made" do
        made = doc.create_element_ns("urn:u", "a:b:c")
        loose = doc.create_loose_dom_element("a:b:c", "a", "b:c", "urn:u")
        expect([made.name, made.prefix, made.local_name, made.namespace_uri])
          .to eq([loose.name, loose.prefix, loose.local_name, loose.namespace_uri])
      end
    end
  end

  describe "Document#attribute_version" do
    def bumps(doc)
      before = [doc.attribute_version, doc.tree_version]
      yield
      expect(doc.attribute_version).to be > before[0]
      expect(doc.tree_version).to eq(before[1])
    end

    def keeps(doc)
      before = doc.attribute_version
      yield
      expect(doc.attribute_version).to eq(before)
    end

    it "is an Integer read without allocating" do
      doc = Makiri::HTML("<p>")
      expect(doc.attribute_version).to be_a(Integer)
      doc.attribute_version
      before = GC.stat(:total_allocated_objects)
      100.times { doc.attribute_version }
      expect(GC.stat(:total_allocated_objects) - before).to be < 10
    end

    context "HTML" do
      let(:doc) { Makiri::HTML(%(<div><p a="1">t</p></div>)) }
      let(:para) { doc.at_css("p") }

      it "grows with every attribute edit, and only with those" do
        bumps(doc) { para["b"] = "2" }
        bumps(doc) { para["b"] = "2" }
        bumps(doc) { para.set_loose_dom_attribute("v:x", "1") }
        bumps(doc) { para.set_attribute_ns("urn:k", "k:a", "1") }
        bumps(doc) { para.remove_attribute_ns("urn:k", "a") }
        bumps(doc) { para.delete("b") }
        bumps(doc) { para.attribute_nodes.first.content = "3" }
        bumps(doc) { doc.create_element("q")["x"] = "1" }
      end

      it "is left alone by child-list and character-data edits and by reads" do
        keeps(doc) do
          para << doc.create_element("b")
          para.child.content = "data"
          para.content = "x"
          para.remove
          doc.css("p")
          para["a"]
        end
      end
    end

    context "XML" do
      let(:doc) { Makiri::XML(%(<r xmlns:k="urn:k"><p a="1">t</p></r>)) }
      let(:para) { doc.root.at_xpath("p") }

      it "grows with every attribute edit, and only with those" do
        bumps(doc) { para["b"] = "2" }
        bumps(doc) { para["b"] = "2" }
        bumps(doc) { para.set_loose_dom_attribute("v:x", "1") }
        bumps(doc) { para.set_attribute_ns("urn:k", "k:c", "3") }
        bumps(doc) { para.remove_attribute_ns("urn:k", "c") }
        bumps(doc) { para.delete("b") }
        bumps(doc) { doc.create_element("q")["x"] = "1" }
        bumps(doc) { para.attribute_nodes.first.remove }
        expect(para["a"]).to be_nil
      end

      it "is left alone by child-list and character-data edits" do
        keeps(doc) do
          para << doc.create_element("b")
          para.child.content = "data"
          para.content = "x"
          para.remove
        end
      end
    end

    # The cache's promise: while the version stands still, every attribute is
    # as it was.
    {
      "HTML" => -> { Makiri::HTML("<div a=1><p b=2>a</p><p>b</p></div>") },
      "XML" => -> { Makiri::XML("<r a='1'><p b='2'>a</p><p>b</p></r>") },
    }.each do |kind, make|
      it "an unchanged version means unchanged attributes (#{kind})" do
        rng = Random.new(20_261_003)
        doc = make.call
        nodes = [doc.root, *doc.root.children.select(&:element?)]
        snapshot = -> { nodes.map { |n| n.attribute_nodes.map { [_1.name, _1.value] } } }
        300.times do
          before = [doc.attribute_version, snapshot.call]
          n = nodes.sample(random: rng)
          case rng.rand(5)
          when 0 then n["a#{rng.rand(3)}"] = "v#{rng.rand(2)}"
          when 1 then n.delete("a#{rng.rand(3)}")
          when 2 then n.set_loose_dom_attribute("x:#{rng.rand(2)}", "v")
          when 3 then n << doc.create_element("c")
          when 4 then n.content = "t"
          end
          expect(doc.attribute_version).to be > before[0] if snapshot.call != before[1]
        end
      end
    end
  end

  # No namespace and no prefix read as nil, never as "".
  describe "namespace_uri and prefix without a namespace" do
    it "are nil on elements and attributes, however they were made" do
      xml = Makiri::XML(%(<r xmlns:p="u" a="1"><b xmlns=""/></r>))
      html = Makiri::HTML("<p>")
      html.at_css("p")["a"] = "1"
      els = [xml.root.children.first, xml.create_element_ns("", "x"), html.create_element_ns("", "x"),
             xml.create_element("x"),]
      [xml.root, html.at_css("p")].each do |e|
        e.set_attribute_ns("", "c", "1")
        e.set_attribute_ns(nil, "d", "1")
      end
      attrs = [xml.root, html.at_css("p")].flat_map { |e| e.attribute_nodes.reject { _1.name.start_with?("xmlns") } }
      expect(attrs.size).to be >= 5
      expect((els + attrs).map { [_1.namespace_uri, _1.prefix] }.uniq).to eq([[nil, nil]])
      expect([xml.root.prefix, html.at_css("p").prefix]).to eq([nil, nil])
    end
  end

  describe "Makiri::HTML::Document.new" do
    let(:doc) { Makiri::HTML::Document.new }

    it "is an empty HTML document in no-quirks mode" do
      expect(doc).to be_a(Makiri::HTML::Document)
      expect(doc.children.size).to eq(0)
      expect(doc.root).to be_nil
      expect([doc.quirks_mode, doc.quirks_mode?, doc.compat_mode]).to eq([0, false, "CSS1Compat"])
      expect([doc.to_html, doc.text, doc.title, doc.body]).to eq(["", "", "", nil])
    end

    it "builds into a document that queries and serializes like a parsed one" do
      doc << doc.create_document_type("html")
      html = doc.create_element("html")
      doc << html
      html << doc.create_element("body")
      doc.body.inner_html = %(<p class="a">x</p>)
      expect(doc.to_html).to eq(%(<!DOCTYPE html><html><body><p class="a">x</p></body></html>))
      expect(doc.css("p.a").size).to eq(1)
      expect(doc.at_xpath("//p").text).to eq("x")
      expect(doc.at_css("p").line).to be_nil
    end

    it "takes no title or charset while it has no root, and dups as empty" do
      doc.title = "t"
      doc.meta_encoding = "utf-8"
      expect(doc.children.size).to eq(0)
      expect(doc.dup.children.size).to eq(0)
    end

    # The DOM's title setter does nothing while the document has no head;
    # neither setter puts its element straight under the root.
    it "takes no title or charset while it has a root but no head" do
      doc << doc.create_element("html")
      doc.title = "t"
      doc.meta_encoding = "utf-8"
      expect(doc.to_html).to eq("<html></html>")
      doc.root << doc.create_element("head")
      doc.title = "t"
      doc.meta_encoding = "utf-8"
      expect(doc.to_html).to eq(%(<html><head><title>t</title><meta charset="utf-8"></head></html>))
    end

    # Lexbor's own document-root lookup falls back to the first child when
    # there is no <html>; the DOM's documentElement is the element child.
    it "answers root with its element child, never another node" do
      doc << doc.create_comment("c")
      expect(doc.root).to be_nil
      doc.title = "t"
      doc.meta_encoding = "utf-8"
      expect(doc.to_html).to eq("<!--c-->")

      svg = Makiri::HTML::Document.new
      svg << svg.create_document_type("html")
      svg << svg.create_element_ns("http://www.w3.org/2000/svg", "svg")
      expect([svg.root.name, svg.at_css(":root").name]).to eq(%w[svg svg])
    end

    # A re-parse of to_html would wrap a root-less tree in html/head/body and
    # put it in quirks mode; dup copies the tree as it is.
    it "dups a tree with no html root as it is" do
      doc << doc.create_comment("c")
      doc << doc.create_element("div")
      copy = doc.dup
      expect([copy.to_html, copy.quirks_mode]).to eq(["<!--c--><div></div>", 0])
    end
  end

  describe "HTML Document#dup" do
    it "copies the tree, template contents and quirks mode, sharing no node" do
      {
        "<p>x<template><b>t</b></template>" => 1,
        "<!DOCTYPE html><p>x</p>" => 0,
        '<!DOCTYPE html PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN" "http://www.w3.org/TR/html4/loose.dtd"><p>' => 2,
      }.each do |source, mode|
        doc = Makiri::HTML(source)
        copy = doc.dup
        expect([copy.to_html, copy.quirks_mode]).to eq([doc.to_html, mode])
        expect(copy.at_css("p")).not_to eql(doc.at_css("p"))
        copy.at_css("p")["x"] = "1"
        expect(doc.at_css("p")["x"]).to be_nil
      end
      tpl = Makiri::HTML("<template><b>t</b></template>").dup.at_css("template")
      expect(tpl.content_fragment.children.map(&:name)).to eq(["b"])
    end

    it "has no source lines: its nodes were not parsed" do
      expect(Makiri::HTML("<p>x</p>").dup.at_css("p").line).to be_nil
    end

    it "survives a GC.compact with live wrappers" do
      docs = Array.new(20) { Makiri::HTML::Document.new.tap { _1 << _1.create_element("html") } }
      GC.start
      GC.compact if GC.respond_to?(:compact)
      expect(docs.map { _1.root.name }.uniq).to eq(["html"])
    end
  end

  describe "HTML Document#quirks_mode? and #compat_mode" do
    {
      "<p>" => [1, true, "BackCompat"],
      "<!DOCTYPE html>" => [0, false, "CSS1Compat"],
      '<!DOCTYPE html PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN" "http://www.w3.org/TR/html4/loose.dtd">' =>
        [2, false, "CSS1Compat"],
    }.each do |source, expected|
      it "reads #{expected.last} for #{source[0, 30]}" do
        doc = Makiri::HTML(source)
        expect([doc.quirks_mode, doc.quirks_mode?, doc.compat_mode]).to eq(expected)
      end
    end
  end

  describe "XML Document#quirks_mode? and #compat_mode" do
    it "is never in quirks mode" do
      [Makiri::XML::Document.new, Makiri::XML("<r/>")].each do |doc|
        expect([doc.quirks_mode?, doc.compat_mode]).to eq([false, "CSS1Compat"])
      end
    end
  end

  # The DOM's createElement over an HTML-backed document whose type is not
  # HTML: the local name whole, its case kept, in the namespace given.
  describe "HTML Document#create_loose_dom_element" do
    let(:doc) { Makiri::HTML("<!DOCTYPE html><body></body>") }
    let(:html_ns) { "http://www.w3.org/1999/xhtml" }

    it "keeps a local name's colons and case" do
      [[%w[foo: foo:], nil], [%w[f::oo f::oo], nil], [%w[xmlns:foo xmlns:foo], nil],
       [%w[Foo:Bar Foo:Bar], nil], [%w[f::oo f::oo], "urn:u"], [%w[Foo:Bar Foo:Bar], html_ns]].each do |(q, local), ns|
        el = doc.create_loose_dom_element(q, nil, local, ns)
        expect([el.name, el.prefix, el.local_name, el.namespace_uri]).to eq([q, nil, local, ns])
      end
    end

    it "makes what create_element_ns makes for a split name" do
      made = doc.create_element_ns("urn:u", "a:b:c")
      loose = doc.create_loose_dom_element("a:b:c", "a", "b:c", "urn:u")
      expect([loose.name, loose.prefix, loose.local_name, loose.namespace_uri])
        .to eq([made.name, made.prefix, made.local_name, made.namespace_uri])
      expect([loose.name, loose.prefix, loose.local_name]).to eq(%w[a:b:c a b:c])
    end

    it "keeps an upper-case HTML-namespace name its own element" do
      el = doc.create_loose_dom_element("BR", nil, "BR", html_ns)
      doc.body << el
      expect(el.local_name).to eq("BR")
      expect(doc.body.inner_html).to eq("<BR></BR>")
    end

    it "is found by CSS and XPath and serialized as written" do
      el = doc.create_loose_dom_element("f::oo", nil, "f::oo", nil)
      el["id"] = "x"
      doc.body << el
      expect(doc.at_css("#x")).to eq(el)
      expect(doc.at_xpath("//*[local-name() = 'f::oo']")).to eq(el)
      expect(doc.body.inner_html).to eq(%(<f::oo id="x"></f::oo>))
    end

    it "refuses names the DOM refuses, or a split that is not the name, with ArgumentError" do
      [["1bad", nil, "1bad"], ["a b", nil, "a b"], ["a:b", nil, "b"], ["a:b", "a", "c"],
       ["p q:b", "p q", "b"]].each do |q, p, l|
        expect { doc.create_loose_dom_element(q, p, l, nil) }.to raise_error(ArgumentError)
      end
    end

    it "agrees with the XML Document's" do
      xml = Makiri::XML::Document.new
      [["foo:", nil, "foo:", nil], ["p:Q", "p", "Q", "urn:u"], ["xmlns:foo", nil, "xmlns:foo", nil]].each do |args|
        h = doc.create_loose_dom_element(*args)
        x = xml.create_loose_dom_element(*args)
        expect([h.name, h.prefix, h.local_name, h.namespace_uri])
          .to eq([x.name, x.prefix, x.local_name, x.namespace_uri])
      end
    end
  end
end

# The DOM's createProcessingInstruction refuses data holding `?>` with
# InvalidCharacterError, which Makiri words as ArgumentError on both
# representations; a target XML reserves is made on both.
RSpec.describe "Document#create_processing_instruction, HTML and XML alike" do
  { "HTML" => -> { Makiri::HTML::Document.new }, "XML" => -> { Makiri::XML::Document.new } }.each do |kind, make|
    it "#{kind}: refuses ?> in the data with ArgumentError and makes an xml target" do
      doc = make.call
      expect { doc.create_processing_instruction("t", "a?>b") }
        .to raise_error(ArgumentError, /must not contain \?>/)
      expect(doc.create_processing_instruction("xml", "d").name).to eq("xml")
    end
  end

  # Importing an HTML xml-target processing instruction into XML holds it,
  # and to_xml refuses it rather than the import.
  it "imports an HTML xml-target processing instruction into XML" do
    hdoc = Makiri::HTML("<div></div>")
    html = hdoc.at_css("div")
    html << hdoc.create_processing_instruction("xml", %(version="1.0"))
    xml = Makiri::XML("<r/>")
    xml.root << xml.import_node(html, true)
    expect(xml.root.children.first.children.first.name).to eq("xml")
    expect { xml.to_xml }.to raise_error(Makiri::Error, /target is xml/)
  end
end
