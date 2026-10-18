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
end
