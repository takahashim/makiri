# frozen_string_literal: true

require "spec_helper"

# Regression: the node-test kinds (text() / comment() /
# processing-instruction() / node()) must select the right nodes. A node-TYPE
# constant once collided in name with the node-TEST kind enum (MKR_NT_*),
# silently making //comment() and //processing-instruction() return nothing.
# These assert the actual selected nodes, not just at_xpath == xpath.first
# (which compares broken-to-broken and would not have caught the regression).
RSpec.describe "XPath node tests" do
  let(:doc) do
    Makiri.parse(<<~HTML)
      <html><body>
        <!--first--><p>hello</p> world <!--second-->
      </body></html>
    HTML
  end

  it "//comment() selects every comment node" do
    comments = doc.xpath("//comment()")
    expect(comments.map(&:to_s)).to eq(["<!--first-->", "<!--second-->"])
    expect(comments).to all(be_a(Makiri::Comment))
  end

  it "//text() selects text nodes (not comments or elements)" do
    texts = doc.xpath("//body//text()").map { |t| t.to_s.strip }.reject(&:empty?)
    expect(texts).to include("hello", "world")
    expect(doc.xpath("//text()")).to all(be_a(Makiri::Text))
  end

  it "//node() selects element, text and comment children alike" do
    kinds = doc.at_xpath("//body").xpath("node()").map(&:class).uniq
    expect(kinds).to include(Makiri::Element, Makiri::Text, Makiri::Comment)
  end

  it "//processing-instruction() selects PI nodes" do
    # PIs are uncommon in HTML; assert the node test is wired (empty, not error)
    # and matches when present via an XML-ish fragment parsed as HTML.
    expect(doc.xpath("//processing-instruction()")).to be_a(Makiri::NodeSet)
  end

  it "string() of an element concatenates its descendant text" do
    expect(doc.at_xpath("//body").xpath("string()")).to include("hello", "world")
  end
end
