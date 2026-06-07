# frozen_string_literal: true

require "spec_helper"

# Makiri::XML::Builder - the Nokogiri-compatible construction DSL. It is pure
# Ruby over the public factories (Document.new / create_element / create_* /
# add_child), so these specs pin the DSL semantics, not the underlying mutation
# (covered by xml_build_spec.rb).
RSpec.describe Makiri::XML::Builder do
  describe "construction forms" do
    it "yields the builder when the block takes an argument" do
      b = described_class.new do |xml|
        xml.root do
          xml.child("hi")
        end
      end
      expect(b.to_xml).to include("<root><child>hi</child></root>")
    end

    it "instance_evals the block when it takes no argument" do
      b = described_class.new do
        root do
          child "hi"
        end
      end
      expect(b.to_xml).to include("<root><child>hi</child></root>")
    end

    it "emits a bare XML declaration (a built document declares no encoding, like Nokogiri)" do
      b = described_class.new { |xml| xml.r }
      expect(b.to_xml).to start_with(%(<?xml version="1.0"?>))
      expect(b.to_xml).not_to include("encoding=")
      expect(b.to_xml).to include("<r/>")
    end
  end

  describe "elements, text and attributes" do
    it "treats a Hash argument as attributes and any other as text content" do
      b = described_class.new do |xml|
        xml.a("hello", href: "/x", id: "1")
      end
      expect(b.doc.root.to_xml).to eq(%(<a href="/x" id="1">hello</a>))
    end

    it "builds nested structure with a block" do
      b = described_class.new do |xml|
        xml.ul do
          xml.li("one")
          xml.li("two")
        end
      end
      expect(b.doc.root.to_xml).to eq("<ul><li>one</li><li>two</li></ul>")
    end

    it "strips a single trailing underscore (or bang) so reserved names work" do
      b = described_class.new do |xml|
        xml.widget do
          xml.id_("10")     # id_ -> <id>
          xml.text_("body") # text_ -> <text> (text is a builder helper)
        end
      end
      expect(b.doc.root.to_xml).to eq("<widget><id>10</id><text>body</text></widget>")
    end

    it "coerces non-string content with #to_s" do
      b = described_class.new { |xml| xml.n(42) }
      expect(b.doc.root.text).to eq("42")
    end
  end

  describe "text / cdata / comment helpers" do
    it "appends raw text, CDATA and comment nodes to the current parent" do
      b = described_class.new do |xml|
        xml.root do
          xml.text("plain ")
          xml.cdata("a < b")
          xml.comment(" note ")
        end
      end
      expect(b.doc.root.to_xml).to eq("<root>plain <![CDATA[a < b]]><!-- note --></root>")
    end
  end

  describe "namespaces" do
    it "declares namespaces via xmlns attributes and prefixes the next element with []" do
      b = described_class.new do |xml|
        xml.feed("xmlns" => "urn:a", "xmlns:dc" => "urn:dc") do
          xml.entry do
            xml.title("Hello")     # inherits the default namespace
            xml["dc"].id_("42")    # dc:id, bound by the in-scope xmlns:dc
          end
        end
      end

      entry = b.doc.at_xpath("//a:entry", "a" => "urn:a")
      expect(entry).not_to be_nil
      expect(entry.namespace_uri).to eq("urn:a")
      expect(entry.at_xpath("a:title", "a" => "urn:a").text).to eq("Hello")
      id = entry.at_xpath("dc:id", "dc" => "urn:dc")
      expect(id.namespace_uri).to eq("urn:dc")
      expect(id.text).to eq("42")
    end

    it "raises ArgumentError like Nokogiri when a selected prefix is not defined" do
      expect do
        described_class.new do |xml|
          xml.root do
            xml["zzz"].item   # zzz declared nowhere
          end
        end
      end.to raise_error(ArgumentError, "Namespace zzz has not been defined")
    end

    it "accepts a prefix declared on the element itself (self-declaring root)" do
      b = described_class.new do |xml|
        xml["foo"].root("xmlns:foo" => "bar") do
          xml["foo"].child
        end
      end
      expect(b.to_xml).to include(%(<foo:root xmlns:foo="bar"><foo:child/></foo:root>))
    end
  end

  describe ".with" do
    it "builds into an existing node using that node's document" do
      doc = Makiri::XML("<feed><entry/></feed>")
      entry = doc.at_xpath("//entry")
      described_class.with(entry) do |xml|
        xml.title("Hi")
        xml.summary("S")
      end
      expect(entry.to_xml).to eq("<entry><title>Hi</title><summary>S</summary></entry>")
      expect(entry.children.first.document).to equal(doc)
    end

    it "accepts the Nokogiri positional (options, root) constructor signature" do
      doc = Makiri::XML("<feed><entry/></feed>")
      entry = doc.at_xpath("//entry")
      described_class.new({}, entry) { |xml| xml.title("Hi") }
      expect(entry.to_xml).to eq("<entry><title>Hi</title></entry>")
    end

    it "leaves #parent at the node when .with is given no block (imperative use)" do
      doc = Makiri::XML("<feed><entry/></feed>")
      entry = doc.at_xpath("//entry")
      builder = described_class.with(entry)
      expect(builder.parent).to equal(entry)
      builder.title("Later")
      expect(entry.to_xml).to eq("<entry><title>Later</title></entry>")
    end
  end

  describe "accessors" do
    it "exposes the built document and restores #parent after the build" do
      b = described_class.new { |xml| xml.root { xml.child } }
      expect(b.doc).to be_a(Makiri::XML::Document)
      expect(b.parent).to equal(b.doc) # back at the document once the build is done
    end
  end

  describe "attribute short-cut chain (NodeBuilder)" do
    it "builds class/id terse: object.classy.thing!" do
      b = described_class.new do |xml|
        xml.root do
          xml.object.classy.thing!
        end
      end
      expect(b.doc.root.to_xml).to eq(%(<root><object class="classy" id="thing"/></root>))
    end

    it "appends plain names to class, = sets an attribute, ! sets id with content" do
      b = described_class.new do |xml|
        xml.root do
          xml.a.one.two          # class="one two"
          xml.b.href = "/x"      # href attribute
          xml.c.section!("body") # id="section", text content
        end
      end
      expect(b.doc.at_xpath("//a")["class"]).to eq("one two")
      expect(b.doc.at_xpath("//b")["href"]).to eq("/x")
      expect(b.doc.at_xpath("//c").to_xml).to eq(%(<c id="section">body</c>))
    end

    it "accepts a trailing Hash and a nested block on the chain" do
      b = described_class.new do |xml|
        xml.root do
          xml.item.flagged(role: "x") do
            xml.child
          end
        end
      end
      expect(b.doc.at_xpath("//item").to_xml)
        .to eq(%(<item class="flagged" role="x"><child/></item>))
    end

    it "reads and writes attributes via [] / []= on the returned builder" do
      b = described_class.new do |xml|
        xml.root do
          n = xml.obj
          n["id"] = "9"
          @seen = n["id"]
        end
      end
      expect(b.doc.at_xpath("//obj")["id"]).to eq("9")
    end
  end

  describe "raw XML append (#<<)" do
    it "parses a fragment and appends its children to the current parent" do
      b = described_class.new do |xml|
        xml.root do
          xml << %(<child a="1">txt</child><other/>)
        end
      end
      expect(b.doc.root.to_xml).to eq(%(<root><child a="1">txt</child><other/></root>))
    end

    it "resolves fragment names against the document's in-scope namespaces" do
      b = described_class.new do |xml|
        xml.feed("xmlns:dc" => "urn:dc") do
          xml << "<dc:id>7</dc:id>"
        end
      end
      expect(b.doc.at_xpath("//dc:id", "dc" => "urn:dc").text).to eq("7")
    end
  end

  describe "round-trip" do
    it "re-parses to the same namespaced structure" do
      b = described_class.new do |xml|
        xml.feed("xmlns" => "urn:a", "xmlns:dc" => "urn:dc") do
          xml.entry("dc:id" => "42") do # a namespaced attribute via the attrs hash
            xml.title("Hello")
          end
        end
      end

      re = Makiri::XML(b.to_xml)
      n = re.at_xpath("//a:entry", "a" => "urn:a")
      expect(n.namespace_uri).to eq("urn:a")
      expect(n.at_xpath("@dc:id", "dc" => "urn:dc").value).to eq("42")
      expect(n.at_xpath("a:title", "a" => "urn:a").text).to eq("Hello")
    end
  end
end
