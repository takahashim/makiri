# frozen_string_literal: true

# A Document's content is reached only through a guard - a counted read, or
# an exclusive edit (`bridge::dom`). An XPath evaluation with a handler holds
# its read across the handler's Ruby, and a handler that switches Fibers
# leaves the evaluation suspended with the read still held. These pin what
# that means: while suspended, the document reads as usual and refuses every
# edit (with the evaluation guard's error, which comes first); once the walk
# has finished - resumed to its end - the read is released and edits work
# again. A walk left suspended for good keeps refusing; that is accepted, as
# an abandoned `Hash#each` Enumerator leaves its Hash locked.
RSpec.describe "a document read held across Ruby", :gc_compact do
  {
    html: [-> { Makiri.HTML("<p>a</p><p>b</p>") }, "//p[pause()]", ->(d) { d.at_css("p") }],
    xml: [-> { Makiri::XML("<r><p>a</p><p>b</p></r>") }, "//p[pause()]", ->(d) { d.at_xpath("//p") }],
  }.each do |kind, (make, expr, first_p)|
    context "on an #{kind.upcase} document" do
      let(:doc) { make.call }
      let(:walk) do
        d = doc
        Enumerator.new do |y|
          h = Object.new
          h.define_singleton_method(:pause) { |*| y << :suspended; true }
          y << d.xpath(expr, h).size
        end
      end

      it "reads while the walk is suspended, and refuses an edit" do
        expect(walk.next).to eq(:suspended)
        expect(first_p.call(doc).text).to eq("a")
        expect(doc.xpath("count(//p)")).to eq(2.0)
        expect { first_p.call(doc)["k"] = "v" }.to raise_error(Makiri::Error, /while evaluating/)
        expect { first_p.call(doc).remove }.to raise_error(Makiri::Error, /while evaluating/)
        expect(first_p.call(doc)["k"]).to be_nil
      end

      it "edits again once the walk has run to its end" do
        expect(walk.next).to eq(:suspended)
        expect(walk.next).to eq(:suspended)
        expect(walk.next).to eq(2)
        first_p.call(doc)["k"] = "v"
        expect(first_p.call(doc)["k"]).to eq("v")
        first_p.call(doc).remove
        expect(doc.xpath("count(//p)")).to eq(1.0)
      end

      it "lets a handler read the document it is evaluating" do
        d = doc
        h = Object.new
        h.define_singleton_method(:peek) { |*| d.xpath("count(//p)") == 2.0 && !first_p.call(d).nil? }
        expect(doc.xpath(expr.sub("pause", "peek"), h).size).to eq(2)
      end

      it "refuses to adopt a node out of it, leaving both documents as they were" do
        target = kind == :html ? Makiri.HTML("<div></div>") : Makiri::XML("<r><div/></r>")
        host = kind == :html ? target.at_css("div") : target.at_xpath("//div")
        expect(walk.next).to eq(:suspended)
        expect { host.add_child(first_p.call(doc)) }.to raise_error(Makiri::Error, /while evaluating/)
        expect(host.children.size).to eq(0)
        expect(doc.xpath("count(//p)")).to eq(2.0)
      end

      it "adopts a node out of it once the walk has run to its end" do
        target = kind == :html ? Makiri.HTML("<div></div>") : Makiri::XML("<r><div/></r>")
        host = kind == :html ? target.at_css("div") : target.at_xpath("//div")
        3.times { walk.next }
        host.add_child(first_p.call(doc))
        expect(host.children.size).to eq(1)
        expect(doc.xpath("count(//p)")).to eq(1.0)
      end

      it "evaluates a kept context afresh after an edit between two evaluates" do
        ctx = Makiri::XPathContext.new(doc)
        expect(ctx.evaluate("count(//p)")).to eq(2.0)
        first_p.call(doc).remove
        expect(ctx.evaluate("count(//p)")).to eq(1.0)
      end
    end
  end
end
