# frozen_string_literal: true

# v0.3: XPath custom function handler (Ruby callable). Unknown functions are
# dispatched to a handler object; the XPath local name maps to a Ruby method
# with '-' replaced by '_'. Arguments and the return value are converted
# between engine and Ruby values, and handler exceptions become clean errors.
RSpec.describe "Makiri XPath custom function handler" do
  nokogiri_uri = "http://www.nokogiri.org/default_ns/ruby/extensions_functions"

  handler_class = Class.new do
    def thrice(str) = str.to_s * 3
    def my_count(nodes) = nodes.length * 1.0
    def is_paragraph(set) = set.first&.name == "p"
    def first_node(set) = set.first          # returns a Node
    def boom = raise("kaboom")
  end

  let(:doc) do
    Makiri::HTML("<html><body><p>hi</p><p>there</p><div>x</div></body></html>")
  end
  let(:ctx) do
    c = Makiri::XPathContext.new(doc)
    c.register_namespace("ng", nokogiri_uri)
    c
  end
  let(:handler) { handler_class.new }

  describe "dispatch and value conversion" do
    it "passes/returns strings" do
      expect(ctx.evaluate('ng:thrice("ab")', handler)).to eq("ababab")
    end

    it "converts node-set arguments and numeric returns" do
      expect(ctx.evaluate("ng:my-count(//p)", handler)).to eq(2.0)
    end

    it "accepts boolean returns" do
      expect(ctx.evaluate("ng:is-paragraph(//p)", handler)).to be(true)
    end

    it "accepts a Node return as a node-set" do
      result = ctx.evaluate("ng:first-node(//p)", handler)
      expect(result).to be_a(Makiri::NodeSet)
      expect(result.first.text).to eq("hi")
    end

    it "maps '-' in the function name to '_' in the method" do
      # my-count -> my_count is exercised above; verify the inverse fails clean.
      expect { ctx.evaluate("ng:mycount(//p)", handler) }
        .to raise_error(Makiri::Error, /unknown function/)
    end
  end

  describe "errors" do
    it "raises 'unknown function' when the handler lacks the method" do
      expect { ctx.evaluate("ng:no_such()", handler) }
        .to raise_error(Makiri::Error, /unknown function/)
    end

    it "raises 'unknown function' when no handler is supplied" do
      expect { ctx.evaluate('ng:thrice("x")') }
        .to raise_error(Makiri::Error, /unknown function/)
    end

    it "wraps a handler exception instead of crashing the evaluator" do
      expect { ctx.evaluate("ng:boom()", handler) }
        .to raise_error(Makiri::Error, /handler raised: kaboom/)
    end

    it "rejects a node returned from a different document" do
      foreign_doc = Makiri::HTML("<span>x</span>")
      foreign_handler = Class.new do
        def initialize(d) = (@d = d)
        def grab = @d.at_css("span")
      end.new(foreign_doc)
      expect { ctx.evaluate("ng:grab()", foreign_handler) }
        .to raise_error(Makiri::Error, /different document/)
    end
  end

  describe "Node#xpath with an unprefixed handler function" do
    it "dispatches without needing a namespace" do
      h = Class.new { def shout(set) = set.first.text.upcase }.new
      expect(doc.xpath("shout(//p)", h)).to eq("HI")
    end
  end

  describe "nested evaluation from a handler" do
    # A handler that re-enters XPath exercises the per-evaluate string-value
    # cache's snapshot / partial-truncate path (the cache is hashed for O(1)
    # lookup; nested evals must restore it correctly).
    it "evaluates correctly and keeps the outer cache consistent" do
      list = Makiri::HTML("<html><body><ul>" \
        "#{(1..20).map { |i| %(<li class="item" data-r="#{i}">x</li>) }.join}</ul></body></html>")
      probe = Class.new do
        def initialize(d) = (@d = d)
        def probe(*) = @d.xpath('//li[@data-r="3"]').length * 1.0
      end.new(list)

      c = Makiri::XPathContext.new(list)
      c.register_namespace("ng", nokogiri_uri)

      # The outer comparison populates the cache per <li> before ng:probe()
      # runs a nested evaluate; all 20 satisfy class=item AND probe()==1.
      result = c.evaluate('//li[@class="item" and ng:probe() = 1]', probe)
      expect(result.length).to eq(20)
      # Cache reused correctly afterwards.
      expect(c.evaluate('count(//li[@class="item"])')).to eq(20.0)
    end
  end

  describe "handler-returned string validation (fail closed)" do
    let(:bad) do
      Class.new do
        def nul = "a\u0000b"                                # embedded NUL
        def invalid = "\xC3".dup.force_encoding("BINARY")   # invalid UTF-8
        def good = "héllo"                                  # valid multibyte
      end.new
    end

    it "rejects an embedded NUL instead of silently truncating" do
      expect { ctx.evaluate("ng:nul()", bad) }.to raise_error(Makiri::Error)
    end

    it "rejects invalid UTF-8" do
      expect { ctx.evaluate("string-length(ng:invalid())", bad) }
        .to raise_error(Makiri::Error)
    end

    it "accepts valid multibyte UTF-8" do
      expect(ctx.evaluate("string-length(ng:good())", bad)).to eq(5.0)
    end
  end

  describe "memory safety" do
    it "survives handler dispatch under GC stress" do
      GC.stress = true
      begin
        expect(ctx.evaluate("ng:my-count(//p)", handler)).to eq(2.0)
        GC.compact
        expect(ctx.evaluate('ng:thrice("z")', handler)).to eq("zzz")
      ensure
        GC.stress = false
      end
    end
  end
end
