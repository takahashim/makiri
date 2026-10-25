# frozen_string_literal: true

# How the extension's methods read their arguments: positional versus keyword,
# the arity errors, and how a block's exits travel out of NodeSet#each.
#
# These are what magnus's `scan_args` and `method!` answered before Makiri
# replaced them with its own argument reading (`glue::args`) and method
# trampolines (`bridge::rb::method`). They were pinned first, and the
# replacement answers the same - messages included, since Ruby's own arity
# message is what a caller sees.
RSpec.describe "argument shapes and block exits" do
  svg = "http://www.w3.org/2000/svg"
  let(:doc) { Makiri::HTML("<p>a</p><svg><circle/></svg>") }
  let(:xml) { Makiri::XML(%(<r xmlns:s="urn:s"><s:p/><s:p/></r>)) }

  def arity_error(given, expected)
    raise_error(ArgumentError, "wrong number of arguments (given #{given}, expected #{expected})")
  end

  describe "a method with an optional positional Hash and keywords (#xpath)" do
    it "takes the Hash positionally and bindings as keywords" do
      expect(doc.xpath("//s:circle", { "s" => svg }).length).to eq(1)
      expect(doc.xpath("//s:circle", s: svg).length).to eq(1)
    end

    it "reads namespace_matching: as a keyword, beside a positional Hash" do
      expect(doc.xpath("//circle", namespace_matching: :lax).length).to eq(1)
      expect(doc.xpath("//s:circle", { "s" => svg }, namespace_matching: :lax).length).to eq(1)
    end

    it "lets a keyword binding win over the positional Hash" do
      expect(doc.xpath("//s:circle", { "s" => "urn:x" }, s: svg).length).to eq(1)
    end

    it "takes a braced Hash as the positional one, not as keywords" do
      expect { doc.xpath("//circle", { namespace_matching: :lax }) }
        .to raise_error(TypeError, /wrong argument type Symbol \(expected String\)/)
    end

    it "takes nil for the Hash, and refuses anything else" do
      expect(doc.xpath("//p", nil).length).to eq(1)
      expect { doc.xpath("//p", 1) }.to raise_error(ArgumentError, /expected a namespace Hash/)
    end

    it "raises Ruby's arity error" do
      expect { doc.xpath }.to arity_error(0, "1..2")
      expect { doc.xpath(s: svg) }.to arity_error(0, "1..2")
      expect { doc.at_xpath(s: svg) }.to arity_error(0, "1..2")
      expect { doc.xpath("//p", {}, {}) }.to arity_error(3, "1..2")
      expect { xml.root.css("p", {}, {}) }.to arity_error(3, "1..2")
    end
  end

  describe "a method with a required argument and keywords" do
    it "reads XPathContext.new's keywords, and refuses a braced Hash" do
      expect(Makiri::XPathContext.new(doc, s: svg).evaluate("//s:circle").length).to eq(1)
      expect(Makiri::XPathContext.new(doc, namespace_matching: :lax).evaluate("//circle").length)
        .to eq(1)
      expect { Makiri::XPathContext.new(doc, { s: svg }) }.to arity_error(2, 1)
      expect { Makiri::XPathContext.new }.to arity_error(0, 1)
      expect { Makiri::XPathContext.new(doc, namespace_matching: :nope) }
        .to raise_error(ArgumentError, "namespace_matching: must be :strict or :lax, got :nope")
    end

    it "reads Document#fragment's keywords, ignores an unknown one, refuses a braced Hash" do
      expect(doc.fragment("<td>x</td>", context: "tr").children.map(&:name)).to eq(["td"])
      expect(doc.fragment("<b>x</b>", foo: 1).children.map(&:name)).to eq(["b"])
      expect { doc.fragment("<td>x</td>", { context: "tr" }) }.to arity_error(2, 1)
      expect { doc.fragment("<b><b><b>x</b></b></b>", max_tree_depth: 2) }
        .to raise_error(Makiri::Error, "document tree depth limit exceeded (2)")
      expect(Makiri::HTML::DocumentFragment.parse("<td>x</td>", context: "tr").children.map(&:name))
        .to eq(["td"])
    end

    it "reads XML::Document.parse's keywords, and refuses an unknown one" do
      expect { Makiri::XML::Document.parse("<r/>", max_bytes: 2) }
        .to raise_error(Makiri::XML::LimitExceeded)
      expect { Makiri::XML::Document.parse("<r/>", foo: 1) }
        .to raise_error(ArgumentError, "unknown keyword: :foo")
      expect { Makiri::XML::Document.parse("<r/>", { max_bytes: 2 }) }.to arity_error(2, 1)
    end
  end

  describe "a method that takes only keywords" do
    it "reads them, ignores an unknown one, and refuses positional arguments" do
      p = doc.at_css("p")
      expect(p.to_html(pretty: true)).to eq(%(<p>\n  "a"\n</p>\n))
      expect(p.to_html(foo: 1)).to eq("<p>a</p>")
      expect { p.to_html({ pretty: true }) }.to arity_error(1, 0)
      expect { p.to_html(1, 2) }.to arity_error(2, 0)
      expect(xml.to_xml(indent: 2)).to be_a(String)
      expect { xml.to_xml({ indent: 2 }) }.to arity_error(1, 0)
    end
  end

  describe "a method that takes no keywords" do
    # Ruby passes keywords to such a method as a trailing positional Hash, so
    # `deep: true` is the `deep` argument, a Hash, and truthy.
    it "receives keywords as a positional Hash" do
      copy = doc.import_node(xml.root, deep: true)
      expect(copy.name).to eq("r")
      expect(copy.children.length).to eq(2)
    end
  end

  describe "arity across the shapes" do
    let(:xdoc) { Makiri::XML("<r/>") }

    it "counts a rest shape as 'n+', and passes a trailing Hash into it" do
      expect { xdoc.create_element }.to arity_error(0, "1+")
      expect(xdoc.create_element("e", "t", { "a" => "1" }, nil).to_xml).to eq(%(<e a="1">t</e>))
      expect(xdoc.create_element("e", a: "1").to_xml).to eq(%(<e a="1"/>))
    end

    it "counts two optional arguments as 'n..n+2', taking nil for either" do
      expect { xdoc.create_document_type }.to arity_error(0, "1..3")
      expect { xdoc.create_document_type("a", "b", "c", "d") }.to arity_error(4, "1..3")
      expect { doc.create_document_type("a", "b", "c", "d") }.to arity_error(4, "1..3")
      expect(xdoc.create_document_type("a", nil, nil).to_xml).to eq("<!DOCTYPE a>")
    end

    it "answers the hand-counted arities with the same message" do
      expect { xdoc.root.clone_node(true, 1) }.to arity_error(2, "0..1")
      expect { doc.at_css("p").clone_node(1, 2) }.to arity_error(2, "0..1")
      expect { doc.css("p")[0, 1, 2] }.to arity_error(3, "1..2")
      expect { Makiri::NodeSet.new }.to arity_error(0, "1..2")
      expect { Makiri::NodeSet.new(doc, [], 1) }.to arity_error(3, "1..2")
      expect(Makiri::NodeSet.new(doc, nil).length).to eq(0)
    end

    it "counts only the positional arguments when keywords are given" do
      expect { Makiri::XPathContext.new(doc, {}, x: "u") }.to arity_error(2, 1)
      expect(doc.xpath("//p", nil, x: "u").length).to eq(1)
    end

    it "reads a nil keyword as absent" do
      expect(doc.at_css("p").to_html(pretty: nil)).to eq("<p>a</p>")
      expect(doc.fragment("<b>x</b>", context: nil).children.length).to eq(1)
    end
  end

  describe "NodeSet#each" do
    let(:set) { doc.css("p, circle") }

    it "returns an Enumerator without a block, and the set with one" do
      expect(set.each).to be_a(Enumerator)
      expect(set.each { nil }).to equal(set)
    end

    it "lets break carry a value out, and leaves the document unborrowed" do
      expect(set.each { |n| break n.name }).to eq("p")
      expect(Makiri.__borrowed?(doc)).to be(false)
    end

    it "lets next skip, and throw unwind to its catch" do
      seen = []
      set.each do |n|
        next if n.name == "p"

        seen << n.name
      end
      expect(seen).to eq(["circle"])
      expect(catch(:k) { set.each { |n| throw :k, n.name } }).to eq("p")
    end

    it "passes the block's own exception through, unchanged" do
      boom = Class.new(StandardError).new("boom")
      expect { set.each { raise boom } }.to raise_error { |e| expect(e).to equal(boom) }
      expect(Makiri.__borrowed?(doc)).to be(false)
    end

    it "lets a method return from inside the block" do
      first_name = ->(s) { s.each { |n| return n.name } } # rubocop:disable Lint/NonLocalExitFromIterator
      def first_of(set) = set.each { |n| return n.name } # rubocop:disable Lint/NonLocalExitFromIterator
      expect(first_of(set)).to eq("p")
      expect(first_name.call(set)).to eq("p")
    end

    it "visits every node of the set while the block removes them" do
      seen = []
      set.each do |n|
        seen << n.name
        n.remove
      end
      expect(seen).to eq(%w[p circle])
      expect(doc.css("p, circle").length).to eq(0)
    end
  end
end
