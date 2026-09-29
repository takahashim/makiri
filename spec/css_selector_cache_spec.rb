# frozen_string_literal: true

require "spec_helper"

# The HTML CSS engine caches compiled selectors in a process-global arena and
# adapts: a full cache is flushed as a unit, a window of mostly one-off
# selectors switches caching off, and it is switched back on to be re-tested
# (lexbor/selectors.rs: CachePolicy, SelectorCache). Every one of those moves
# frees compiled lists, so an answer that survives them all is the property -
# a stale list read after its arena was cleaned would answer from freed memory,
# which `rake sanitize` turns into a report.
RSpec.describe "CSS compiled-selector cache" do
  ids = (1..300).map { |i| "n#{i}" }
  let(:doc) do
    Makiri::HTML("<html><body>#{ids.map { |id| %(<p id="#{id}" class="c">#{id}</p>) }.join}</body></html>")
  end

  def expect_answers(doc, ids)
    ids.each { |id| expect(doc.at_css("##{id}")&.text).to eq(id) }
    expect(doc.css("p.c").length).to eq(300)
    expect(doc.css("p:nth-child(2)").map(&:text)).to eq(%w[n2])
  end

  it "answers the same across a full-cache flush" do
    # 300 distinct selectors against a 256-entry cache: the 257th flushes it.
    2.times { expect_answers(doc, ids) }
  end

  it "answers the same while bypassing and after caching is re-tested" do
    # Mostly one-off selectors for several windows switch caching off; enough
    # further windows switch it back on to be measured again.
    40_000.times { |i| doc.at_css("#n#{i % 300 + 1}, #x#{i}") }
    expect_answers(doc, ids)
    40_000.times { |i| doc.at_css("#n#{i % 3 + 1}") }
    expect_answers(doc, ids)
  end

  # The `:nth-*` memos' tables are kept from one query to the next too
  # (css_match's Scratch) - but never what they counted: a node moved between
  # two queries keeps its address and changes its position.
  it "counts :nth-* positions afresh after the tree changes" do
    d = Makiri::HTML("<ul>#{(1..5).map { |i| "<li>#{i}</li>" }.join}</ul>")
    ul = d.at_css("ul")
    5.times do
      order = ul.children.map(&:text)
      expect(d.css("li:nth-child(odd)").map(&:text)).to eq(order.values_at(0, 2, 4))
      expect(d.css("li:nth-last-child(2)").map(&:text)).to eq([order[3]])
      ul << ul.children.first
    end
  end

  it "rejects a bad selector without disturbing the cached ones" do
    expect_answers(doc, ids.first(10))
    expect { doc.css("p[") }.to raise_error(Makiri::CSS::SyntaxError)
    expect_answers(doc, ids.first(10))
  end
end
