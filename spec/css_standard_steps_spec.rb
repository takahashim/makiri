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
  # C5: would start an ident sequence, U+002D: true when the next code point
  # is an ident-start code point - a non-ASCII ident code point included.
  it "starts a name with a hyphen before a non-ASCII ident code point only" do
    doc = Makiri::HTML(%(<p class="-א"></p>))
    expect(doc.css(".-א").size).to eq(1)
    expect(Makiri::Lexbor::CSS.parse_stylesheet("@-א x {}").first[:name]).to eq("-א")
    expect(Makiri::Lexbor::CSS.parse_stylesheet("@-¶ x {}").first[:type]).to eq(:bad_style)
  end
  # C6: consume a number, step 6: the value is the whole digit run's.
  it "keeps the value of a number with more than 128 integer digits" do
    doc = Makiri::HTML("<ul><li>a<li>b</ul>")
    expect(doc.css("li:nth-child(#{"0" * 128}1)").map(&:text)).to eq(["a"])
  end
  # C8: consume an ident-like token: url( and whitespace, then no quote -
  # a URL token, which keeps the whitespace in a declaration's value.
  it "reads url( and whitespace to the end of the input as a URL token" do
    value = Makiri::Lexbor::CSS.parse_stylesheet("a{b:url(   ").first[:declarations].first[:value]
    expect(value).to eq("url(   ")
  end
  # C7: an ID selector is a <hash-token> whose type flag is "id".
  it "refuses a hash that does not start a name as an ID selector" do
    doc = Makiri::HTML(%(<p id="1a"></p>))
    expect { doc.css("#1a") }.to raise_error(Makiri::CSS::SyntaxError)
    expect { doc.css("p#-2") }.to raise_error(Makiri::CSS::SyntaxError)
    expect(doc.css('#\\31 a').size).to eq(1)
  end
  # C22: <ndashdigit-ident> has no bound on its digits; a B too large for a
  # long saturates, as the same value written as a number does.
  it "saturates an n- digit run too large for a long" do
    doc = Makiri::HTML("<ul><li>a<li>b</ul>")
    expect(doc.css("li:nth-child(n-99999999999999999999)").size).to eq(2)
    expect(doc.css("li:nth-child(n- 99999999999999999999)").size).to eq(2)
  end
  # C15: serialization round-trips; a <delim-token> holding "\" is written
  # as "\" followed by a newline.
  it "keeps the newline after a backslash delim in a declaration's value" do
    values = Makiri::Lexbor::CSS.parse_stylesheet("a{b: c\\\nd; e: x \\\n}").first[:declarations].map { |d| d[:value] }
    expect(values).to eq(["c\\\nd", "x \\\n"])
  end
  def declarations(css)
    Makiri::Lexbor::CSS.parse_stylesheet(css).first[:declarations].map { |d| [d[:value], d[:important]] }
  end

  # C12: consume a declaration, step 6: the last two tokens, whitespace and
  # comments aside, are "!" and an ident "important".
  it "finds !important on tokens" do
    expect(declarations("a { x: y ! important }")).to eq([["y", true]])
    expect(declarations("a{b: c !/**/important}")).to eq([["c", true]])
    expect(declarations("a{b: c !imp\\ortant}")).to eq([["c", true]])
  end

  # C13: an !important inside a block or function is the block's content.
  it "leaves an !important inside brackets in the value" do
    expect(declarations("a{b: (x !important; y); c:d}")).to eq([["(x !important; y)", false], ["d", false]])
  end
  # C9: consume a declaration, step 8: a top-level {}-block beside other
  # values is no declaration; consume a block's contents reads a rule.
  it "reads a:hover{...} in a block as a rule, not a declaration" do
    media = Makiri::Lexbor::CSS.parse_stylesheet("@media (max-width:600px){ .x{a:b} a:hover{color:red} .y{c:d} }").first
    expect(media[:rules].map { |r| r[:selectors].map { |s| s[:text] } }).to eq([[".x"], ["a:hover"], [".y"]])
  end

  # C14: comments are not tokens; a comment may stand before the colon.
  it "reads a declaration with a comment before its colon" do
    expect(declarations("a{x:y; c /**/ : d; e:f}").size).to eq(3)
  end
  # C10: consume a qualified rule, <EOF-token> / stop token / nested <}-token>:
  # return nothing.
  it "makes nothing of a qualified rule that ends without a block" do
    sheet = Makiri::Lexbor::CSS.parse_stylesheet("a{b:c} d")
    expect(sheet.map { |r| r[:selectors].map { |s| s[:text] } }).to eq([["a"]])
    expect(declarations("a { c: d; b; e: f }")).to eq([["d", false], ["f", false]])
  end

  # C11: consume a qualified rule, <{-token>: a top-level prelude starting
  # "--name:" consumes its block and returns nothing.
  it "makes nothing of a top-level rule whose prelude looks like a custom property" do
    sheet = Makiri::Lexbor::CSS.parse_stylesheet("--foo:hover { color: blue } a{b:c}")
    expect(sheet.map { |r| r[:selectors].map { |s| s[:text] } }).to eq([["a"]])
  end
  # C16: a block at-rule holds a list of declarations (@font-face, @page).
  it "returns the declarations in an at-rule's block" do
    rule = Makiri::Lexbor::CSS.parse_stylesheet("@font-face { font-family: x; src: url(a) }").first
    expect(rule[:declarations].map { |d| [d[:name], d[:value]] }).to eq([%w[font-family x], ["src", "url(a)"]])
    expect(Makiri::Lexbor::CSS.parse_stylesheet("@media print { a{b:c} }").first[:declarations]).to eq([])
  end

  # M8: Selectors 4, :nth-child(): "the element has An+B-1 siblings before
  # it" - element siblings, so a doctype or a processing instruction takes
  # no position.
  it "counts only element siblings for the :nth-child family" do
    doc = Makiri::HTML("<!doctype html><div><?pi x?><b>1</b><!--c--><i>2</i><?pi y?></div>")
    expect(doc.css("html:first-child").size).to eq(1)
    expect(doc.css("b:first-child").map(&:text)).to eq(["1"])
    expect(doc.css("i:last-child").map(&:text)).to eq(["2"])
    expect(doc.css("i:nth-child(2)").map(&:text)).to eq(["2"])
    expect(doc.at_css("b").matches?(":first-child")).to be(true)
  end

  # M2: HTML Standard, :link - "all a elements that have an href attribute,
  # and all area elements that have an href attribute"; SVG 2 adds its a.
  it "takes a and area with href, and SVG a, as links" do
    doc = Makiri::HTML(%(<!doctype html><link href=x><map href=y></map><a href=z>a</a><area href=w>
      <svg><a href=s></a><a xlink:href=t></a></svg>))
    expect(doc.css(":link").map { |e| e["href"] || e.attribute_nodes.first.value }).to eq(%w[z w s t])
    expect(doc.css(":any-link").size).to eq(4)
  end

  # M9: HTML Standard, :read-write - input elements to which the readonly
  # attribute applies and that are mutable, textarea elements without
  # readonly that are not disabled, and editing hosts or editable elements.
  it "decides :read-write by readonly's input types and by contenteditable" do
    doc = Makiri::HTML(%(<!doctype html><input id=t><input id=c type=checkbox><input id=n type=NUMBER>
      <input id=r type=email readonly><textarea id=x></textarea>
      <div id=h contenteditable><p id=p>a</p><span contenteditable=false><b id=f>b</b></span></div>))
    expect(doc.css(":read-write").map { |e| e["id"] }).to eq(%w[t n x h p])
    expect(doc.at_css("#c").matches?(":read-only")).to be(true)
    expect(doc.at_css("#f").matches?(":read-only")).to be(true)
  end

  # M10: :empty, as browsers count children - elements and text with data;
  # comments and processing instructions do not count.
  it "ignores comments and processing instructions for :empty" do
    doc = Makiri::HTML("<!doctype html><p id=a><?pi x?></p><p id=b><!--c--></p><p id=c> </p><p id=d></p>")
    expect(doc.css("p:empty").map { |e| e["id"] }).to eq(%w[a b d])
    xml = Makiri::XML("<r><e><![CDATA[]]></e><e><?pi x?></e><e>t</e></r>")
    expect(xml.css("e:empty").size).to eq(2)
  end

  # C28: <compound-selector> = [ <type-selector>? <subclass-selector>* ... ]:
  # a type selector, namespaced or not, only begins a compound.
  it "refuses a namespaced type selector in the middle of a compound" do
    doc = Makiri::HTML("<a class=x>t</a>")
    %w[|a|* .x|a [x]|a *|a|b].each do |sel|
      expect { doc.css(sel) }.to raise_error(Makiri::CSS::SyntaxError), sel
    end
    expect(doc.css("|a.x")).to be_empty
    expect(doc.css("*|a.x").size).to eq(1)
    expect(doc.css("body *|a").size).to eq(1)
    expect(Makiri::XML("<r><a/></r>").css("r >|a").size).to eq(1)
  end
end
