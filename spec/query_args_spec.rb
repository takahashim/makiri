# frozen_string_literal: true

# `#xpath` / `#at_xpath` read one argument list for HTML and XML alike:
# `(expr, [namespaces Hash], namespace_matching:, **prefix_bindings)`.
RSpec.describe "XPath query arguments" do
  let(:svg) { "http://www.w3.org/2000/svg" }
  let(:html) { Makiri::HTML("<svg><path/></svg>") }
  let(:xml) { Makiri::XML(%(<r xmlns:s="urn:s"><s:p/></r>)) }

  it "binds a per-query namespace Hash on HTML too" do
    expect(html.xpath("//s:path", { "s" => svg }).size).to eq(1)
    expect(html.at_xpath("//s:path", { "s" => svg }).name).to eq("path")
  end

  it "takes keywords other than namespace_matching: as prefix bindings" do
    expect(html.xpath("//s:path", s: svg).size).to eq(1)
    expect(xml.xpath("//s:p", s: "urn:s").size).to eq(1)
  end

  it "reads namespace_matching: as the mode, never as a prefix" do
    expect(html.xpath("//path", namespace_matching: :lax).size).to eq(1)
    # Lax changes nothing for XML (Nokogiri::XML is namespace-strict), and the
    # keyword is not registered as a prefix named "namespace_matching".
    expect(xml.xpath("//s:p", namespace_matching: :lax, s: "urn:s").size).to eq(1)
    expect { xml.xpath("//namespace_matching:p", namespace_matching: :lax) }
      .to raise_error(Makiri::Error, /unknown namespace prefix/)
  end

  it "registers prefix keywords on an XPathContext, as #xpath does" do
    ctx = Makiri::XPathContext.new(xml.root, s: "urn:s")
    expect(ctx.evaluate("//s:p").size).to eq(1)
    # The mode keyword is still the mode, not a prefix.
    lax = Makiri::XPathContext.new(xml.root, s: "urn:s", namespace_matching: :lax)
    expect(lax.evaluate("//s:p").size).to eq(1)
    expect { Makiri::XPathContext.new(xml.root, s: "bad\0uri") }
      .to raise_error(Makiri::Error, /invalid namespace mapping/)
  end

  it "takes the same (selector, namespaces) list for CSS on either representation" do
    expect(html.css("path", nil).size).to eq(1)
    expect(html.css("path", {}).size).to eq(1)
    expect(html.at_css("path").matches?("path", {})).to be(true)
    expect(xml.css("s|p", { "s" => "urn:s" }).size).to eq(1)
    # HTML takes the bindings and does not use them (Lexbor matches a prefixed
    # type selector loosely) - what Nokogiri::HTML5 answers for the same call.
    expect(html.css("path", { "svg" => svg }).size).to eq(1)
    expect(html.css("svg|path", { "svg" => svg }).size).to eq(1)
    expect { html.css("path", 5) }.to raise_error(TypeError, /must be a Hash/)
  end

  it "refuses two namespace Hashes, or anything but a Hash" do
    expect { html.xpath("//a", {}, {}) }.to raise_error(ArgumentError)
    expect { xml.xpath("//a", Object.new) }.to raise_error(ArgumentError, /handlers are not supported/)
  end

  it "reads a namespace Hash through its storage, not overridden Hash methods" do
    # A subclass whose #merge/#dup/#delete lie must not be able to drop or forge
    # a binding: the Hash is read with rb_hash_foreach/rb_hash_aset.
    klass = Class.new(Hash) do
      def merge(*) = {}
      def dup = {}
      def delete(*) = nil
    end
    h = klass.new
    h["s"] = "urn:s"
    # Positional Hash plus keyword bindings is the branch that used to call
    # `Hash#merge`; the keyword binding wins and the override is not consulted.
    expect(xml.xpath("//s:p", h, s: "urn:s").size).to eq(1)
  end

  it "refuses a namespace key of a String subclass, whose #hash is the caller's" do
    called = false
    key = Class.new(String) { define_method(:hash) { called = true; super() } }.new("s")
    h = { key => "urn:s" }
    called = false
    expect { xml.xpath("//s:p", h, t: "urn:t") }.to raise_error(TypeError, /not a subclass's/)
    expect(called).to be(false)
  end
end
