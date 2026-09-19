# frozen_string_literal: true

# `#xpath` / `#at_xpath` read one argument list for HTML and XML alike:
# `(expr, [namespaces Hash], [handler], namespace_matching:, **prefix_bindings)`.
RSpec.describe "XPath query arguments" do
  let(:svg) { "http://www.w3.org/2000/svg" }
  let(:html) { Makiri::HTML("<svg><path/></svg>") }
  let(:xml) { Makiri::XML(%(<r xmlns:s="urn:s"><s:p/></r>)) }
  let(:handler) { Class.new { def up(s) = s.upcase }.new }

  it "binds a per-query namespace Hash on HTML too" do
    expect(html.xpath("//s:path", { "s" => svg }).size).to eq(1)
    expect(html.at_xpath("//s:path", { "s" => svg }).name).to eq("path")
  end

  it "takes keywords other than namespace_matching: as prefix bindings" do
    expect(html.xpath("//s:path", s: svg).size).to eq(1)
    expect(xml.xpath("//s:p", s: "urn:s").size).to eq(1)
  end

  it "accepts a handler on XML, and both a Hash and a handler in either order" do
    expect(xml.xpath("up('a')", handler)).to eq("A")
    expect(xml.xpath("up(name(//s:p))", { "s" => "urn:s" }, handler)).to eq("S:P")
    expect(html.xpath("up(name(//s:path))", handler, { "s" => svg })).to eq("PATH")
  end

  it "reads namespace_matching: as the mode, never as a prefix" do
    expect(html.xpath("//path", namespace_matching: :lax).size).to eq(1)
    # Lax changes nothing for XML (Nokogiri::XML is namespace-strict), and the
    # keyword is not registered as a prefix named "namespace_matching".
    expect(xml.xpath("//s:p", namespace_matching: :lax, s: "urn:s").size).to eq(1)
    expect { xml.xpath("//namespace_matching:p", namespace_matching: :lax) }
      .to raise_error(Makiri::Error, /unknown namespace prefix/)
  end

  it "refuses two namespace Hashes or two handlers" do
    expect { html.xpath("//a", {}, {}) }.to raise_error(ArgumentError)
    expect { html.xpath("//a", handler, handler) }.to raise_error(ArgumentError)
  end
end
