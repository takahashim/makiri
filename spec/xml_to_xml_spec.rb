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

    it "keeps a descendant's own declarations that change the binding" do
      xml = %(<r xmlns:p="urn:a"><a xmlns="urn:b"/><p:x xmlns:p="urn:c" xmlns:q="urn:d"/>) +
            %(<d xmlns="urn:e"><u xmlns=""/></d></r>)
      expect(Makiri::XML(xml).canonicalize)
        .to eq(%(<r xmlns:p="urn:a"><a xmlns="urn:b"></a>) +
               %(<p:x xmlns:p="urn:c" xmlns:q="urn:d"></p:x>) +
               %(<d xmlns="urn:e"><u xmlns=""></u></d></r>))
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
    # itself cannot read back - which breaks "the output re-parses to the same
    # tree" - and the walk is recursive, so a deep enough tree would exhaust the
    # C stack before it got there (a 1 MB stack, the Windows default, runs out
    # around a few thousand frames). Both are refused at the reader's own cap.
    #
    # The cap counts ELEMENT nesting, the way the reader does. Anything else
    # would refuse documents the reader accepts, so the boundary cases below are
    # what keep the two in step.
    it "round-trips at the deepest nesting the reader accepts" do
      doc = Makiri::XML("<a>" * 1024 + "</a>" * 1024)
      out = doc.to_xml
      expect(Makiri::XML(out).to_xml).to eq(out)
      expect(doc.root.canonicalize).to be_a(String)
      expect(doc.to_xml(pretty: true)).to be_a(String)   # quadratic but still bounded
    end

    # A leaf at the deepest element is not another level of nesting.
    {
      "text" => "x",
      "a comment" => "<!--c-->",
      "CDATA" => "<![CDATA[x]]>",
      "a processing instruction" => "<?p x?>",
    }.each do |what, leaf|
      it "still serializes with #{what} at the deepest element" do
        doc = Makiri::XML("<a>" * 1024 + leaf + "</a>" * 1024)
        out = doc.to_xml
        expect(Makiri::XML(out).to_xml).to eq(out)
        expect(doc.canonicalize).to be_a(String)
      end
    end

    # A fragment has no markup of its own, so it is not a level either.
    it "does not count a fragment as a level" do
      doc = Makiri::XML("<r/>")
      frag = doc.fragment("<a>" * 1024 + "</a>" * 1024)
      expect(frag.to_xml).to include("<a>")
    end

    it "fails closed past the cap instead of emitting XML it could not re-parse" do
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

  describe "namespace planning scales with the document, not its square" do
    # The bindings in scope were a chain of per-element links, and resolving one
    # prefix re-walked it rescanning every ancestor's ATTRIBUTE list, so #to_xml
    # cost O(depth^2 x attributes): 403 KB of nested prefixed attributes took
    # 4.88s against 0.097s for the same bytes unprefixed. They are one stack now
    # (serialize/xml.rs), as the parser has always kept them.
    def nested_prefixed(depth, attrs)
      open = (1..depth).map { |i| "<e#{i} " + (1..attrs).map { |j| %(p:a#{j}="v") }.join(" ") + ">" }
      close = (1..depth).to_a.reverse.map { |i| "</e#{i}>" }
      %(<r xmlns:p="urn:p">#{open.join}#{close.join}</r>)
    end

    def seconds
      t = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      yield
      Process.clock_gettime(Process::CLOCK_MONOTONIC) - t
    end

    it "stays linear as depth and attribute count both double" do
      small = Makiri::XML(nested_prefixed(100, 25))
      large = Makiri::XML(nested_prefixed(200, 50))   # 4x the attributes
      small.to_xml # warm
      ratio = seconds { large.to_xml } / [seconds { small.to_xml }, 1e-6].max
      # Linear would be ~4; the chain made this ~16 and rising. A loose ceiling
      # keeps the example about the complexity class, not the machine.
      expect(ratio).to be < 10
    end

    it "declares each prefix once, wherever it is bound" do
      doc = Makiri::XML(nested_prefixed(3, 2))
      out = doc.to_xml
      expect(out.scan("xmlns:p=").length).to eq(1)
      expect(out).to include(%(p:a1="v"))
      expect(Makiri::XML(out).to_xml).to eq(out) # round-trips
    end
  end

  # Output that did not re-parse. Each example serialises, re-parses, and
  # compares what matters with the original.
  describe "well-formed output" do
    def names_of(doc)
      doc.xpath("//*").map { |e| [e.name, e.namespace_uri] }
    end

    # The parser merges adjacent CDATA sections (as libxml2 does), so one node
    # can hold "]]>"; written raw, it closed the section early.
    it "splits ]]> in a CDATA value across two sections, as libxml2 does" do
      doc = Makiri::XML("<r><![CDATA[a]]]><![CDATA[]>b]]></r>")
      expect(doc.root.to_xml).to eq("<r><![CDATA[a]]]]><![CDATA[>b]]></r>")
      expect(Makiri::XML(doc.to_xml).root.children.map(&:content)).to eq(["a]]>b"])
    end

    it "quotes a SYSTEM id holding a double quote with single quotes" do
      doc = Makiri::XML(%(<!DOCTYPE r SYSTEM 'a"b'><r/>))
      expect(doc.to_xml).to include(%(<!DOCTYPE r SYSTEM 'a"b'>))
      expect(Makiri::XML(doc.to_xml).internal_subset.system_id).to eq('a"b')
    end

    # A prefix was invented for the element - xmlns:ns1="" - which Namespaces
    # 1.0 forbids. The DOM Parsing spec drops the declaration instead, so the
    # element stays in no namespace. Nokogiri writes it and the element moves.
    it "leaves out an xmlns attribute that contradicts a no-namespace element" do
      doc = Makiri::XML("<r><c/></r>")
      doc.root["xmlns"] = "urn:x"
      expect(doc.root.to_xml).to eq("<r><c/></r>")
      expect(names_of(Makiri::XML(doc.to_xml))).to eq(names_of(doc))
    end
  end

  describe "an attribute with a namespace but no prefix" do
    # Unprefixed means no namespace (Namespaces in XML §6.2), so written bare it
    # lost its namespace on re-parse, beside a plain attribute of the same name.
    it "is written under a declared prefix" do
      doc = Makiri::XML(%(<r c="plain"/>))
      doc.root.set_attribute_ns("urn:p", "c", "v")
      back = Makiri::XML(doc.to_xml).root.attribute_nodes.reject { |a| a.name.start_with?("xmlns") }
      expect(back.map { |a| [a.local_name, a.namespace_uri, a.value] })
        .to contain_exactly(["c", nil, "plain"], ["c", "urn:p", "v"])
    end
  end

  describe "#canonicalize" do
    # c14n renders the declarations the document holds; when they no longer
    # give a name its namespace it refuses, where it wrote p:x under u2's
    # declaration - a different namespace - or an unbound prefix.
    it "refuses names their declarations no longer describe" do
      moved = Makiri::XML(%(<r><a xmlns:p="u1"><p:x/></a><b xmlns:p="u2"/></r>))
      moved.at_xpath("//b").add_child(moved.at_xpath("//*[local-name()='x']"))
      expect { moved.canonicalize }.to raise_error(Makiri::Error, /no longer match/)
      expect(Makiri::XML(moved.to_xml).at_xpath("//*[local-name()='x']").namespace_uri).to eq("u1")

      nsattr = Makiri::XML("<r/>")
      nsattr.root.set_attribute_ns("urn:q", "q:a", "v")
      expect { nsattr.canonicalize }.to raise_error(Makiri::Error, /bound to nothing/)
    end

    it "still renders a consistent document and a subtree under its ancestors' declarations" do
      doc = Makiri::XML(%(<r xmlns:p="u"><a xmlns:q="v"><p:x q:y="1"/></a></r>))
      expect(doc.canonicalize).to eq(%(<r xmlns:p="u"><a xmlns:q="v"><p:x q:y="1"></p:x></a></r>))
      expect(doc.at_xpath("//*[local-name()='x']").canonicalize)
        .to eq(%(<p:x xmlns:p="u" xmlns:q="v" q:y="1"></p:x>))
    end
  end

  # "Not decided yet" and "no namespace" were one state for an attribute, so
  # detach-edit-reinsert sequences left names the output could not express.
  # Each example ends with the invariant: the output re-parses to the same
  # names and namespaces.
  describe "namespaces decided when a detached node is inserted" do
    def names(doc)
      doc.xpath("//*").map { |e| [e.name, e.namespace_uri, e.attribute_nodes.map { |a| [a.name, a.namespace_uri] }] }
    end

    def expect_round_trip(doc)
      expect(names(Makiri::XML(doc.to_xml))).to eq(names(doc))
    end

    it "resolves an attribute set while its (parsed) element was detached" do
      doc = Makiri::XML(%(<r xmlns:q="urn:q"><e/></r>))
      e = doc.at_xpath("//e")
      e.remove
      e["q:a"] = "1"
      doc.root << e
      expect(e.attribute_nodes.map(&:namespace_uri)).to eq(["urn:q"])
      expect_round_trip(doc)
    end

    it "refuses the insertion when that prefix is still unbound" do
      doc = Makiri::XML("<r><e/></r>")
      e = doc.at_xpath("//e")
      e.remove
      e["q:a"] = "1"
      expect { doc.root << e }.to raise_error(Makiri::Error, /not bound/)
    end

    it "refuses two attributes that insertion gives one key" do
      doc = Makiri::XML(%(<r xmlns:p="u" xmlns:q="u"/>))
      n = doc.create_element("n")
      n["p:a"] = "1"
      n["q:a"] = "2"
      expect { doc.root << n }.to raise_error(Makiri::Error, /already has an attribute/)
    end

    it "does not let a pending attribute answer for a no-namespace key" do
      doc = Makiri::XML("<r/>")
      e = doc.create_element("e")
      e["p:a"] = "1"
      e.set_attribute_ns("", "a", "2")
      expect(e.attribute_nodes.map(&:name)).to eq(["p:a", "a"])
    end
  end

  describe "a prefix bound to nothing" do
    it "is refused by both writers rather than written as xmlns:q=\"\" or bare" do
      e = Makiri::XML::Document.new.create_element("q:e")
      expect { e.to_xml }.to raise_error(Makiri::Error, /bound to nothing/)
      expect { e.canonicalize }.to raise_error(Makiri::Error, /bound to nothing/)
    end
  end

  # The DOM's "validate and extract", plus Namespaces in XML's converse for the
  # XML namespace: each of these wrote a tree that did not re-read.
  describe "set_attribute_ns" do
    {
      ["", "p:a"] => "a prefix without a namespace",
      ["urn:x", "xml:lang"] => "xml with another namespace",
      ["http://www.w3.org/XML/1998/namespace", "p:a"] => "the XML namespace under another prefix",
      ["urn:x", "xmlns:p"] => "xmlns with another namespace",
      ["http://www.w3.org/2000/xmlns/", "a"] => "the XMLNS namespace on another name"
    }.each do |(ns, qname), what|
      it "refuses #{what}" do
        doc = Makiri::XML("<r/>")
        expect { doc.root.set_attribute_ns(ns, qname, "v") }.to raise_error(Makiri::Error, /does not fit/)
      end
    end

    it "still takes xml:lang in its own namespace" do
      doc = Makiri::XML("<r/>")
      doc.root.set_attribute_ns("http://www.w3.org/XML/1998/namespace", "xml:lang", "en")
      expect(doc.root.to_xml).to eq(%(<r xml:lang="en"/>))
    end
  end

  # The writer ignored a no-namespace element's contrary xmlns, but the
  # mutators still resolved against it: a new child took its namespace.
  it "ignores a contrary default declaration when resolving too" do
    doc = Makiri::XML("<r><e/></r>")
    e = doc.at_xpath("//e")
    e["xmlns"] = "urn:x"
    e << doc.create_element("c")
    expect(e.namespace_uri).to be_nil
    expect(doc.at_xpath("//c").namespace_uri).to be_nil
  end
end
