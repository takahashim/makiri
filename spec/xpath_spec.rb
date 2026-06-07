# frozen_string_literal: true

# M5: native XPath 1.0 engine. Covers axes, predicates, the built-in function
# library, node-set operations, document order (including attribute nodes),
# namespace/variable binding, and the fail-closed evaluation budgets.
RSpec.describe "Makiri XPath" do
  let(:doc) do
    Makiri::HTML(<<~HTML)
      <html><body>
        <div id="d" class="container">
          <section>
            <article>
              <p id="p1">first</p>
              <p id="p2" lang="en">second</p>
              <p id="p3">third</p>
            </article>
          </section>
          <a href="/one">one</a>
          <a href="/two">two</a>
        </div>
      </body></html>
    HTML
  end

  describe "result types" do
    it "returns a NodeSet for node-set expressions" do
      result = doc.xpath("//p")
      expect(result).to be_a(Makiri::NodeSet)
      expect(result.map(&:text)).to eq(%w[first second third])
    end

    it "returns a Float for number expressions" do
      expect(doc.xpath("count(//p)")).to eq(3.0)
    end

    it "returns a String for string expressions" do
      expect(doc.xpath("string(//p)")).to eq("first")
    end

    it "returns a boolean for boolean expressions" do
      expect(doc.xpath("count(//p) = 3")).to be(true)
      expect(doc.xpath("//x")).to be_a(Makiri::NodeSet) # empty set, not false
      expect(doc.xpath("boolean(//x)")).to be(false)
    end

    it "formats numbers as strings per XPath 1.0 (string(number))" do
      # Integers print without a fraction, no exponent; the special values use
      # their XPath spellings. These are the classic conversion edge cases.
      expect(doc.xpath("string(42)")).to eq("42")
      expect(doc.xpath("string(0)")).to eq("0")
      expect(doc.xpath("string(-7)")).to eq("-7")
      expect(doc.xpath("string(1.5)")).to eq("1.5")
      expect(doc.xpath("string(1 div 3)")).to eq("0.333333333333333")
      expect(doc.xpath("string(0 div 0)")).to eq("NaN")
      expect(doc.xpath("string(1 div 0)")).to eq("Infinity")
      expect(doc.xpath("string(-1 div 0)")).to eq("-Infinity")
    end

    it "converts booleans to/from strings and numbers" do
      expect(doc.xpath("string(true())")).to eq("true")
      expect(doc.xpath("string(false())")).to eq("false")
      expect(doc.xpath("number(true())")).to eq(1.0)
      expect(doc.xpath("number(false())")).to eq(0.0)
    end
  end

  describe "#at_xpath" do
    it "returns the first matching node" do
      expect(doc.at_xpath("//p").text).to eq("first")
    end

    it "returns nil when nothing matches" do
      expect(doc.at_xpath("//blockquote")).to be_nil
    end
  end

  describe "axes" do
    let(:p2) { doc.at_xpath('//*[@id="p2"]') }

    it "child / descendant" do
      expect(doc.xpath("//article/p").length).to eq(3)
      expect(doc.at_xpath("/html/body/div/section").name).to eq("section")
    end

    it "ancestor (reverse axis, surfaced in document order)" do
      expect(p2.xpath("ancestor::*").map(&:name)).to eq(%w[html body div section article])
      expect(p2.xpath("ancestor::*[1]").first.name).to eq("article")
    end

    it "ancestor-or-self" do
      expect(p2.xpath("ancestor-or-self::*").map(&:name))
        .to eq(%w[html body div section article p])
    end

    it "parent / self via abbreviations" do
      expect(p2.at_xpath("..").name).to eq("article")
      expect(p2.at_xpath(".").text).to eq("second")
    end

    it "following-sibling / preceding-sibling" do
      p1 = doc.at_xpath('//*[@id="p1"]')
      p3 = doc.at_xpath('//*[@id="p3"]')
      expect(p1.xpath("following-sibling::p").map { |n| n["id"] }).to eq(%w[p2 p3])
      expect(p3.xpath("preceding-sibling::p[1]").first["id"]).to eq("p2")
    end

    it "following / preceding skip descendants and ancestors" do
      flat = Makiri::HTML("<html><body><span></span><b><i></i></b><div></div><em></em></body></html>")
      expect(flat.at_xpath("//b").xpath("following::*").map(&:name)).to eq(%w[div em])
      expect(flat.at_xpath("//div").xpath("preceding::*").map(&:name)).to eq(%w[head span b i])
    end

    it "attribute axis yields Attribute nodes" do
      attrs = doc.xpath("//div/@*")
      expect(attrs).to all(be_a(Makiri::Attr)) # HTML::Attr leaf, is_a? Attr
      expect(attrs.map(&:name)).to eq(%w[id class])
    end
  end

  describe "predicates" do
    it "filters by attribute equality" do
      expect(doc.xpath('//p[@lang="en"]').map(&:text)).to eq(%w[second])
    end

    # The [@name] / [@name='lit'] shapes take a specialized direct-attribute
    # fast path; these lock in that it matches the generic evaluator exactly,
    # including the cases that must fall through.
    describe "attribute predicate fast path" do
      let(:list) do
        Makiri::HTML(%(<html><body><ul>) +
          %(<li id="a" class="x" data-r="1">A</li>) +
          %(<li class="y">B</li>) +
          %(<li data-r="2" lang="en">C</li>) +
          %(<li class="">D</li></ul></body></html>))
      end

      def ids(set) = set.map { |n| n["id"] || n.text }

      it "existence [@name]" do
        expect(ids(list.xpath("//li[@class]"))).to eq(%w[a B D])
        expect(list.xpath("//li[@missing]")).to be_empty
      end

      it "equality [@name='value'] including empty value" do
        expect(ids(list.xpath('//li[@class="x"]'))).to eq(%w[a])
        expect(ids(list.xpath('//li[@class=""]'))).to eq(%w[D])
        expect(ids(list.xpath('//li[@data-r="2"]'))).to eq(%w[C])
        expect(list.xpath('//li[@class="wrong"]')).to be_empty
      end

      it "single- and double-quoted literals are equivalent" do
        expect(list.xpath("//li[@class='x']").length).to eq(1)
      end

      it "matches attribute names case-sensitively (like the attribute axis)" do
        # The fast path must not inherit Lexbor's case-insensitive HTML
        # attribute lookup: //li[@Class] would then match `class`, diverging
        # from XPath 1.0, Nokogiri::HTML5, and Makiri's own //@Class axis test.
        expect(list.xpath("//li[@Class]")).to be_empty
        expect(list.xpath('//li[@Class="x"]')).to be_empty
        expect(list.xpath("//li[@class]").length).to eq(3) # exact case still matches
      end

      it "falls through for shapes that are not a plain attribute test" do
        expect(ids(list.xpath("//li[not(@class)]"))).to eq(%w[C])
        expect(ids(list.xpath('//li[@class="x" and @data-r]'))).to eq(%w[a])
        # positional predicates combined with an attribute test stay correct
        expect(ids(list.xpath("//li[@data-r][1]"))).to eq(%w[a])
        expect(ids(list.xpath("//li[1][@data-r]"))).to eq(%w[a])
      end
    end

    it "filters by position" do
      expect(doc.xpath("//article/p[2]").first.text).to eq("second")
      expect(doc.xpath("//article/p[last()]").first.text).to eq("third")
      expect(doc.xpath("//article/p[position() < 3]").map(&:text)).to eq(%w[first second])
    end

    it "filters by sub-expression existence" do
      expect(doc.xpath("//p[@id]").length).to eq(3)
      expect(doc.xpath("//p[@lang]").length).to eq(1)
    end
  end

  describe "node-set functions" do
    it "count / name / local-name" do
      expect(doc.xpath("count(//a)")).to eq(2.0)
      expect(doc.xpath("name(//div)")).to eq("div")
      expect(doc.xpath("local-name(//div)")).to eq("div")
    end

    it "id()" do
      expect(doc.at_xpath('id("p2")').text).to eq("second")
    end
  end

  describe "string functions" do
    it "concat / contains / starts-with" do
      expect(doc.xpath('concat("a", "b", "c")')).to eq("abc")
      expect(doc.xpath('contains("hello", "ell")')).to be(true)
      expect(doc.xpath('starts-with("hello", "he")')).to be(true)
    end

    it "substring (1-based, character-counted)" do
      expect(doc.xpath('substring("12345", 2, 3)')).to eq("234")
      expect(doc.xpath('substring-before("a/b", "/")')).to eq("a")
      expect(doc.xpath('substring-after("a/b", "/")')).to eq("b")
    end

    it "substring handles Infinity / NaN / huge positions (no UB)" do
      expect(doc.xpath('substring("12345", 1 div 0)')).to eq("")          # start = +Inf
      expect(doc.xpath('substring("12345", -1 div 0, 1 div 0)')).to eq("") # end = NaN
      expect(doc.xpath('substring("12345", 1e309)')).to eq("")            # overflows to +Inf
      expect(doc.xpath('substring("12345", 2, 1 div 0)')).to eq("2345")   # length = +Inf
      expect(doc.xpath('substring("12345", -1 div 0)')).to eq("12345")    # start = -Inf
    end

    it "string-length / normalize-space / translate" do
      expect(doc.xpath('string-length("héllo")')).to eq(5.0) # counts code points
      expect(doc.xpath('normalize-space("  a   b ")')).to eq("a b")
      expect(doc.xpath('translate("bar", "abc", "ABC")')).to eq("BAr")
    end

    it "translate operates on code points, not bytes (non-ASCII to/from)" do
      expect(doc.xpath('translate("a", "a", "é")')).to eq("é")
      expect(doc.xpath('translate("abc", "b", "ü")')).to eq("aüc")
      expect(doc.xpath('translate("héllo", "é", "e")')).to eq("hello")
      expect(doc.xpath('translate("abcd", "bd", "x")')).to eq("axc") # surplus 'from' dropped
    end
  end

  describe "number functions and arithmetic" do
    it "computes arithmetic with XPath precedence" do
      expect(doc.xpath("1 + 2 * 3")).to eq(7.0)
      expect(doc.xpath("(1 + 2) * 3")).to eq(9.0)
      expect(doc.xpath("7 mod 3")).to eq(1.0)
      expect(doc.xpath("floor(1.7)")).to eq(1.0)
      expect(doc.xpath("ceiling(1.2)")).to eq(2.0)
      expect(doc.xpath("round(2.5)")).to eq(3.0)
    end

    it "sum over a node-set" do
      nums = Makiri::HTML("<html><body><n>1</n><n>2</n><n>3</n></body></html>")
      expect(nums.xpath("sum(//n)")).to eq(6.0)
    end
  end

  describe "boolean operators" do
    it "and / or / not with short-circuit" do
      expect(doc.xpath("true() and false()")).to be(false)
      expect(doc.xpath("true() or false()")).to be(true)
      expect(doc.xpath("not(//x)")).to be(true)
    end
  end

  describe "node-set vs node-set comparison (XPath 1.0 §3.4)" do
    # True iff SOME pair of nodes - one from each set - satisfies the relation,
    # so every pair must be considered (not just the first node of each side).
    let(:ns) { Makiri::HTML("<body><a>3</a><b>2</b><b>4</b><a>5</a></body>") }

    it "relational compares all pairs" do
      expect(ns.xpath("//a < //b")).to be(true)   # 3 < 4
      expect(ns.xpath("//a <= //b")).to be(true)
      expect(ns.xpath("//a > //b")).to be(true)   # 5 > 2
      expect(ns.xpath("//a >= //b")).to be(true)
    end

    it "equality compares all pairs; empty set never satisfies" do
      expect(ns.xpath("//a = //b")).to be(false)  # {3,5} vs {2,4}: no common value
      expect(ns.xpath("//a != //b")).to be(true)
      expect(ns.xpath("//none < //a")).to be(false)
    end
  end

  describe "union" do
    it "merges and returns document order regardless of operand order" do
      d = Makiri::HTML("<html><body><a>1</a><b>2</b><c>3</c></body></html>")
      expect(d.xpath("//c | //a | //b").map(&:name)).to eq(%w[a b c])
      expect(d.xpath("(//b | //a)[1]").first.name).to eq("a")
    end
  end

  describe "document order with attribute nodes" do
    it "places an attribute before its element's descendants" do
      d = Makiri::HTML('<html><body><p id="x" class="c">A</p></body></html>')
      p = d.at_xpath('//p[@id="x"]')
      kinds = p.xpath("@class | text()").map do |n|
        n.is_a?(Makiri::Attr) ? :attr : :text
      end
      expect(kinds).to eq(%i[attr text])
    end

    # XPath 1.0 §5.1: "element, then its attribute nodes, then its children."
    # When a union yields an element together with its OWN attributes, the
    # element node must sort first. (Regression: the small-node-set fallback
    # comparator placed the attribute before its owner element, disagreeing
    # with both the spec and the indexed comparator used for larger sets.)
    it "places an element before its own attribute nodes" do
      d = Makiri::HTML('<html><body><p id="x" class="c">A</p></body></html>')
      kinds = d.xpath("//p | //p/@id | //p/@class").map do |n|
        n.is_a?(Makiri::Attr) ? :"@#{n.name}" : n.name.to_sym
      end
      expect(kinds).to eq(%i[p @id @class])
    end

    it "navigates from an attribute back to its element" do
      expect(doc.at_xpath("//a/@href/parent::a").name).to eq("a")
      expect(doc.xpath("//p/ancestor::div/@id").map(&:value)).to eq(%w[d])
    end
  end

  describe "XPathContext" do
    it "binds namespaces" do
      svg = Makiri::HTML(%(<html><body><svg xmlns="http://www.w3.org/2000/svg"><circle/></svg></body></html>))
      ctx = Makiri::XPathContext.new(svg)
      ctx.register_namespace("svg", "http://www.w3.org/2000/svg")
      expect(ctx.evaluate("//svg:circle").length).to eq(1)

      wrong = Makiri::XPathContext.new(svg)
      wrong.register_ns("svg", "urn:nope")
      expect(wrong.evaluate("//svg:circle").length).to eq(0)
    end

    it "binds variables" do
      ctx = Makiri::XPathContext.new(doc)
      ctx.register_variable("want", "en")
      expect(ctx.evaluate("//p[@lang=$want]").map(&:text)).to eq(%w[second])
    end

    it "accepts a valid multibyte variable value" do
      ctx = Makiri::XPathContext.new(doc)
      ctx.register_variable("v", "héllo")
      expect(ctx.evaluate("string-length($v)")).to eq(5.0)
    end

    it "rejects an invalid-UTF-8 variable value (fail closed)" do
      ctx = Makiri::XPathContext.new(doc)
      expect { ctx.register_variable("v", "\xC3".dup.force_encoding("BINARY")) }
        .to raise_error(Makiri::Error)
    end

    it "rejects a variable value with an embedded NUL (fail closed)" do
      ctx = Makiri::XPathContext.new(doc)
      expect { ctx.register_variable("v", "a\u0000b") }
        .to raise_error(Makiri::Error)
    end

    it "caps the number of registered namespaces (fail closed)" do
      ctx = Makiri::XPathContext.new(doc)
      expect do
        70_000.times { |i| ctx.register_namespace("p#{i}", "urn:#{i}") }
      end.to raise_error(Makiri::Error)
    end
  end

  describe "namespace matching (strict default / lax opt-in)" do
    let(:mixed) do
      Makiri::HTML("<body><div>a</div><div>b</div><svg><path/><title>s</title></svg>" \
                   "<math><mi>x</mi></math></body>")
    end

    it "strict (default): unprefixed names resolve in the HTML namespace" do
      # HTML elements match unprefixed...
      expect(mixed.xpath("//div").length).to eq(2)
      # ...but foreign (SVG/MathML) elements do NOT (browser / Nokogiri::HTML5 behaviour)
      expect(mixed.xpath("//path")).to be_empty
      expect(mixed.xpath("//svg")).to be_empty
      expect(mixed.xpath("//mi")).to be_empty
    end

    it "strict: the '*' wildcard still matches any namespace" do
      # html, head, body, 2×div, svg, path, svg-title, math, mi = 10 elements,
      # foreign ones included - the wildcard is namespace-agnostic by design.
      expect(mixed.xpath("//*").length).to eq(10)
      # foreign children are reachable via the wildcard even in strict mode
      expect(mixed.xpath("//*[local-name()='svg']/*").length).to eq(2) # path + title
    end

    it "strict: foreign content is reachable via a registered prefix" do
      ctx = Makiri::XPathContext.new(mixed)
      ctx.register_namespace("svg", "http://www.w3.org/2000/svg")
      expect(ctx.evaluate("//svg:path").length).to eq(1)
      expect(ctx.evaluate("//svg:title").map(&:text)).to eq(["s"]) # not the HTML sense
    end

    it "supports the prefix:* name test (any element in a namespace)" do
      ctx = Makiri::XPathContext.new(mixed)
      ctx.register_namespace("svg", "http://www.w3.org/2000/svg")
      # svg, path, title are all in the SVG namespace
      expect(ctx.evaluate("//svg:*").map(&:name)).to eq(%w[svg path title])
      expect { mixed.xpath("//bogus:*") }.to raise_error(Makiri::Error, /unknown namespace prefix/i)
    end

    it "lax: unprefixed names match by local name regardless of namespace" do
      expect(mixed.xpath("//path", namespace_matching: :lax).length).to eq(1)
      expect(mixed.xpath("//svg",  namespace_matching: :lax).length).to eq(1)
      expect(mixed.xpath("//mi",   namespace_matching: :lax).length).to eq(1)
    end

    it "lax can be set once on an XPathContext" do
      ctx = Makiri::XPathContext.new(mixed, namespace_matching: :lax)
      expect(ctx.evaluate("//path").length).to eq(1)
      expect(Makiri::XPathContext.new(mixed).evaluate("//path")).to be_empty # default strict
    end

    it "rejects an invalid namespace_matching value" do
      expect { mixed.xpath("//div", namespace_matching: :nope) }
        .to raise_error(ArgumentError, /namespace_matching/)
    end
  end

  describe "non-ASCII names (XML NCName, not ASCII-only)" do
    # XPath 1.0 §3.7 builds NCName on the XML Name production, whose letters
    # span a large non-ASCII range - so an HTML element named "dØdd" is a valid
    # name test. (HTML can only create such elements when the tag starts with
    # an ASCII letter: "<dØdd>" is a tag, "<Ødd>" is text.)
    it "accepts and matches a non-ASCII element name test" do
      d = Makiri::HTML("<body><dØdd>x</dØdd><dödd>y</dödd></body>")
      expect(d.xpath("//dØdd").map(&:text)).to eq(["x"])
      expect(d.xpath("//dödd").map(&:text)).to eq(["y"]) # distinct non-ASCII letter
    end

    it "rejects a malformed UTF-8 byte sequence in a name (fail closed)" do
      # Makiri's text-input contract rejects invalid UTF-8 at the API boundary
      # (before lexing) with a Makiri::Error, so the engine only ever sees
      # well-formed input.
      expect { Makiri::HTML("<p>x</p>").xpath("//a\xC3") }
        .to raise_error(Makiri::Error, /valid UTF-8/)
    end

    it "rejects a string literal containing invalid UTF-8 (fail closed)" do
      # Rejected at the boundary, so every character-wise function (translate,
      # substring, string-length, ...) can assume well-formed input.
      bad = ('string-length("' + "\xC3" + '")').b
      expect { Makiri::HTML("<p>x</p>").xpath(bad) }
        .to raise_error(Makiri::Error, /valid UTF-8/)
    end
  end

  describe "errors (fail closed)" do
    it "raises SyntaxError on malformed expressions" do
      expect { doc.xpath("//p[") }.to raise_error(Makiri::XPath::SyntaxError)
      expect { doc.xpath("1 +") }.to raise_error(Makiri::XPath::SyntaxError)
    end

    it "raises on an unknown namespace prefix" do
      expect { doc.xpath("//foo:bar") }
        .to raise_error(Makiri::Error, /unknown namespace prefix/i)
    end

    it "raises on an undefined variable" do
      expect { doc.xpath("//p[@id=$missing]") }
        .to raise_error(Makiri::Error, /undefined variable/i)
    end

    it "reports the namespace axis as not implemented (never silently empty)" do
      expect { doc.xpath("namespace::*") }
        .to raise_error(Makiri::Error, /not implemented/i)
    end
  end

  describe "evaluation budgets" do
    let(:small) { Makiri::HTML("<html><body><p>hi</p></body></html>") }

    it "rejects an over-long expression" do
      expect { small.xpath("/#{"*" * 100_000}") }
        .to raise_error(Makiri::XPath::LimitExceeded, /expression|too long/i)
    end

    it "rejects too many path steps" do
      expect { small.xpath("/a" * 300) }
        .to raise_error(Makiri::XPath::LimitExceeded, /step count/i)
    end

    it "rejects too many predicates on a step" do
      expect { small.xpath("//a#{"[1]" * 100}") }
        .to raise_error(Makiri::XPath::LimitExceeded, /predicate count/i)
    end

    it "rejects too many function arguments" do
      args = (["\"x\""] * 100).join(",")
      expect { small.xpath("concat(#{args})") }
        .to raise_error(Makiri::XPath::LimitExceeded, /function argument count/i)
    end

    it "rejects deeply nested parentheses (recursion depth)" do
      expr = "#{"(" * 300}1#{")" * 300}"
      expect { small.xpath(expr) }
        .to raise_error(Makiri::XPath::LimitExceeded, /recursion depth/i)
    end

    it "fails closed when a node string-value exceeds the byte cap" do
      # string(.) of a >64MB text node must raise, not return a truncated/empty
      # string (the builder must trip the limit, not stop short of it).
      big = Makiri::HTML("<p>#{"x" * (65 * 1024 * 1024)}</p>")
      expect { big.xpath("string(//p)") }
        .to raise_error(Makiri::XPath::LimitExceeded, /string size limit/i)
    end

    it "caps translate() output (multibyte expansion past the limit)" do
      # 17MB of "a" replaced by a 4-byte emoji blows past 64MB even though the
      # input is within the limit.
      doc = Makiri::HTML("<p>#{"a" * (17 * 1024 * 1024)}</p>")
      expect { doc.xpath('translate(string(//p), "a", "😀")') }
        .to raise_error(Makiri::XPath::LimitExceeded, /string size limit/i)
    end
  end

  describe "XPathContext AST cache" do
    it "returns identical results when the same expression is reused" do
      ctx = Makiri::XPathContext.new(doc)
      3.times { expect(ctx.evaluate("//p").map(&:text)).to eq(%w[first second third]) }
      expect(ctx.evaluate("count(//p)")).to eq(3.0)
    end

    it "stays correct across many distinct expressions" do
      ctx = Makiri::XPathContext.new(doc)
      60.times { |i| ctx.evaluate("//p[#{(i % 3) + 1}]") }
      expect(ctx.evaluate("//p").length).to eq(3)
    end
  end

  describe "memory safety" do
    it "stays correct under GC stress and compaction" do
      GC.stress = true
      begin
        nodes = doc.xpath("//p").to_a
        expect(nodes.map(&:text)).to eq(%w[first second third])
        GC.compact
        expect(doc.at_xpath('//*[@id="p2"]').text).to eq("second")
      ensure
        GC.stress = false
      end
    end

    it "survives many independent queries across dropped documents" do
      200.times do |i|
        d = Makiri::HTML("<html><body><p class='c#{i}'>#{i}</p></body></html>")
        expect(d.xpath("//p[@class='c#{i}']").first.text).to eq(i.to_s)
      end
      GC.start
    end
  end

  # `//tag` (a document-rooted descendant name-test) is served from the
  # document's element index instead of a tree walk. These assert it matches
  # the walk exactly, including the cases that must NOT take the fast path.
  describe "descendant tag fast path (element index)" do
    let(:doc) do
      Makiri::HTML(<<~HTML)
        <html><body>
          <ul><li id="a">a</li><li id="b">b</li></ul>
          <div><li id="c">nested</li></div>
          <p>p1</p>
        </body></html>
      HTML
    end

    it "returns every matching element in document order" do
      expect(doc.xpath("//li").map { |n| n["id"] }).to eq(%w[a b c])
    end

    it "returns empty for a standard tag that is absent (no walk needed)" do
      expect(doc.xpath("//table")).to be_empty
    end

    it "returns empty for an unknown tag name" do
      expect(doc.xpath("//nosuchtag")).to be_empty
    end

    it "still uses the child axis correctly (not the fast path)" do
      # //ul/li must exclude the <li> nested under <div>.
      expect(doc.xpath("//ul/li").map { |n| n["id"] }).to eq(%w[a b])
    end

    it "applies predicates on a descendant tag step" do
      # //li[1] = the first li *child within each parent* (a in <ul>, c in
      # <div>), not the first li in the document - and predicates disable the
      # fast path, so this also checks the generic step still works.
      expect(doc.xpath("//li[1]").map { |n| n["id"] }).to eq(%w[a c])
      expect(doc.xpath("//li[@id='c']").map(&:text)).to eq(%w[nested])
    end

    it "handles a relative descendant from a sub-context (not document-rooted)" do
      ul = doc.at_xpath("//ul")
      expect(ul.xpath(".//li").map { |n| n["id"] }).to eq(%w[a b])
    end

    it "finds custom-element tags via the tree-walk fallback" do
      d = Makiri::HTML("<html><body><my-widget>w1</my-widget><my-widget>w2</my-widget></body></html>")
      expect(d.xpath("//my-widget").map(&:text)).to eq(%w[w1 w2])
    end

    it "matches the case-sensitivity of the generic walk" do
      # HTML qualified names are lowercase; an upper-case test matches nothing.
      expect(doc.xpath("//LI")).to be_empty
    end

    it "reflects mutations (the index is invalidated)" do
      expect(doc.xpath("//li").length).to eq(3)
      doc.at_xpath("//ul").add_child(doc.create_element("li"))
      expect(doc.xpath("//li").length).to eq(4)
      doc.at_xpath("//li[@id='a']").remove
      # Document order: <ul>'s remaining li (b) and the appended li (nil id)
      # come before <div>'s li (c).
      expect(doc.xpath("//li").map { |n| n["id"] }).to eq(["b", nil, "c"])
    end
  end
end
