# frozen_string_literal: true

require "spec_helper"

# The shared glue (XPath engine, NodeSet, node identity) is used by BOTH the
# HTML (Lexbor) and XML (custom arena) representations. These regress the
# HTML/XML boundary: operations must be kind-aware and fail closed on a mix,
# never assert-abort the process or silently wrap a node under the wrong
# representation. See docs/html_xml_boundary_hardening.ja.md.
RSpec.describe "Makiri HTML/XML representation boundary" do
  # Run a snippet in a fresh process and return [Process::Status, stdout+stderr].
  # The two formerly-aborting cases (== and node= against an XML Document) are
  # checked this way so a regression surfaces as a failed child, not a crash of
  # the whole rspec run.
  def run_isolated(code)
    lib = File.expand_path("../lib", __dir__)
    # Run the child as a normal process: drop RUBY_FREE_AT_EXIT, which the
    # ruby_memcheck Valgrind harness sets in our env. Inherited, it makes Ruby 3.4
    # print "warning: Free at exit is experimental and may be unstable" to stderr,
    # which - since we fold stderr into stdout below - would pollute the captured
    # output these specs assert on (and its full-heap teardown adds Valgrind noise).
    env = { "RUBY_FREE_AT_EXIT" => nil }
    out = IO.popen([env, RbConfig.ruby, "-I#{lib}", "-e", %(require "makiri"\n#{code})],
                   err: %i[child out], &:read)
    [$?, out]
  end

  # The two cases below USED to assert-abort. They are kept as subprocess specs so
  # a regression surfaces as a failed child, not a crash of the whole rspec run -
  # but on macOS a child can't inherit the ASan DYLD preload (SIP strips DYLD_*),
  # so skip them under the sanitizer (the in-process examples that follow cover
  # the same kind-aware Document-branch under ASan).
  def skip_under_sanitizer!
    return unless ENV.key?("ASAN_OPTIONS")

    skip "subprocess can't inherit the ASan preload on macOS (SIP strips DYLD_*)"
  end

  describe "kind-aware document unwrap (no assert-abort)" do
    it "compares an XML node to its Document without aborting (isolated)" do
      skip_under_sanitizer!
      status, out = run_isolated(<<~RUBY)
        doc = Makiri::XML("<r><a/></r>")
        print(doc.at_xpath("//a") == doc)
      RUBY
      expect(status).to be_success, "process aborted: #{out}"
      expect(out).to eq("false")
    end

    it "rebinds XPathContext#node to an XML Document without aborting (isolated)" do
      skip_under_sanitizer!
      status, out = run_isolated(<<~RUBY)
        doc = Makiri::XML("<r><a/><a/></r>")
        ctx = Makiri::XPathContext.new(doc.root)
        ctx.node = doc
        print(ctx.evaluate("count(//a)"))
      RUBY
      expect(status).to be_success, "process aborted: #{out}"
      expect(out).to eq("2.0")
    end

    it "compares an XML node to its Document (and rebinds node=) in-process" do
      # The same Document-branch of the kind-aware unwrap, run in-process so it is
      # exercised under the sanitizer too.
      doc = Makiri::XML("<r><a/><a/></r>")
      expect(doc.at_xpath("//a") == doc).to be(false)
      ctx = Makiri::XPathContext.new(doc.root)
      ctx.node = doc
      expect(ctx.evaluate("count(//a)")).to eq(2.0)
    end

    it "treats two wrappers of the same XML node as equal, and a foreign node as unequal" do
      doc = Makiri::XML("<r><a/></r>")
      a1 = doc.at_xpath("//a")
      a2 = doc.at_xpath("//r/a")
      expect(a1).to eq(a2)
      expect(a1.hash).to eq(a2.hash)
      # an XML node compared to an HTML node is unequal, not a crash
      html_node = Makiri::HTML("<p/>").at_xpath("//p")
      expect(a1 == html_node).to be(false)
    end

    it "rejects an XPathContext#node from a different document" do
      a = Makiri::XML("<r><a/></r>").root
      ctx = Makiri::XPathContext.new(a)
      other = Makiri::XML("<z><b/></z>").root
      expect { ctx.node = other }.to raise_error(Makiri::Error, /same document/)
    end
  end

  describe "custom-function handler results respect the document boundary" do
    ns = "urn:fns"
    handler = Class.new do
      def first_node(set) = set.first   # returns a Node -> push into the result set
      def my_count(set) = set.length * 1.0
    end.new

    it "accepts a same-document node returned by an XML handler" do
      xml = Makiri::XML("<r><a/><a/></r>")
      ctx = Makiri::XPathContext.new(xml.root)
      ctx.register_namespace("f", ns)
      expect(ctx.evaluate("f:my-count(//a)", handler)).to eq(2.0)
      result = ctx.evaluate("f:first-node(//a)", handler)
      expect(result).to be_a(Makiri::NodeSet)
      expect(result.first.name).to eq("a")
    end

    it "fails closed when an XML handler returns a node from another document" do
      xml = Makiri::XML("<r><a/></r>")
      other = Makiri::XML("<z><b/></z>")
      foreign = Class.new do
        define_method(:foreign) { |_set| other.at_xpath("//b") }
      end.new
      ctx = Makiri::XPathContext.new(xml.root)
      ctx.register_namespace("f", ns)
      expect { ctx.evaluate("f:foreign(//a)", foreign) }
        .to raise_error(Makiri::Error, /different document/)
    end

    it "still accepts a same-document node from an HTML handler (regression)" do
      html = Makiri::HTML("<div><p>hi</p><p>yo</p></div>")
      ctx = Makiri::XPathContext.new(html)
      ctx.register_namespace("f", ns)
      result = ctx.evaluate("f:first-node(//p)", handler)
      expect(result.first.text).to eq("hi")
    end
  end

  describe "NodeSet set operations require the same document" do
    it "raises rather than mixing HTML and XML node sets" do
      xml = Makiri::XML("<r><a/></r>").xpath("//a")
      html = Makiri::HTML("<div><p/></div>").xpath("//p")
      %i[| + & -].each do |op|
        expect { xml.public_send(op, html) }
          .to raise_error(Makiri::Error, /different documents/)
      end
    end

    it "raises across two distinct XML documents" do
      a = Makiri::XML("<r><a/></r>").xpath("//a")
      b = Makiri::XML("<r><a/></r>").xpath("//a")
      expect { a | b }.to raise_error(Makiri::Error, /different documents/)
    end

    it "still unions/intersects/differences within one document (order + dedup)" do
      doc = Makiri::XML("<r><a/><b/><c/></r>")
      as = doc.xpath("//a | //b")
      bs = doc.xpath("//b | //c")
      expect((as | bs).map(&:name)).to eq(%w[a b c])     # encounter order, deduped
      expect((as & bs).map(&:name)).to eq(%w[b])
      expect((as - bs).map(&:name)).to eq(%w[a])
      expect((as + bs).map(&:name)).to eq(%w[a b b c])   # concatenation keeps dups
    end
  end

  # An HTML-only API that hands a node ARGUMENT to Lexbor must reject a Makiri::XML
  # node first - its stored pointer is an mkr_xml_node_t*, not an lxb_dom_node_t,
  # so reading it as one corrupts memory / segfaults. Coercion goes through
  # mkr_html_arg_node, which fails closed with TypeError. The ONE sanctioned
  # crossing point is Document#import_node, which TRANSLATES across representations
  # (see "cross-kind import_node") rather than rejecting.
  describe "HTML APIs reject an XML node argument" do
    let(:html) { Makiri::HTML("<div><p/></div>") }
    let(:xml_node) { Makiri::XML("<r><a/></r>").root }

    it "raises TypeError on add_child / before / after / replace / fragment" do
      expect { html.at_css("div").add_child(xml_node) }.to raise_error(TypeError)
      expect { html.at_css("p").before(xml_node) }.to raise_error(TypeError)
      expect { html.at_css("p").after(xml_node) }.to raise_error(TypeError)
      expect { html.at_css("p").replace(xml_node) }.to raise_error(TypeError)
      expect { html.fragment("<b/>", context: xml_node) }.to raise_error(TypeError)
    end

    it "still imports / inserts a same-document HTML node" do
      div = html.at_css("div")
      div.add_child(html.create_element("span"))
      expect(div.to_html).to eq("<div><p></p><span></span></div>")
    end

    # import_node now TRANSLATES an XML node (it is the cross-kind crossing point),
    # so it must not crash and must yield an HTML node. Isolated so a regression
    # fails the child rather than crashing the whole run (skipped under the
    # sanitizer, where the child can't inherit the ASan preload on macOS).
    it "translates (does not crash on) import_node(xml_node)" do
      skip_under_sanitizer!
      status, out = run_isolated(<<~RUBY)
        h = Makiri::HTML("<div/>")
        x = Makiri::XML("<r/>")
        imp = h.import_node(x.root)
        print(imp.is_a?(Makiri::HTML::Element) ? "translated" : "wrong")
      RUBY
      expect(status).to be_success, "process aborted: #{out}"
      expect(out).to eq("translated")
    end
  end
end
