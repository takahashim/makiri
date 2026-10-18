# frozen_string_literal: true

require "spec_helper"

# Where the CSS parser follows CSS Syntax 3 and Selectors 4 and Lexbor does
# not (docs/parser_differences.ja.md, the C items from C4): one example per
# fix, each the smallest input the specification answers differently.
RSpec.describe "CSS Syntax 3 and Selectors 4 steps the CSS parser follows" do
  def specificity(selector)
    Makiri::Lexbor::CSS.parse_stylesheet("#{selector} {}").first[:selectors].first[:specificity]
  end

  # C23-C26: Selectors 4, 17 "Calculating a selector's specificity".
  it "counts specificity as Selectors 4 does" do
    expect(specificity("p:first-child")).to eq([0, 1, 1])
    expect(specificity("a:is(.b, .c .d)")).to eq([0, 2, 1])
    expect(specificity(":not(#a.b, #c#d)")).to eq([2, 0, 0])
    expect(specificity(":nth-child(1 of .a .b)")).to eq([0, 3, 0])
    expect(specificity("a:where(#x)")).to eq([0, 0, 1])
    expect(specificity("foo|div")).to eq([0, 0, 1])
    expect(specificity(".a" * 512)).to eq([0, 511, 0])
  end
  # C21: An+B is the whole argument (or followed by "of S" in :nth-child()).
  it "rejects a token after An+B" do
    doc = Makiri::HTML("<div><p>t</p><span>s</span><p>u</p></div>")
    [":nth-child(2n+1of p)", ":nth-child(3 n)", ":nth-of-type(2 of p)"].each do |sel|
      expect { doc.css(sel) }.to raise_error(Makiri::CSS::SyntaxError)
    end
    expect(doc.css(":nth-child(2n+1 of p)").map(&:text)).to eq(["t"])
  end
  # C18: a forgiving selector list may be empty; :is() and :where() then
  # match nothing.
  it "accepts an empty :is() or :where(), which matches nothing" do
    doc = Makiri::HTML("<div><p>t</p></div>")
    expect(doc.css("p:is()")).to be_empty
    expect(doc.css("p:where(:foo)")).to be_empty
    expect(doc.css("p:not(:is())").size).to eq(1)
    expect(Makiri::XML("<r><p/></r>").css("p:not(:where())").size).to eq(1)
  end
  # C19: :has() takes an unforgiving <relative-selector-list>.
  # C20: :has() is not valid within :has().
  it "reads :has() as unforgiving and refuses a :has() inside one" do
    doc = Makiri::HTML("<div><p><span></span></p></div>")
    [":has(p, :foo)", ":has(p,)", ":has(:has(span))", ":has(p:has(span))"].each do |sel|
      expect { doc.css(sel) }.to raise_error(Makiri::CSS::SyntaxError)
    end
    expect(doc.css("div:is(:has(p))").size).to eq(1)
  end
  # C4: consume a string token, U+005C at EOF: do nothing.
  it "adds nothing for a backslash that ends the input inside a string" do
    doc = Makiri::HTML(%(<p t="x"></p><p t="x&#xFFFD;"></p>))
    expect(doc.css(%([t="x\\)).map(&:to_html)).to eq([%(<p t="x"></p>)])
  end
end
