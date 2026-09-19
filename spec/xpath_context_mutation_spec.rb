# frozen_string_literal: true

# An XPathContext is reused across the edits made to its document between
# evaluations. It used to keep the element index it saw when it was made, so an
# evaluation after a mutation - which frees that index - read freed memory: the
# answer came from whatever reused the block, typically another document's
# index. Each evaluation now reads the index afresh from the document.
RSpec.describe "Makiri::XPathContext across document mutations" do
  let(:doc) { Makiri::HTML("<html><body>#{"<div></div>" * 3}</body></html>") }
  let(:ctx) { Makiri::XPathContext.new(doc) }

  it "answers //tag from the document as it is now" do
    expect(ctx.evaluate("//div").size).to eq(3)

    body = doc.at_css("body")
    200.times { body.add_child(doc.create_element("div")) }
    # Build other documents' indexes into the memory the dropped one freed.
    others = Array.new(5) do
      Makiri::HTML("<html><body>#{"<div><span></span></div>" * 50}</body></html>")
    end
    others.each { |o| o.xpath("//span") }

    expect(ctx.evaluate("//div").size).to eq(203)
    expect(ctx.evaluate("count(//div)")).to eq(203.0)
  end

  it "finds the owner of an attribute added after the first evaluation" do
    expect(ctx.evaluate("count(//@*)")).to eq(0.0)

    doc.at_css("div")["data-k"] = "v"

    owners = ctx.evaluate("//@data-k/..")
    expect(owners.size).to eq(1)
    expect(owners.first.name).to eq("div")
  end
end
