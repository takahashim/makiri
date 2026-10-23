# frozen_string_literal: true

# Makiri runs no custom-function handler: XPath is the 26 core functions (and
# the namespaced built-ins), and a call to any other is an XPath::SyntaxError.
# An unprefixed one is refused when the expression is parsed; a prefixed one
# when it is evaluated, since the prefix is bound then.
RSpec.describe "an XPath call to an unknown function" do
  let(:doc) { Makiri::HTML("<p>hi</p><p>there</p>") }
  let(:xml) { Makiri::XML("<r><p>a</p></r>") }

  it "is a syntax error from Node#xpath and #at_xpath, on HTML and XML" do
    [doc, xml].each do |d|
      expect { d.xpath("my-count(//p)") }.to raise_error(Makiri::XPath::SyntaxError, /unknown function my-count/)
      expect { d.at_xpath("//p[f()]") }.to raise_error(Makiri::XPath::SyntaxError, /unknown function f/)
    end
  end

  it "is refused at parse time even where it would never be called" do
    expect { doc.xpath("false() and f()") }.to raise_error(Makiri::XPath::SyntaxError)
  end

  it "is a syntax error from XPathContext#evaluate, prefixed or not" do
    ctx = Makiri::XPathContext.new(doc)
    ctx.register_namespace("ext", "urn:ext")
    expect { ctx.evaluate("thrice('a')") }.to raise_error(Makiri::XPath::SyntaxError)
    expect { ctx.evaluate("ext:thrice('a')") }.to raise_error(Makiri::XPath::SyntaxError, /unknown function ext:thrice/)
    expect(ctx.evaluate("count(//p)")).to eq(2.0)
  end

  it "refuses a handler argument" do
    handler = Class.new { def thrice(s) = s * 3 }.new
    expect { doc.xpath("thrice('a')", handler) }.to raise_error(ArgumentError, /handlers are not supported/)
    expect { xml.at_xpath("//p", handler) }.to raise_error(ArgumentError, /handlers are not supported/)
    expect { Makiri::XPathContext.new(doc).evaluate("//p", handler) }.to raise_error(ArgumentError)
  end

  it "still takes nil, or a namespace Hash, as the optional argument" do
    expect(doc.xpath("//p", nil).size).to eq(2)
    expect(xml.xpath("//d:p", { "d" => "urn:x" }).size).to eq(0)
  end
end
