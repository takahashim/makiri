# frozen_string_literal: true

# XPath results are returned in document order. The doc-order comparator
# (mkr_xpath_value.c doc_order_cmp) resolves same-parent sibling order by
# scanning outward from the two nodes in lockstep, so a predicate/union that
# selects nodes sitting deep in a wide, flat parent stays O(gap) rather than
# O(distance-from-first-child). These assert the ORDER is correct (the property
# the optimisation must preserve), independent of timing.
RSpec.describe "XPath document order" do
  # 600 <li>, only some marked; markers scattered incl. deep in the flat <ul>.
  let(:marked) { [3, 200, 201, 410, 599] }
  let(:doc) do
    lis = (0...600).map { |i| %(<li>#{marked.include?(i) ? "hit#{i}" : "x"}</li>) }.join
    Makiri::HTML("<html><body><ul>#{lis}</ul></body></html>")
  end

  it "returns a scattered predicate result in document order" do
    got = doc.xpath('//li[starts-with(., "hit")]').map(&:text)
    expect(got).to eq(marked.map { |i| "hit#{i}" })
  end

  it "returns deep-only matches in document order" do
    got = doc.xpath('//li[contains(., "hit5")]').map(&:text) # only hit599 ... and hit5xx (none)
    expect(got).to eq(["hit599"])
  end

  it "orders a union of separately-collected node sets by document order" do
    d = Makiri::HTML(<<~HTML)
      <html><body>
        <p id="p1">1</p><div id="d1">2</div>
        <p id="p2">3</p><div id="d2">4</div>
        <p id="p3">5</p>
      </body></html>
    HTML
    # //div | //p collects all <p> then all <div>; the result must be doc order.
    expect(d.xpath("//div | //p").map { |n| n["id"] })
      .to eq(%w[p1 d1 p2 d2 p3])
  end

  it "dedups and orders descendant-or-self unions across nesting" do
    d = Makiri::HTML("<html><body><div><span class=a>1</span></div>" \
                     "<div><span class=a>2</span><span class=a>3</span></div></body></html>")
    expect(d.xpath("//span").map(&:text)).to eq(%w[1 2 3])
  end
end
