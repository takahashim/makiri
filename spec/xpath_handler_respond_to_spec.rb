# frozen_string_literal: true

# Deciding whether a handler answers an XPath function asks the handler itself:
# its `respond_to?`, and through it `respond_to_missing?`, are Ruby code that may
# raise. That raise must come back like any other handler failure - a
# Makiri::Error, never an unwind through the evaluator - so the evaluation's
# guards still run. The observable one is the document's "being evaluated"
# lock: a raise that skipped it left every later mutation refused.
RSpec.describe "An XPath handler whose respond_to? raises" do
  let(:doc) { Makiri::HTML("<p>a</p><p>b</p>") }

  handlers = {
    "respond_to?" => Class.new do
      def respond_to?(name, include_all = false) = raise(ArgumentError, "respond_to? refused #{name}")
      def myfn(*) = 1.0
    end,
    "respond_to_missing?" => Class.new do
      def respond_to_missing?(name, include_all = false) = raise(ArgumentError, "missing refused #{name}")
    end
  }

  handlers.each do |hook, klass|
    context "when #{hook} raises" do
      it "fails the evaluation with Makiri::Error naming the handler's raise" do
        expect { doc.xpath("count(//p[myfn()])", klass.new) }
          .to raise_error(Makiri::Error, /handler raised: .*refused myfn/)
      end

      it "leaves the document mutable afterwards" do
        expect { doc.xpath("count(//p[myfn()])", klass.new) }.to raise_error(Makiri::Error)
        p = doc.at_css("p")
        p["x"] = "1"
        expect(p["x"]).to eq("1")
      end

      it "leaves a reused XPathContext usable" do
        ctx = Makiri::XPathContext.new(doc)
        expect { ctx.evaluate("myfn(//p)", klass.new) }.to raise_error(Makiri::Error)
        expect(ctx.evaluate("count(//p)")).to eq(2.0)
      end
    end
  end

  it "still reports an unknown function for a handler that simply lacks it" do
    expect { doc.xpath("nosuchfn()", Object.new) }
      .to raise_error(Makiri::Error, /unknown function/)
    doc.at_css("p")["y"] = "2"
    expect(doc.at_css("p")["y"]).to eq("2")
  end
end
