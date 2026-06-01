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
      expect(attrs.map(&:class)).to all(eq(Makiri::Attribute))
      expect(attrs.map(&:name)).to eq(%w[id class])
    end
  end

  describe "predicates" do
    it "filters by attribute equality" do
      expect(doc.xpath('//p[@lang="en"]').map(&:text)).to eq(%w[second])
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

    it "string-length / normalize-space / translate" do
      expect(doc.xpath('string-length("héllo")')).to eq(5.0) # counts code points
      expect(doc.xpath('normalize-space("  a   b ")')).to eq("a b")
      expect(doc.xpath('translate("bar", "abc", "ABC")')).to eq("BAr")
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
        n.is_a?(Makiri::Attribute) ? :attr : :text
      end
      expect(kinds).to eq(%i[attr text])
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
end
