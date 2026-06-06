# frozen_string_literal: true

require "spec_helper"

RSpec.describe "Makiri::XML#to_xml" do
  describe "Document#to_xml" do
    it "emits the UTF-8 XML declaration, then the document's nodes" do
      out = Makiri::XML("<r><a/></r>").to_xml
      expect(out).to start_with(%(<?xml version="1.0" encoding="UTF-8"?>\n))
      expect(out).to include("<r><a/></r>")
    end

    it "emits the DOCTYPE (SYSTEM / PUBLIC) when present" do
      sys = Makiri::XML(%(<!DOCTYPE r SYSTEM "r.dtd"><r/>)).to_xml
      expect(sys).to include(%(<!DOCTYPE r SYSTEM "r.dtd">))
      pub = Makiri::XML(%(<!DOCTYPE html PUBLIC "-//W3C//X" "x.dtd"><html/>)).to_xml
      expect(pub).to include(%(<!DOCTYPE html PUBLIC "-//W3C//X" "x.dtd">))
    end

    it "keeps prolog/epilog comments and PIs in document order" do
      out = Makiri::XML("<!--top--><r/><?after x?>").to_xml
      expect(out).to include("<!--top-->").and include("<r/>").and include("<?after x?>")
      expect(out.index("<!--top-->")).to be < out.index("<r/>")
      expect(out.index("<r/>")).to be < out.index("<?after x?>")
    end
  end

  describe "escaping" do
    it "escapes &, <, > in text and &, <, >, \" in attribute values" do
      doc = Makiri::XML(%(<r a="x&lt;y&amp;&quot;z">t&lt;e&amp;xt&gt;</r>))
      out = doc.root.to_xml
      expect(out).to eq(%(<r a="x&lt;y&amp;&quot;z">t&lt;e&amp;xt&gt;</r>))
    end

    it "escapes reference-derived whitespace in attribute values (round-trip safe)" do
      # &#10; is a newline in the value; it must serialize back as &#10;, not a
      # literal newline (which attribute-value normalization would fold to a space).
      doc = Makiri::XML(%(<r a="x&#10;y"/>))
      expect(doc.root["a"]).to eq("x\ny")
      expect(doc.root.to_xml).to include("&#10;")
      expect(Makiri::XML(doc.to_xml).root["a"]).to eq("x\ny")
    end

    it "round-trips CDATA and comments verbatim" do
      doc = Makiri::XML("<r><![CDATA[a < b]]><!-- c & d --></r>")
      expect(doc.root.to_xml).to eq("<r><![CDATA[a < b]]><!-- c & d --></r>")
    end
  end

  describe "node-level #to_xml (no declaration) and #to_s" do
    let(:doc) { Makiri::XML("<r xmlns:p='urn:a'><p:b c='1'>x</p:b></r>") }

    it "serializes just the subtree, preserving namespaces, without a declaration" do
      el = doc.at_xpath("//p:b", "p" => "urn:a")
      expect(el.to_xml).to eq(%(<p:b c="1">x</p:b>))
      expect(el.to_xml).not_to include("<?xml")
    end

    it "#to_s is an alias of #to_xml" do
      expect(doc.root.to_s).to eq(doc.root.to_xml)
    end
  end

  describe "pretty: true" do
    it "indents element-only content but leaves text-bearing elements inline" do
      out = Makiri::XML("<a><b><c>txt</c></b><d/></a>").root.to_xml(pretty: true)
      expect(out).to eq("<a>\n  <b>\n    <c>txt</c>\n  </b>\n  <d/>\n</a>")
    end

    it "honours an explicit indent width" do
      out = Makiri::XML("<a><b/></a>").root.to_xml(indent: 4)
      expect(out).to eq("<a>\n    <b/>\n</a>")
    end
  end

  describe "output encoding" do
    it "transcodes the output and names the encoding in a Document declaration" do
      doc = Makiri::XML("<r>日本</r>")
      sj = doc.to_xml(encoding: "Shift_JIS")
      expect(sj.encoding).to eq(Encoding::Shift_JIS)
      expect(sj.encode("UTF-8")).to include(%(encoding="Shift_JIS"))
      expect(Makiri::XML(sj).root.text).to eq("日本") # round-trips
    end

    it "emits a hex character reference for a character the encoding cannot hold" do
      out = Makiri::XML("<r>日本</r>").root.to_xml(encoding: "ISO-8859-1")
      expect(out.encode("UTF-8")).to eq("<r>&#x65E5;&#x672C;</r>")
    end

    it "accepts an Encoding object and rejects an unknown name" do
      expect(Makiri::XML("<r/>").to_xml(encoding: Encoding::UTF_8)).to include("UTF-8")
      expect { Makiri::XML("<r/>").to_xml(encoding: "no-such-encoding") }.to raise_error(ArgumentError)
    end
  end

  describe "#canonicalize (Inclusive Canonical XML 1.0)" do
    it "sorts attributes and namespaces, uses explicit end tags, escapes per C14N" do
      xml = %(<r xmlns:b="urn:b" xmlns:a="urn:a" z="1" a="2" b:k="3"><e/></r>)
      expect(Makiri::XML(xml).root.canonicalize)
        .to eq(%(<r xmlns:a="urn:a" xmlns:b="urn:b" a="2" z="1" b:k="3"><e></e></r>))
    end

    it "drops superfluous namespace declarations and renders CDATA as text" do
      xml = %(<a xmlns="urn:d"><b xmlns="urn:d"><![CDATA[x < y]]></b></a>)
      expect(Makiri::XML(xml).root.canonicalize)
        .to eq(%(<a xmlns="urn:d"><b>x &lt; y</b></a>)) # inner xmlns is superfluous
    end

    it "omits comments by default and includes them with comments: true" do
      xml = "<r>a<!-- c -->b</r>"
      expect(Makiri::XML(xml).root.canonicalize).to eq("<r>ab</r>")
      expect(Makiri::XML(xml).root.canonicalize(comments: true)).to eq("<r>a<!-- c -->b</r>")
    end

    it "applies the prolog/epilog newline rule at the document level" do
      doc = Makiri::XML("<?p x?><r/><?e y?>")
      expect(doc.canonicalize).to eq("<?p x?>\n<r></r>\n<?e y?>")
    end
  end

  describe "HTML serialization stays unsupported" do
    it "raises NotImplementedError for to_html / inner_html / outer_html" do
      n = Makiri::XML("<r/>").root
      expect { n.to_html }.to raise_error(NotImplementedError)
      expect { n.inner_html }.to raise_error(NotImplementedError)
      expect { n.outer_html }.to raise_error(NotImplementedError)
    end
  end

  describe "the serialization buffer is bounded (fails closed, never OOM)" do
    # The buffer cap is scaled to the document's content (arena_bytes), so any
    # legitimate document serialises, but a pathologically deep CONSTRUCTED tree
    # whose pretty-printed form is super-linear in the content fails closed with
    # Makiri::Error rather than growing the buffer without limit.
    it "serialises a deep tree but fails closed pretty-printing one whose indentation explodes" do
      doc = Makiri::XML("<r/>")
      cur = doc.root
      8000.times { e = doc.create_element("a"); cur.add_child(e); cur = e }
      expect(doc.to_xml.bytesize).to be < 100_000          # compact form is bounded
      expect { doc.to_xml(pretty: true) }                  # indentation would be ~quadratic
        .to raise_error(Makiri::Error, /size limit|out of memory/)
    end
  end
end
