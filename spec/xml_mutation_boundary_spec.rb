# frozen_string_literal: true

require "spec_helper"

# Unsafe-boundary coverage for the XML mutation primitives. The raw linked-list
# relinking (`xml/raw.rs`: detach, splice, attribute unlink/append, the
# iterative deep copy) is driven through the safe `xml/mutate.rs` API, so these
# tests exercise the paths a single edit misses - a high-volume attribute list,
# repeated attach/detach, and a deep subtree copy - where a dropped `prev`/
# `next` link or a recursive walk would show up.
#
# Structural soundness is checked by recomputing it from scratch (serialize +
# reparse) rather than by trusting the in-memory links the code under test
# maintains.
RSpec.describe "Makiri::XML mutation boundary" do
  def element_depth(node)
    depth = 0
    while (node = node.element_children.first)
      depth += 1
    end
    depth
  end

  it "deep-copies and imports a deep subtree without recursion" do
    doc = Makiri::XML("<x/>")
    cur = doc.root
    depth = gc_churn_iters(800, 100)
    depth.times do |i|
      n = doc.create_element("n")
      n["i"] = i.to_s
      cur.add_child(n)
      cur = n
    end
    cur.content = "leaf"

    copy = doc.root.clone_node(1)
    expect(element_depth(copy)).to eq(depth)
    expect(copy.xpath(".//n[last()]/text()").first.text).to eq("leaf")

    # A fresh parse of the serialized copy proves every parent/child link is
    # right, not just the ones the walk happened to touch.
    reparsed = Makiri::XML(copy.to_xml)
    expect(element_depth(reparsed.root)).to eq(depth)

    target = Makiri::XML("<y/>")
    imported = target.import_node(doc.root, true)
    expect(element_depth(imported)).to eq(element_depth(copy))
  end

  it "handles a large attribute list through append, replace and removal" do
    doc = Makiri::XML("<r/>")
    r = doc.root
    n = gc_churn_iters(2000, 100)
    n.times { |i| r["a#{i}"] = "v#{i}" }
    expect(r.attribute_nodes.length).to eq(n)
    expect(r["a#{n - 1}"]).to eq("v#{n - 1}")

    # Replacing in place must not duplicate the attribute.
    (n / 2).times { |i| r["a#{i}"] = "w#{i}" }
    expect(r.attribute_nodes.length).to eq(n)
    expect(r["a0"]).to eq("w0")

    # Remove from the head, the middle and the tail.
    r.delete("a0")
    r.delete("a#{n / 2}")
    r.delete("a#{n - 1}")
    expect(r.attribute_nodes.length).to eq(n - 3)

    pairs = r.attribute_nodes.map { |a| [a.name, a.value] }
    round_tripped = Makiri::XML(r.to_xml).root.attribute_nodes.map { |a| [a.name, a.value] }
    expect(round_tripped).to eq(pairs)
  end

  it "keeps the child list consistent under repeated attach/detach churn" do
    doc = Makiri::XML("<r><keep/></r>")
    r = doc.root
    a = doc.create_element("a")
    b = doc.create_element("b")

    gc_churn_iters(2000, 100).times do
      r.add_child(a)
      a.remove
      r << a
      a.add_previous_sibling(b)
      b.remove
      a.add_next_sibling(b)
      b.remove
    end

    expect(b.parent).to be_nil
    # Recomputed from scratch: any stale prev/next link would surface here.
    expect(doc.root.to_xml).to eq("<r><keep/><a/></r>")
  end

  it "moves one node between parents without corrupting either list" do
    doc = Makiri::XML("<r><p/><q/></r>")
    p = doc.at_xpath("//p")
    q = doc.at_xpath("//q")
    n = doc.create_element("n")

    gc_churn_iters(1000, 50).times do
      p.add_child(n)
      q.add_child(n)
      p.add_child(n)
    end

    expect(p.element_children.map(&:name)).to eq(["n"])
    expect(q.element_children.map(&:name)).to eq([])
    expect(doc.root.to_xml).to eq("<r><p><n/></p><q/></r>")
  end

  it "rejects a cycle and an attribute-as-child without changing the tree" do
    doc = Makiri::XML("<r><a/></r>")
    r = doc.root
    a = doc.at_xpath("//a")

    expect { a.add_child(r) }.to raise_error(Makiri::Error)
    expect(doc.root.to_xml).to eq("<r><a/></r>")

    el = doc.create_element("e")
    el["k"] = "v"
    expect { r.add_child(el.attribute_nodes.first) }.to raise_error(Makiri::Error)
    expect(doc.root.to_xml).to eq("<r><a/></r>")
  end
end
