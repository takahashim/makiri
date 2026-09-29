# frozen_string_literal: true

# The DOM's attribute algorithms over Lexbor (`lexbor/adapter/html/attrs.rs`).
#
# Lexbor's own element helpers match attributes by LOCAL name and, on append,
# DESTROY the attribute behind the element's `id` / `class` shortcut when
# another attribute with that local name arrives. Makiri detaches, never
# destroys: an Attr wrapper must keep reading its own attribute, and a by-name
# set must not land on a namespaced attribute that shares the local name.
RSpec.describe "HTML attribute DOM algorithms" do
  XHTML = "http://www.w3.org/1999/xhtml" unless defined?(XHTML)
  XLINK = "http://www.w3.org/1999/xlink" unless defined?(XLINK)

  def attrs_of(el)
    el.attribute_nodes.map { |a| [a.name, a.namespace_uri, a.value] }
  end

  describe "an Attr wrapper survives an attribute with the same Lexbor shortcut" do
    [
      ["setAttributeNS(nil, 'ID')", "id", ->(el, i) { el.set_attribute_ns(nil, "ID", "v#{i}") }],
      ["setAttributeNS('urn:x', 'id')", "id", ->(el, i) { el.set_attribute_ns("urn:x", "id", "v#{i}") }],
      ["setAttributeNS('urn:x', 'p:id')", "id", ->(el, i) { el.set_attribute_ns("urn:x", "p:id", "v#{i}") }],
      ["setAttributeNS(XHTML, 'class')", "class", ->(el, i) { el.set_attribute_ns(XHTML, "class", "v#{i}") }]
    ].each do |label, name, set|
      it "keeps the original #{name} attribute readable after #{label}" do
        300.times do |i|
          doc = Makiri::HTML(%(<div id="orig-id" class="orig-class"><p id="other" class="x">t</p></div>))
          div = doc.at_css("div")
          held = div.xpath("@#{name}").first
          set.call(div, i)
          # churn the arena so a freed attribute's memory gets reused
          doc.at_css("p")["data-#{i}"] = "z" * (i % 40)
          GC.start if (i % 50).zero?
          expect(held.name).to eq(name)
          expect(held.value).to eq("orig-#{name}")
          expect(held.parent).to eq(div)
          expect(div.attribute_nodes.map(&:name)).to include(name)
          expect(div[name]).to eq("orig-#{name}")
        end
      end
    end
  end

  describe "setAttribute is keyed on the qualified name" do
    it "adds a plain href beside xlink:href instead of overwriting it" do
      doc = Makiri::HTML('<svg><a xlink:href="old"></a></svg>')
      a = doc.css("a").first
      a["href"] = "new"
      expect(attrs_of(a)).to eq([["xlink:href", XLINK, "old"], ["href", nil, "new"]])
      expect(a["href"]).to eq("new")
      expect(a["xlink:href"]).to eq("old")
    end

    it "removes by qualified name, leaving the namespaced attribute" do
      doc = Makiri::HTML('<svg><a xlink:href="old" href="plain"></a></svg>')
      a = doc.css("a").first
      a.delete("href")
      expect(attrs_of(a)).to eq([["xlink:href", XLINK, "old"]])
    end
  end

  describe "cross-kind import (XML -> HTML) keeps every attribute" do
    let(:html) { Makiri::HTML("<body></body>") }

    it "keeps an id beside a namespaced id" do
      xml = Makiri::XML("<e/>")
      e = xml.root
      e["id"] = "plain"
      e.set_attribute_ns("urn:x", "id", "namespaced")
      imp = html.import_node(e, true)
      expect(attrs_of(imp)).to contain_exactly(["id", nil, "plain"], ["id", "urn:x", "namespaced"])
      html.at_css("body") << imp
      expect(html.css("#plain").size).to eq(1)
      expect(html.css("#namespaced").size).to eq(0)
    end

    it "keeps a plain href beside xlink:href" do
      xml = Makiri::XML("<e/>")
      e = xml.root
      e.set_attribute_ns(XLINK, "href", "link")
      e.set_attribute_ns(nil, "href", "plain")
      imp = html.import_node(e, true)
      expect(attrs_of(imp)).to contain_exactly(["href", nil, "plain"], ["href", XLINK, "link"])
    end

    it "keeps prefixed and plain attributes of a parsed element" do
      xml = Makiri::XML(%(<e xmlns:x="urn:x" xmlns:xl="#{XLINK}" id="1" x:id="2" xl:href="h" href="p"/>))
      imp = html.import_node(xml.root, true)
      expect(attrs_of(imp).reject { |n, _| n.start_with?("xmlns") }).to contain_exactly(
        ["id", nil, "1"], ["x:id", "urn:x", "2"], ["xl:href", XLINK, "h"], ["href", nil, "p"]
      )
    end
  end

  describe "a copy (clone / import) keeps attributes that share a Lexbor shortcut" do
    it "keeps id and a namespaced unprefixed id through dup" do
      doc = Makiri::HTML('<div id="a"></div>')
      div = doc.at_css("div")
      div.set_attribute_ns("urn:x", "id", "b")
      copy = div.dup
      expect(attrs_of(copy)).to eq([["id", nil, "a"], ["id", "urn:x", "b"]])
      doc.at_css("body") << copy
      expect(doc.css("#a").size).to eq(2)
      expect(doc.css("#b").size).to eq(0)
    end

    it "keeps them through import into another document" do
      src = Makiri::HTML('<div class="k"></div>')
      div = src.at_css("div")
      div.set_attribute_ns(XHTML, "p:class", "ns")
      div.set_attribute_ns(nil, "CLASS", "upper")
      dst = Makiri::HTML("<body></body>")
      imp = dst.import_node(div, true)
      expect(imp.attribute_nodes.map(&:name)).to eq(%w[class p:class CLASS])
      dst.at_css("body") << imp
      expect(dst.css(".k").size).to eq(1)
      expect(dst.css(".ns, .upper").size).to eq(0)
    end
  end

  describe "#id / .class matching follows the DOM's ID and class attributes" do
    it "moves #id with set_attribute_ns(nil, 'id')" do
      doc = Makiri::HTML('<div id="a"></div>')
      div = doc.at_css("div")
      div.set_attribute_ns(nil, "id", "b")
      expect(doc.css("#a")).to be_empty
      expect(doc.css("#b").to_a).to eq([div])
    end

    it "ignores a namespaced p:id or an ID-spelled attribute" do
      doc = Makiri::HTML('<div id="a"></div>')
      div = doc.at_css("div")
      div.set_attribute_ns("urn:x", "p:id", "z")
      div.set_attribute_ns(nil, "ID", "u")
      div.set_attribute_ns("urn:x", "id", "w")
      expect(doc.css("#a").to_a).to eq([div])
      expect(doc.css("#z, #u, #w")).to be_empty
    end

    # The DOM's ID is the no-namespace `id`, so an unprefixed `id` set in a
    # namespace is not one - even when it is the only attribute named `id`,
    # and though `[id=w]` (by qualified name) still finds it.
    it "does not take a namespaced unprefixed id as the ID when it is the only one" do
      doc = Makiri::HTML("<div></div><p class='x'></p>")
      div = doc.at_css("div")
      div.set_attribute_ns("urn:x", "id", "w")
      p = doc.at_css("p")
      p.set_attribute_ns(XHTML, "class", "k")
      p.delete("class")
      expect(doc.css("#w")).to be_empty
      expect(div.matches?("#w")).to be(false)
      expect(doc.css("[id=w]").to_a).to eq([div])
      expect(doc.css(".k")).to be_empty
      expect(doc.css("[class=k]").to_a).to eq([p])
    end

    it "ignores a class set in the XHTML namespace" do
      doc = Makiri::HTML('<div class="c"></div>')
      div = doc.at_css("div")
      div.set_attribute_ns(XHTML, "class", "k")
      expect(doc.css(".c").to_a).to eq([div])
      expect(doc.css(".k")).to be_empty
    end

    it "drops #id when the ID attribute is removed, and gains it when set again" do
      doc = Makiri::HTML('<div id="a"></div>')
      div = doc.at_css("div")
      held = div.xpath("@id").first
      div.delete("id")
      expect(doc.css("#a")).to be_empty
      expect(held.value).to eq("a")
      div["id"] = "c"
      expect(doc.css("#c").to_a).to eq([div])
    end
  end
end
