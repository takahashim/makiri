# frozen_string_literal: true

require "spec_helper"

# Replacing a node's data over and over must keep every answer right. In the
# arena DOM (MAKIRI_ARENA_DOM=1) the old bytes are left in the byte store and
# the store is compacted at the start of a later edit
# (html/dom/compact.rs), which moves every span - this is the workload that
# triggers it; on Lexbor's DOM the old data goes back to Lexbor's pool.
RSpec.describe "Repeated data edits" do
  it "keeps the data of every node through thousands of replacements" do
    doc = Makiri::HTML(<<~HTML)
      <!DOCTYPE html><body><p id="p" title="t">one</p><!--c--><div id="d">two</div></body>
    HTML
    p = doc.at_css("#p")
    text = p.children.first
    detached = doc.create_text_node("kept while detached")
    big = "x" * 2048
    3000.times do |i|
      text.content = "#{big}#{i}"
      p["title"] = "title #{i}" if (i % 7).zero?
    end
    expect(text.content).to eq("#{big}2999")
    expect(p["title"]).to eq("title 2996")
    expect(detached.content).to eq("kept while detached")
    expect(doc.at_css("#d").text).to eq("two")
    expect(doc.body.inner_html).to eq(
      %(<p id="p" title="title 2996">#{big}2999</p><!--c--><div id="d">two</div>\n)
    )
    expect(doc.at_xpath("//p/text()").content).to end_with("2999")
  end
end
