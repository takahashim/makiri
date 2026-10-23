# frozen_string_literal: true

# A Document's content is reached only through a guard - a counted read, or
# an exclusive edit (`bridge::dom`). No Ruby runs while a guard is held
# (Makiri runs no custom-function handler), so no query or edit leaves one.
RSpec.describe "a document's borrow guards", :gc_compact do
  {
    html: [-> { Makiri.HTML("<p>a</p><p>b</p>") }, ->(d) { d.at_css("p") }],
    xml: [-> { Makiri::XML("<r><p>a</p><p>b</p></r>") }, ->(d) { d.at_xpath("//p") }],
  }.each do |kind, (make, first_p)|
    context "on an #{kind.upcase} document" do
      let(:doc) { make.call }

      it "is not borrowed after a query, a failed query or an edit" do
        doc.xpath("//p")
        expect { doc.xpath("//p[") }.to raise_error(Makiri::XPath::SyntaxError)
        first_p.call(doc)["k"] = "v"
        expect(Makiri.__borrowed?(doc)).to be(false)
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
