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

  # `:text` is CSSOM's serialization (`lexbor::selector_text`): identifiers and
  # strings are escaped, so the text means what the selector did. Lexbor's own
  # serializer wrote names decoded - `.md\:block` as `.md:block` - and a caller
  # re-parsing the text (dommy's cascade) lost or misapplied the rule.
  describe "selector text" do
    def texts(css)
      rule = parse("#{css}{x:y}").first
      expect(rule[:type]).to eq(:style), css
      rule[:selectors].map { |s| s[:text] }
    end

    {
      '.md\:block' => '.md\:block',
      '.hover\:bg-red:hover' => '.hover\:bg-red:hover',
      '.w-1\/2' => '.w-1\/2',
      '.\[mask-type\:alpha\]' => '.\[mask-type\:alpha\]',
      '.\31 0' => '.\31 0',
      '.\@container' => '.\@container',
      '.\!important' => '.\!important',
      '.a\ b' => '.a\ b',
      '#x\.y' => '#x\.y',
      '.a\#b' => '.a\#b',
      '.a\+b' => '.a\+b',
      '.a\,b' => '.a\,b',
      '.\-\-x' => ".--x",
      '.-\31' => '.-\31 ',
      'd\69 v' => "div",
      '[data-x="a\"b"]' => '[data-x="a\"b"]',
      '[a="x\\\\y"]' => '[a="x\\\\y"]',
      '[a="a\a b"]' => '[a="a\a b"]',
      '[a="x" i]' => '[a="x" i]',
      ':is(.md\:block)' => ':is(.md\:block)',
      ':not(#x\.y)' => ':not(#x\.y)',
      ':has(> .a\,b)' => ':has(> .a\,b)',
      ':nth-child(2n+1 of .a\:b)' => ':nth-child(odd of .a\:b)',
      ':current(.a\:b)' => ':current(.a\:b)',
    }.each do |css, text|
      it "writes #{css} as #{text}, which reads back the same" do
        expect(texts(css)).to eq([text])
        expect(texts(text)).to eq([text])
        orig = parse("#{css}{x:y}")[0][:selectors][0][:specificity]
        expect(parse("#{text}{x:y}")[0][:selectors][0][:specificity]).to eq(orig)
      end
    end

    it "keeps an escaped comma or combinator inside one selector" do
      expect(texts('.a\,b, .a\+b, .a\ b')).to eq(['.a\,b', '.a\+b', '.a\ b'])
    end

    it "gives text that matches the elements the original selector matches" do
      doc = Makiri::HTML(<<~HTML)
        <div class="md:block w-1/2 10 a,b a+b" id="x.y" data-x='a"b' data-y="x\\y">
          <p class="a b hidden md:block">t</p><b>u</b>
        </div>
      HTML
      ['.md\:block', '.w-1\/2', '.\31 0', '.a\,b', '.a\+b', '#x\.y', '.a\ b',
       '[data-x="a\"b"]', '[data-y="x\\\\y"]', ':is(.md\:block) > p', 'div:has(> .md\:block)',
       ':not(.md\:block)'].each do |css|
        text = texts(css).first
        expect(doc.css(text).to_a).to eq(doc.css(css).to_a), "#{css} -> #{text}"
      end
    end

    it "writes pseudo-class names in Lexbor's lower case" do
      expect(texts(":HOVER, :IS(A), :Nth-Child(2)")).to eq([":hover", ":is(A)", ":nth-child(2)"])
    end

    it "keeps the column combinator" do
      expect(texts("col || td")).to eq(["col || td"])
    end

    # Lexbor stores `[|a]` (no namespace) with the namespace `*` and refuses
    # `[*|a]`, so a `*` there is `[|a]`. It used to be reported as :bad_style.
    # An escaped `\*|` prefix is stored the same way and reads back as `|`,
    # as `\*|a` reads back as `*|a` on a type selector.
    it "writes an attribute in no namespace back as [|a]" do
      rules = parse(".x{color:red} [|a] { color: blue } .y{color:green}")
      expect(rules.map { |r| r[:type] }).to eq(%i[style style style])
      expect(rules[1][:selectors]).to eq([{ text: "[|a]", specificity: [0, 1, 0] }])
      expect(texts("[|a=x], p[|a]")).to eq(['[|a="x"]', "p[|a]"])
      expect(texts('[\*|a]')).to eq(["[|a]"])
      expect(parse("[*|a]{x:y}")[0][:type]).to eq(:bad_style)
    end

    it "trims a :bad_style prelude" do
      expect(parse("  :focus-within  {x:y}")[0][:selector_text]).to eq(":focus-within")
      expect(parse(" \t ::before \n {x:y}")[0][:selector_text]).to eq("::before")
    end

    # Trimming used to drop every byte up to 0x20 and an escaped space with it,
    # leaving a lone `\` at the end - an escape at EOF, U+FFFD - so a prelude
    # Lexbor rejected came back as a different, valid selector.
    describe "trimming whitespace that an escape owns" do
      it "keeps an escaped trailing space" do
        expect(parse('::foo .a\  {x:y}')[0][:selector_text]).to eq('::foo .a\ ')
      end

      it "never leaves a lone trailing backslash" do
        expect(parse(".a\\\n{x:y}")[0][:selector_text]).to eq(".a\\\n")
        expect(parse("::foo .a\\\\ {x:y}")[0][:selector_text]).to eq("::foo .a\\\\")
      end

      it "keeps it in an at-rule prelude" do
        expect(parse('@media screen\  { a { x: y } }')[0][:prelude]).to eq('screen\ ')
      end

      it "keeps it in a value taken from the source" do
        decl = parse('a{--x: :lexbor-contains(1) \ ;}')[0][:declarations][0]
        expect(decl[:value]).to eq(':lexbor-contains(1) \ ')
      end

      it "trims only CSS whitespace" do
        expect(parse("::foo .a\x01 {x:y}")[0][:selector_text]).to eq("::foo .a\x01")
        expect(parse("::foo .a\v {x:y}")[0][:selector_text]).to eq("::foo .a\v")
        expect(parse("\t\f\r\n ::foo\t\f\r\n {x:y}")[0][:selector_text]).to eq("::foo")
      end
    end

    it "leaves selectors without escapes as Lexbor wrote them" do
      expect(texts("div.a, #b > span, a + b, a ~ b, ns|a, *|*, |a")).to eq(
        ["div.a", "#b > span", "a + b", "a ~ b", "ns|a", "*|*", "|a"]
      )
      expect(texts(":nth-child(2n), :nth-last-child(-n+3), :nth-of-type(5)")).to eq(
        [":nth-child(even)", ":nth-last-child(-n+3)", ":nth-of-type(5)"]
      )
    end
  end

  describe "input contract" do
    it "rejects a NUL byte" do
      expect { parse("a{}\0x") }.to raise_error(Makiri::Error, /NUL/)
    end

    # `:lexbor-contains()` is not supported: `lexbor::contains_guard` renames
    # every one, so the rule degrades the way a rule with any unknown
    # pseudo-class does and the rest of the sheet is unaffected.
    describe "a :lexbor-contains()" do
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

      it "is a bad rule even with a well-formed argument" do
        rules = parse(%(.a{color:red}:lexbor-contains("x"){color:blue}.b{color:green}))
        expect(rules.map { |r| r[:type] }).to eq(%i[style bad_style style])
        expect(rules[1][:selector_text]).to eq(%(:lexbor-contains("x")))
      end

      # A forgiving list drops it as it drops any unknown pseudo-class.
      it "is dropped from a forgiving list like any unknown pseudo-class" do
        [":is(.a, :lexbor-contains(\"x\"))", ":is(.a, :zz-unknown(\"x\"))"].each do |sel|
          rules = parse("#{sel}{color:blue}")
          expect(rules.map { |r| r[:type] }).to eq(%i[style]), sel
          expect(rules[0][:selectors].map { |s| s[:text] }).to eq([":is(.a)"]), sel
        end
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

      # The guard used to read strings with a scanner of its own that ended one
      # only at LF and gave up on the rest of the text at an unterminated one;
      # Lexbor's tokenizer ends a string at CR, FF or LF and reads on. So the
      # pseudo after such a string reached the parser, and serializing the rule
      # it left behind killed the process (Bus Error). Each runs in a child, so
      # a regression is a failed example rather than a dead suite.
      describe "after a string a newline ends (isolated)" do
        def parse_isolated(css)
          lib = File.expand_path("../lib", __dir__)
          code = "require 'makiri'\n" \
                 "p Makiri::Lexbor::CSS.parse_stylesheet(#{css.dump}).map { |r| r[:type] }"
          out = IO.popen([{ "RUBY_FREE_AT_EXIT" => nil }, RbConfig.ruby, "-I#{lib}", "-e", code],
                         err: %i[child out], &:read)
          [$?, out]
        end

        ["\r", "\n", "\r\n", "\f"].each do |nl|
          it "rejects the rule after a string ended by #{nl.dump}" do
            status, out = parse_isolated("a{content:\"x#{nl}}b:lexbor-contains(#x){color:red}")
            expect(status).to be_success, out
            expect(out).to eq("[:style, :bad_style]\n")
          end
        end

        it "reads a backslash-newline as a string continuation" do
          status, out = parse_isolated("a{content:\"x\\\n}b:lexbor-contains(#x){color:red}")
          expect(status).to be_success, out
          expect(out).to eq("[:style]\n") # the rest is string content

          status, out = parse_isolated("a{content:\"x\\\n\"}b:lexbor-contains(#x){color:red}")
          expect(status).to be_success, out
          expect(out).to eq("[:style, :bad_style]\n")
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
