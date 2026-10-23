# frozen_string_literal: true

# A node wrapper keeps its data in its own object slot (an embeddable
# TypedData), so compaction moves the data with the object. Every wrapper must
# still name its own node, under its own Document, afterwards - and still be
# the one wrapper of that node.
RSpec.describe "node wrappers across compaction", :gc_compact do
  # `GC.compact` rather than `verify_compaction_references`, which raises
  # inside Ruby under the GC_COMPACT_STRESS hook (with either build).
  def compact!
    GC.compact
  end

  it "keeps each HTML and XML wrapper naming its node" do
    html = Makiri.HTML("<ul>#{(1..300).map { |i| "<li id='i#{i}'>#{i}</li>" }.join}</ul>")
    xml = Makiri::XML("<r>#{(1..300).map { |i| "<i n='#{i}'>#{i}</i>" }.join}</r>")
    lis = html.css("li").to_a
    is = xml.root.element_children.to_a
    ids = lis.map(&:object_id)
    3.times do
      compact!
      expect(lis.map { |n| n["id"] }).to eq((1..300).map { |i| "i#{i}" })
      expect(is.map { |n| n["n"] }).to eq((1..300).map(&:to_s))
      expect(lis.map(&:document).uniq).to eq([html])
      expect(html.css("li").map(&:object_id)).to eq(ids)
      expect(lis.first.parent.name).to eq("ul")
    end
  end

  it "frees wrappers that nothing holds" do
    doc = Makiri.HTML("<p>x</p>" * 500)
    5.times { doc.css("p").each(&:name) }
    GC.start
    compact!
    expect(doc.css("p").size).to eq(500)
  end
end
