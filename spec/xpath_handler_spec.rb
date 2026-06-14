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
    def type_of(arg) = arg.class.name        # echoes the Ruby type of the arg
    def echo(arg) = arg                       # round-trips the arg back
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

    it "passes number and boolean arguments to the handler" do
      # number arg -> Ruby Float, boolean arg -> Ruby true/false (the non-string,
      # non-node-set argument conversions).
      expect(ctx.evaluate("ng:type-of(count(//p))", handler)).to eq("Float")
      expect(ctx.evaluate("ng:echo(count(//p))", handler)).to eq(2.0)
      expect(ctx.evaluate("ng:type-of(1 = 1)", handler)).to eq("TrueClass")
      expect(ctx.evaluate("ng:echo(1 = 0)", handler)).to be(false)
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

  describe "handler exception message extraction (fail closed)" do
    it "surfaces a NUL-containing message as Makiri::Error (no ArgumentError leak)" do
      h = Object.new
      def h.boom = raise("a\u0000b")
      expect { ctx.evaluate("ng:boom()", h) }
        .to raise_error(Makiri::Error, /handler raised/)
    end

    it "survives a handler whose #message itself raises" do
      h = Object.new
      def h.boom
        e = StandardError.new
        def e.message = raise("broken")
        raise e
      end
      expect { ctx.evaluate("ng:boom()", h) }.to raise_error(Makiri::Error)
    end
  end

  describe "memory safety", :gc_compact do
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

    # Regression: a handler invoked from a predicate could re-enter the SAME
    # context and mutate it mid-walk. Re-registering the prefixed name test's own
    # namespace prefix freed the URI string the evaluator still borrowed -> an
    # ASan-confirmed use-after-free read on the next context iteration's walk.
    # The fix refuses register_namespace / register_variable / node= while an
    # evaluate is in progress on that context (mkr_ctx_is_evaluating), so the
    # borrowed registrations and context node can never be freed/swapped under
    # the suspended evaluator. The mutation fails closed; the handler exception
    # surfaces as a clean Makiri::Error.
    let(:multi) do
      Makiri::HTML(<<~HTML)
        <html><body>
          <div><p>a</p></div>
          <div><p>b</p></div>
          <div><p>c</p></div>
        </body></html>
      HTML
    end
    let(:multi_ctx) do
      c = Makiri::XPathContext.new(multi)
      c.register_namespace("ng", "http://www.w3.org/1999/xhtml")
      c
    end

    it "fails closed when a handler re-registers the name-test's prefix mid-walk" do
      c = multi_ctx
      reg = Object.new
      reg.instance_variable_set(:@ctx, c)
      def reg.touch
        @ctx.register_namespace("ng", "http://www.w3.org/1999/xhtml") # would free borrowed URI
        true
      end
      # `ng:p` resolves over a multi-context set (the three divs); the predicate
      # would re-register on the first, freeing the URI the later iterations read.
      expect { c.evaluate("//div/ng:p[ng:touch()]", reg) }
        .to raise_error(Makiri::Error, /while evaluating/)
    end

    it "fails closed when a handler swaps the context node mid-walk" do
      c = multi_ctx
      target = multi.at_xpath("//p")
      reg = Object.new
      reg.instance_variable_set(:@ctx, c)
      reg.instance_variable_set(:@n, target)
      def reg.touch
        @ctx.node = @n
        true
      end
      expect { c.evaluate("//div/ng:p[ng:touch()]", reg) }
        .to raise_error(Makiri::Error, /while evaluating/)
    end

    it "fails closed when a handler registers a variable mid-walk" do
      c = multi_ctx
      reg = Object.new
      reg.instance_variable_set(:@ctx, c)
      def reg.touch
        @ctx.register_variable("x", "1")
        true
      end
      expect { c.evaluate("//div/ng:p[ng:touch()]", reg) }
        .to raise_error(Makiri::Error, /while evaluating/)
    end

    it "still allows a nested evaluate() on the same context from a handler" do
      c = multi_ctx
      reg = Object.new
      reg.instance_variable_set(:@ctx, c)
      def reg.inner(*) = @ctx.evaluate("count(//p)")
      expect(c.evaluate("ng:inner()", reg)).to eq(3.0)
    end
  end
end
