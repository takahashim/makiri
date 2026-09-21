# frozen_string_literal: true

require "spec_helper"

# Makiri::Lexbor::CSS.parse_stylesheet is a thin binding over Lexbor's CSS
# stylesheet parser: it returns the parsed rules as plain Ruby primitives
# (Array / Hash / Symbol / String / Integer), with per-comma-branch [a, b, c]
# specificity, declaration name/value/important, and @media nesting. dommy
# consumes this in its CSS cascade layer (docs/css-cascade.md, Phase 0).
RSpec.describe Makiri::Lexbor::CSS do
  def parse(css)
    described_class.parse_stylesheet(css)
  end

  describe ".parse_stylesheet" do
    it "returns style rules with selectors and declarations in source order" do
      rules = parse("p { color: red } a { color: blue }")
      expect(rules.map { |r| r[:type] }).to eq([:style, :style])
      expect(rules[0][:selectors].map { |s| s[:text] }).to eq(["p"])
      expect(rules[0][:declarations])
        .to eq([{ name: "color", value: "red", important: false }])
      expect(rules[1][:selectors].map { |s| s[:text] }).to eq(["a"])
    end

    it "splits a comma selector list into one entry per branch" do
      rules = parse("div.a, #b > span { display: none }")
      texts = rules[0][:selectors].map { |s| s[:text] }
      expect(texts).to eq(["div.a", "#b > span"])
    end

    it "reports [a, b, c] specificity per branch" do
      rules = parse("div.a, #b > span { x: y }")
      specs = rules[0][:selectors].map { |s| s[:specificity] }
      expect(specs).to eq([[0, 1, 1], [1, 0, 1]])
    end

    it "computes :is/:not/:where specificity the way Selectors L4 does" do
      rules = parse(":where(.w) :is(.p, #q) { x: y }")
      # :where contributes 0; :is takes the max of its args (#q -> one id).
      expect(rules[0][:selectors][0][:specificity]).to eq([1, 0, 0])
    end

    it "flags !important declarations" do
      rules = parse("a { color: red !important; width: 1px }")
      decls = rules[0][:declarations]
      expect(decls).to eq([
        { name: "color", value: "red", important: true },
        { name: "width", value: "1px", important: false },
      ])
    end

    it "exposes custom properties by name" do
      rules = parse(".x { --tw-ring: 1px solid }")
      expect(rules[0][:declarations].first[:name]).to eq("--tw-ring")
      expect(rules[0][:declarations].first[:value]).to eq("1px solid")
    end

    it "normalizes declaration values through Lexbor" do
      # Lexbor reserializes a known property value in its canonical form.
      rules = parse("a { margin: 1px   2px }")
      expect(rules[0][:declarations].first[:value]).to eq("1px 2px")
    end
  end

  describe "at-rules" do
    it "surfaces @media uniformly with name, prelude (condition), and nested rules" do
      rules = parse("@media (min-width: 600px) { .x { opacity: 0 } }")
      expect(rules.size).to eq(1)
      media = rules[0]
      expect(media[:type]).to eq(:at_rule)
      expect(media[:name]).to eq("media")
      expect(media[:prelude]).to eq("(min-width: 600px)")
      expect(media[:rules].size).to eq(1)
      inner = media[:rules][0]
      expect(inner[:type]).to eq(:style)
      expect(inner[:selectors][0][:text]).to eq(".x")
      expect(inner[:declarations]).to eq([{ name: "opacity", value: "0", important: false }])
    end

    it "trims surrounding whitespace from the prelude" do
      rules = parse("@media   screen and (max-width: 5px)   { a { x: y } }")
      expect(rules[0][:prelude]).to eq("screen and (max-width: 5px)")
    end

    it "surfaces @layer with its name and nested rules" do
      rules = parse("@layer base { p { color: red } }")
      expect(rules[0][:name]).to eq("layer")
      expect(rules[0][:prelude]).to eq("base")
      expect(rules[0][:rules][0][:selectors][0][:text]).to eq("p")
    end

    it "surfaces @supports with its condition prelude" do
      rules = parse("@supports (display: grid) { .g { display: grid } }")
      expect(rules[0][:name]).to eq("supports")
      expect(rules[0][:prelude]).to eq("(display: grid)")
      expect(rules[0][:rules].size).to eq(1)
    end

    it "surfaces other at-rules (e.g. @keyframes, @import) without dropping them" do
      rules = parse("@keyframes spin { from { opacity: 0 } } @import url(x.css);")
      expect(rules.map { |r| r[:name] }).to eq(%w[keyframes import])
      expect(rules[0][:rules].size).to eq(1)   # the `from` keyframe block
      expect(rules[1][:rules]).to eq([])       # statement at-rule, no block
    end

    it "fails closed on pathologically deep nesting" do
      deep = ("@media screen {" * 70) + ".x{color:red}" + ("}" * 70)
      expect { parse(deep) }.to raise_error(Makiri::Error, /nesting too deep/)
    end
  end

  describe "error recovery (css-syntax-3)" do
    it "returns an empty array for an empty stylesheet" do
      expect(parse("")).to eq([])
      expect(parse("   \n\t  ")).to eq([])
    end

    it "surfaces an unknown at-rule (as :at_rule) and keeps a following good rule" do
      rules = parse("@unknown foo { x: y } .good { color: blue }")
      expect(rules.map { |r| r[:type] }).to eq(%i[at_rule style])
      expect(rules[0][:name]).to eq("unknown")
      expect(rules[1][:selectors][0][:text]).to eq(".good")
    end

    it "surfaces (does not drop) recognized at-rules like @font-face" do
      rules = parse("@font-face { font-family: x } .a { color: red }")
      expect(rules.map { |r| r[:type] }).to eq(%i[at_rule style])
      expect(rules[0][:name]).to eq("font-face")
      expect(rules[1][:selectors][0][:text]).to eq(".a")
    end

    it "surfaces a pseudo-element rule as :bad_style with raw text + declarations" do
      rules = parse('p::before { content: "x"; color: blue }')
      expect(rules.size).to eq(1)
      expect(rules[0][:type]).to eq(:bad_style)
      expect(rules[0][:selector_text].strip).to eq("p::before")
      expect(rules[0][:declarations]).to eq(
        [{ name: "content", value: '"x"', important: false },
         { name: "color", value: "blue", important: false }]
      )
    end

    it "surfaces a syntactically rejected selector as :bad_style too (caller re-validates)" do
      rules = parse("p:unknown-pseudo { color: red } .good { color: blue }")
      expect(rules.map { |r| r[:type] }).to eq([:bad_style, :style])
      expect(rules[0][:selector_text].strip).to eq("p:unknown-pseudo")
      expect(rules[1][:selectors][0][:text]).to eq(".good")
    end

    it "never raises a syntax error on broken input" do
      expect { parse("}}} garbage {{{") }.not_to raise_error
      expect(parse("}}} garbage {{{")).to be_an(Array)
    end
  end

  describe "input contract" do
    it "rejects a NUL byte" do
      expect { parse("a{}\0x") }.to raise_error(Makiri::Error, /NUL/)
    end

    # `lexbor::contains_guard` rewrites these, so the rule degrades the way any
    # unparseable selector does and the rest of the sheet is unaffected.
    describe "a :lexbor-contains() Lexbor would reject" do
      it "degrades to :bad_style and leaves the rest of the sheet standing" do
        rules = parse(".a{color:red}:lexbor-contains(#x){color:blue}.b{color:green}")
        expect(rules.map { |r| r[:type] }).to eq(%i[style bad_style style])
        expect(rules[0][:selectors].map { |s| s[:text] }).to eq([".a"])
        expect(rules[2][:selectors].map { |s| s[:text] }).to eq([".b"])
      end

      it "reports the selector the caller wrote, not the rewritten name" do
        rules = parse(".a{color:red}:lexbor-contains(#x){color:blue}")
        expect(rules[1][:selector_text]).to eq(":lexbor-contains(#x)")
        expect(rules[1][:declarations])
          .to eq([{ name: "color", value: "blue", important: false }])
      end

      it "keeps a well-formed :lexbor-contains() working" do
        rules = parse(%(.a{color:red}:lexbor-contains("x"){color:blue}.b{color:green}))
        expect(rules.map { |r| r[:type] }).to eq(%i[style style style])
        expect(rules[1][:selectors].map { |s| s[:text] }).to eq([%(:lexbor-contains("x"))])
      end

      it "sees through the identifier escapes Lexbor decodes" do
        # None of these contain the substring "lexbor-contains".
        [%q(:lexbor\\-contains(#x)), %q(:\\6C exbor-contains(#x)), ":LEXBOR-CONTAINS(#x)"]
          .each do |sel|
            rules = parse("#{sel}{color:blue}.b{color:green}")
            expect(rules.map { |r| r[:type] }).to eq(%i[bad_style style]), sel
          end
      end

      it "handles many rejected rules interleaved with good ones" do
        # An UNCLOSED `(` swallows the rest of the sheet - ordinary css-syntax-3
        # error recovery, and the same with any unknown pseudo - so only the
        # rules before it are asserted to survive.
        bad = [":lexbor-contains(#x)", ":lexbor-contains()", ":lexbor-contains(*)",
               %(:lexbor-contains("s" junk)), ":lexbor-contains("]
        40.times do |i|
          sel = bad[i % bad.length]
          css = ".a#{'x' * (i % 37 + 1)}{color:red}" \
                "#{sel}{color:blue}" \
                ".b#{'y' * (i % 53 + 1)}{color:green}"
          types = parse(css).map { |r| r[:type] }
          expect(types.first).to eq(:style), css
          expect(types.last).to eq(:style), css unless sel.end_with?("(")
        end
      end
    end

    it "rejects invalid UTF-8" do
      expect { parse("a { x: y }".dup.force_encoding("UTF-8") + 255.chr) }
        .to raise_error(Makiri::Error, /UTF-8/)
    end

    it "coerces a non-String argument" do
      expect(parse(:".a { color: red }")).to be_an(Array)
    end

    it "returns UTF-8-encoded strings" do
      rules = parse(".café { content: x }")
      expect(rules[0][:selectors][0][:text].encoding).to eq(Encoding::UTF_8)
    end
  end
end
