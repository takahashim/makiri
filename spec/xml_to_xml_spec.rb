# frozen_string_literal: true

require "spec_helper"

RSpec.describe "Makiri::XML#to_xml" do
  describe "Document#to_xml" do
    it "emits a bare XML declaration for a declaration-less source, then the nodes" do
      out = Makiri::XML("<r><a/></r>").to_xml
      expect(out).to start_with(%(<?xml version="1.0"?>\n))
      expect(out).to include("<r><a/></r>")
    end

    it "emits encoding=\"UTF-8\" only when the source declared an encoding (like Nokogiri)" do
      # No declaration / version-only -> bare; the output is UTF-8 either way.
      expect(Makiri::XML("<r/>").to_xml).to start_with(%(<?xml version="1.0"?>\n))
      expect(Makiri::XML(%(<?xml version="1.0"?><r/>)).to_xml).to start_with(%(<?xml version="1.0"?>\n))
      # An encoding pseudo-attribute in the source is reflected (named UTF-8, the
      # always-UTF-8 output encoding) ...
      expect(Makiri::XML(%(<?xml version="1.0" encoding="UTF-8"?><r/>)).to_xml)
        .to start_with(%(<?xml version="1.0" encoding="UTF-8"?>\n))
      # ... as is an explicit encoding: request on a declaration-less document.
      expect(Makiri::XML("<r/>").to_xml(encoding: "UTF-8"))
        .to start_with(%(<?xml version="1.0" encoding="UTF-8"?>\n))
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

    it "serializes just the subtree, self-contained, without an XML declaration" do
      el = doc.at_xpath("//p:b", "p" => "urn:a")
      # The subtree is cut off from the ancestor that declared the prefix, so the
      # serializer declares it here - the output re-parses to the same namespace
      # standing alone. Chrome's XMLSerializer does this; Nokogiri does not, and
      # its output does not round-trip.
      expect(el.to_xml).to eq(%(<p:b xmlns:p="urn:a" c="1">x</p:b>))
      expect(Makiri::XML(el.to_xml).root.namespace_uri).to eq("urn:a")
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

  describe "serialization is bounded (fails closed, never a stack overflow)" do
    # Only PARSING bounds nesting; the factories will build a tree of any depth.
    # Serializing one deeper than the reader accepts would emit XML that Makiri
    # itself cannot read back, breaking "the output re-parses to the same tree" -
    # and the walk is recursive, so a deep enough tree would exhaust the C stack
    # before it got there (a 1 MB stack, which Windows gives by default, runs out
    # around a few thousand frames). Both are refused at the reader's own cap.
    it "round-trips at the deepest nesting the reader accepts" do
      doc = Makiri::XML("<r>" + ("<a>" * 1023) + ("</a>" * 1023) + "</r>")
      out = doc.to_xml
      expect(Makiri::XML(out).to_xml).to eq(out)
      expect(doc.root.canonicalize).to be_a(String)
      expect(doc.to_xml(pretty: true)).to be_a(String)   # quadratic but still bounded
    end

    it "fails closed past it instead of emitting XML it could not re-parse" do
      doc = Makiri::XML("<r/>")
      cur = doc.root
      8000.times { e = doc.create_element("a"); cur.add_child(e); cur = e }

      expect { doc.to_xml }.to raise_error(Makiri::Error, /size limit|out of memory/)
      expect { doc.to_xml(pretty: true) }.to raise_error(Makiri::Error)
      expect { doc.root.canonicalize }.to raise_error(Makiri::Error)
      # the same tree is fine to hold, walk and query - only serializing it is not
      expect(doc.root.xpath("//a").length).to eq(8000)
    end
  end
end
