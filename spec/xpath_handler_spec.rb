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

    it "round-trips a NodeSet from its validated internal storage in document order" do
      nodes = doc.xpath("//p").to_a
      set = Makiri::NodeSet.new(doc, [nodes.last, nodes.first, nodes.last])
      def set.length = raise("length override was dispatched")
      def set.[](_index) = raise("index override was dispatched")
      returning = Class.new do
        define_method(:initialize) { |value| @value = value }
        define_method(:echo) { |_argument| @value }
      end.new(set)

      result = ctx.evaluate("ng:echo(//p)", returning)
      expect(result).to be_a(Makiri::NodeSet)
      expect(result.map(&:text)).to eq(%w[hi there])
      expect(ctx.evaluate("string(ng:echo(//p))", returning)).to eq("hi")
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

    # The method name is interned as UTF-8, the encoding Ruby defines a
    # non-ASCII method under; interned US-ASCII it was another symbol, and the
    # method was reported as an unknown function.
    it "dispatches a non-ASCII function name to its method" do
      h = Object.new
      def h.é(*) = "ok"
      expect(doc.xpath("é()", h)).to eq("ok")
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
    # evaluate is in progress on that context (Context::is_evaluating), so the
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

    it "keeps the outer walk's handler across a nested evaluate that has its own" do
      # The nested evaluate installs a handler too. Taking it off used to clear
      # the context's resolver outright, so the outer walk's next call - the
      # second <p> - failed as an unknown function.
      c = Makiri::XPathContext.new(multi)
      h = Object.new
      h.instance_variable_set(:@ctx, c)
      h.instance_variable_set(:@calls, 0)
      def h.touch
        @calls += 1
        @ctx.evaluate("count(//p[inner()])", self) if @calls == 1
        true
      end
      def h.inner = true
      def h.calls = @calls
      expect(c.evaluate("//p[touch()]", h).length).to eq(3)
      expect(h.calls).to eq(3)
    end

    # The engine borrows names, values and index slices from the document for
    # the whole walk, and Lexbor frees an attribute's old value when a new one is
    # set, so a handler must not edit the document it is evaluated over: every
    # mutator refuses while such an evaluation runs.
    describe "a handler editing the document under evaluation" do
      editor_class = Class.new do
        def initialize(&edit) = @edit = edit

        def touch
          @edit.call
          true
        end
      end

      {
        "sets an attribute" => ->(d) { d.at_css("p")["class"] = "x" },
        "removes an attribute" => ->(d) { d.at_css("p").delete("class") },
        "sets content" => ->(d) { d.at_css("p").content = "y" },
        "removes a node" => ->(d) { d.at_css("div").remove },
        "inserts a node" => ->(d) { d.at_css("body").add_child(d.create_element("hr")) },
        "sets inner_html" => ->(d) { d.at_css("div").inner_html = "<b>z</b>" },
      }.each do |what, edit|
        it "fails closed when it #{what}" do
          h = editor_class.new { edit.call(doc) }
          expect { doc.xpath("//p[touch()]", h) }
            .to raise_error(Makiri::Error, /while evaluating/)
          expect { ctx.evaluate("//p[ng:touch()]", h) }
            .to raise_error(Makiri::Error, /while evaluating/)
        end
      end

      it "leaves the document unchanged, and editable again once the walk is over" do
        h = editor_class.new { doc.at_css("p")["class"] = "x" }
        expect { doc.xpath("//p[touch()]", h) }.to raise_error(Makiri::Error)
        expect(doc.at_css("p")["class"]).to be_nil
        doc.at_css("p")["class"] = "x"
        expect(doc.at_css("p")["class"]).to eq("x")
      end

      it "still lets a handler edit a different document" do
        other = Makiri::HTML("<p>o</p>")
        h = editor_class.new { other.at_css("p")["class"] = "x" }
        expect(doc.xpath("//p[touch()]", h).length).to eq(2)
        expect(other.at_css("p")["class"]).to eq("x")
      end

      it "fails closed when it moves a node out of the document into another" do
        other = Makiri::HTML("<p>o</p>")
        h = editor_class.new { other.at_css("body").add_child(doc.at_css("div")) }
        expect { doc.xpath("//p[touch()]", h) }
          .to raise_error(Makiri::Error, /while evaluating/)
        expect(doc.at_css("div")).not_to be_nil
      end

      # A factory changes the document too - it makes its nodes there - and on
      # an XML document it grows the arena the walk is reading from. So the
      # factories are refused like any other edit, on both representations.
      {
        "create_element" => ->(d) { d.create_element("hr") },
        "create_text_node" => ->(d) { d.create_text_node("t") },
        "create_comment" => ->(d) { d.create_comment("c") },
        "clone_node" => ->(d) { d.at_css("p").clone_node(true) },
        "import_node" => ->(d) { d.import_node(Makiri::HTML("<i>x</i>").at_css("i")) },
        "fragment" => ->(d) { d.fragment("<i>x</i>") },
      }.each do |what, make|
        it "fails closed when it calls #{what} on the HTML document" do
          h = editor_class.new { make.call(doc) }
          expect { doc.xpath("//p[touch()]", h) }
            .to raise_error(Makiri::Error, /while evaluating/)
        end
      end

      {
        "create_element" => ->(x) { x.create_element("e") },
        "create_text_node" => ->(x) { x.create_text_node("t") },
        "create_comment" => ->(x) { x.create_comment("c") },
        "clone_node" => ->(x) { x.at_xpath("//a").clone_node(true) },
        "import_node" => ->(x) { x.import_node(Makiri::XML("<i/>").root) },
        "fragment" => ->(x) { x.fragment("<i/>") },
      }.each do |what, make|
        it "fails closed when it calls #{what} on the XML document" do
          xml = Makiri::XML(%(<r><a k="1"/><a/></r>))
          h = editor_class.new { make.call(xml) }
          expect { Makiri::XPathContext.new(xml).evaluate("//a[touch()]", h) }
            .to raise_error(Makiri::Error, /while evaluating/)
        end
      end

      # `fragment` converts its argument with `to_s` - arbitrary Ruby - after
      # the check that no evaluation is reading the document. That Ruby can
      # START one and leave it suspended mid-walk (a handler that yields from
      # an Enumerator), still reading the document when the fragment is made
      # in it. So the check is made again after the conversion.
      {
        html: [-> { Makiri.HTML("<p>x</p>") }, "//p[f()]", "<b>y</b>"],
        xml: [-> { Makiri::XML("<r><a/></r>") }, "//a[f()]", "<b/>"],
      }.each do |kind, (make, expr, source)|
        it "fails closed when a #{kind} fragment's source starts an evaluation it leaves suspended" do
          doc = make.call
          walk = Enumerator.new do |y|
            h = Object.new
            h.define_singleton_method(:f) { |*| y << :suspended; true }
            doc.xpath(expr, h)
          end
          arg = Object.new
          arg.define_singleton_method(:to_s) { walk.next && source }

          expect { doc.fragment(arg) }.to raise_error(Makiri::Error, /while evaluating/)
        end
      end

      it "fails closed for an XML document too" do
        xml = Makiri::XML(%(<r><a k="1"/><a/></r>))
        h = editor_class.new { xml.at_xpath("//a")["k"] = "2" }
        expect { Makiri::XPathContext.new(xml).evaluate("//a[touch()]", h) }
          .to raise_error(Makiri::Error, /while evaluating/)
        expect(xml.at_xpath("//a")["k"]).to eq("1")
      end
    end
  end
end

# A `to_s` Makiri calls can be anyone's, and may return anything. Its result
# must be checked to BE a String before its bytes are read as one: an Integer
# read as a String's pointer and length is a crash, not an error.
RSpec.describe "a to_s that returns a non-String" do
  let(:not_a_string) { Class.new { def to_s = 42 }.new }

  it "refuses it as an XPath handler's result" do
    doc = Makiri::HTML("<p>x</p>")
    value = not_a_string
    handler = Class.new { define_method(:odd) { |*| value } }.new
    expect { doc.xpath("odd()", handler) }.to raise_error(Makiri::Error, /converted to a string/)
  end

  it "refuses it as a registered variable's value" do
    ctx = Makiri::XPathContext.new(Makiri::HTML("<p>x</p>"))
    expect { ctx.register_variable("v", not_a_string) }.to raise_error(TypeError, /non-String/)
  end

  it "refuses it as a created element's attribute value" do
    doc = Makiri::XML::Document.parse("<r/>")
    expect { doc.create_element("e", "k" => not_a_string) }.to raise_error(TypeError, /non-String/)
  end
end
