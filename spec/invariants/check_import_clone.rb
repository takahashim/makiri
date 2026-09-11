# frozen_string_literal: true
#
# Deep copies (clone_node / import_node), cross-document insertion, and fragment
# splicing.
#
#   ruby -Ilib spec/invariants/check_import_clone.rb [cases] [seed]
#
# ## The properties
#
#   P1 clone is isomorphic  the fingerprint of clone_node(deep) equals the source's
#   P2 clone is independent editing the clone does not change the source
#   P3 import re-serializes the tree equals what re-parsing its output computes
#   P4 import is independent editing the import source does not change the target
#   P5 fragment splices     inserting a fragment contributes its children, in order
#   P6 a move keeps the URI moving a node does not change namespace_uri
#
# ## Two different oracles, on purpose
#
# P1-P5 use Makiri's own parser as the oracle, the way check_ns_reresolve.rb
# does. That form of check has a blind spot, and P6 exists because of it.
#
# Re-resolution would be SELF-CONSISTENT with serialization. Under a model that
# re-derives a node's namespace from the declarations around it, moving
#
#   <r xmlns:p="urn:a"><p:x/><mid xmlns:p="urn:c"><hole/></mid></r>
#
# would make `p:x` mean urn:c. Only the prefix reaches the output, so reading it
# back gives urn:c as well: both sides agree, and a metamorphic check reports
# nothing. P6's oracle is the DOM instead - a node's namespace URI is decided
# once and does not change - which is what Makiri now implements and what
# browsers do.

require "makiri"

NS_URIS = ["urn:a", "urn:b", "urn:c"].freeze
PREFIXES = %w[p q].freeze
LOCALS = %w[a b c entry title item].freeze

class Rng
  def initialize(seed) = @s = seed
  def next_int(n)
    @s = (@s * 1_103_515_245 + 12_345) % 2_147_483_648
    n.zero? ? 0 : (@s / 65_536) % n
  end
  def pick(list) = list[next_int(list.length)]
  def chance(num, den) = next_int(den) < num
end

# Bind every prefix at the root. `shift` rotates what they are bound to, so the
# same prefix means different things in the source and the destination - which
# is what makes a namespace change visible at all.
def build_doc(rng, shift)
  uris = NS_URIS.rotate(shift)
  decls = PREFIXES.each_with_index.map { |p, i| %( xmlns:#{p}="#{uris[i]}") }.join
  body = Array.new(1 + rng.next_int(3)) { build_elem(rng, 2) }.join
  %(<root#{decls}>#{body}</root>)
end

def build_elem(rng, depth)
  local = rng.pick(LOCALS)
  pfx = rng.chance(6, 10) ? rng.pick(PREFIXES) : nil
  name = pfx ? "#{pfx}:#{local}" : local

  attrs = +""
  attrs << %( id="#{rng.next_int(100)}") if rng.chance(6, 10)
  attrs << %( #{rng.pick(PREFIXES)}:k="v") if rng.chance(4, 10)

  return "<#{name}#{attrs}/>" if depth.zero? || rng.chance(4, 10)

  kids = Array.new(1 + rng.next_int(2)) { build_elem(rng, depth - 1) }.join
  kids += "t" if rng.chance(3, 10)
  "<#{name}#{attrs}>#{kids}</#{name}>"
end

def elements(node, acc = [])
  node.children.each do |c|
    next unless c.node_type == 1

    acc << c
    elements(c, acc)
  end
  acc
end

# Carries the resolved URI, which serialization does not show. Compares LOCAL
# names: the serializer chooses the prefixes and may invent one, so what has to
# round-trip is the namespace, not the spelling.
def fingerprint(node, out = [])
  node.children.each do |c|
    case c.node_type
    when 1
      out << [1, c.local_name, c.namespace_uri,
              c.attribute_nodes.reject { |a| a.name.start_with?("xmlns") }
                               .map { |a| [a.local_name, a.value, a.namespace_uri] }.sort]
      fingerprint(c, out)
    when 3, 4 then out << [c.node_type, c.content]
    when 8 then out << [8, c.content]
    when 7 then out << [7, c.name, c.content]
    end
  end
  out
end

# The subtree's own fingerprint, the node included.
def subtree_fp(node)
  [[1, node.local_name, node.namespace_uri,
    node.attribute_nodes.reject { |a| a.name.start_with?("xmlns") }
        .map { |a| [a.local_name, a.value, a.namespace_uri] }.sort]] + fingerprint(node)
end

count = (ARGV[0] || 2000).to_i
seed  = (ARGV[1] || 20_260_911).to_i

stats = Hash.new(0)
diffs = Hash.new { |h, k| h[k] = [] }

def record(diffs, key, *payload)
  diffs[key] << payload if diffs[key].length < 3
end

count.times do |i|
  rng = Rng.new(seed + i * 7919)
  src_xml = build_doc(rng, 0)
  tgt_xml = build_doc(rng, 1) # the same prefixes mean something else here
  src = Makiri::XML(src_xml)
  tgt = Makiri::XML(tgt_xml)

  src_els = elements(src.root)
  tgt_els = elements(tgt.root) << tgt.root
  next if src_els.empty? || tgt_els.empty?

  node = rng.pick(src_els)
  host = rng.pick(tgt_els)

  # --- P1 / P2: clone -------------------------------------------------
  begin
    before = subtree_fp(node)
    cl = node.clone_node(true)
    if subtree_fp(cl) == before
      stats[:p1_ok] += 1
    else
      stats[:p1_ng] += 1
      record(diffs, :p1, src_xml, before, subtree_fp(cl))
    end

    cl["cloned"] = "1"
    cl.children.first&.remove
    if subtree_fp(node) == before
      stats[:p2_ok] += 1
    else
      stats[:p2_ng] += 1
      record(diffs, :p2, src_xml, before, subtree_fp(node))
    end
  rescue StandardError => e
    stats[:clone_error] += 1
    record(diffs, :clone_error, src_xml, "#{e.class}: #{e.message}", nil)
  end

  # --- P3 / P4: import + insert ---------------------------------------
  begin
    imported = tgt.import_node(node, true)
    host.add_child(imported)

    got = fingerprint(tgt)
    ser = tgt.root.to_xml
    want = fingerprint(Makiri::XML(ser))
    if got == want
      stats[:p3_ok] += 1
    else
      stats[:p3_ng] += 1
      pair = got.zip(want).find { |a, b| a != b }
      record(diffs, :p3, "#{src_xml}\n                  + #{tgt_xml}\n                  -> #{ser}",
             pair&.first, pair&.last)
    end

    # editing the import source must not reach the target (separate arenas)
    tgt_fp = fingerprint(tgt)
    node["touched"] = "1"
    node.children.first&.remove
    if fingerprint(tgt) == tgt_fp
      stats[:p4_ok] += 1
    else
      stats[:p4_ng] += 1
      record(diffs, :p4, src_xml, tgt_fp, fingerprint(tgt))
    end
  rescue StandardError => e
    stats[:import_error] += 1
    record(diffs, :import_error, "#{src_xml} + #{tgt_xml}", "#{e.class}: #{e.message}", nil)
  end

  # --- P5: fragment splice --------------------------------------------
  begin
    pfx = rng.pick(PREFIXES)
    frag_xml = "<#{pfx}:f1/>t<#{pfx}:f2><#{pfx}:inner/></#{pfx}:f2>"
    frag = tgt.fragment(frag_xml)
    kids = frag.children.length
    anchor = rng.pick(elements(tgt.root) << tgt.root)
    before_len = anchor.children.length
    anchor.add_child(frag)

    got = fingerprint(tgt)
    ser = tgt.root.to_xml
    want = fingerprint(Makiri::XML(ser))
    spliced = anchor.children.length == before_len + kids

    if got == want && spliced
      stats[:p5_ok] += 1
    else
      stats[:p5_ng] += 1
      pair = got.zip(want).find { |a, b| a != b }
      record(diffs, :p5, "#{frag_xml} -> #{ser}",
             spliced ? pair&.first : "children #{before_len}+#{kids} -> #{anchor.children.length}",
             pair&.last)
    end
  rescue StandardError => e
    stats[:fragment_error] += 1
    record(diffs, :fragment_error, tgt_xml, "#{e.class}: #{e.message}", nil)
  end

  # --- P6: a move keeps the URI (oracle: the DOM) ----------------------
  # Deterministic, so once is enough.
  next unless i.zero?

  begin
    d = Makiri::XML(%(<r xmlns:p="urn:a"><p:x p:k="v"><p:y/></p:x><mid xmlns:p="urn:c"><hole/></mid></r>))
    x = d.root.children[0]
    hole = d.root.children[1].children[0]
    before = [x.namespace_uri, x.attribute_nodes.map(&:namespace_uri),
              x.children.first.namespace_uri]
    hole.add_child(x)
    after = [x.namespace_uri, x.attribute_nodes.map(&:namespace_uri),
             x.children.first.namespace_uri]
    if before == after
      stats[:p6_ok] += 1
    else
      stats[:p6_ng] += 1
      record(diffs, :p6, %(moving p:x under a subtree binding p to urn:c), after, before)
    end
  rescue StandardError => e
    stats[:move_error] += 1
    record(diffs, :move_error, "move", "#{e.class}: #{e.message}", nil)
  end
end

puts "=" * 72
puts "deep copy / cross-document insertion / fragments (#{count} cases)"
[[:p1, "clone is isomorphic"], [:p2, "clone is independent"],
 [:p3, "import re-serializes"], [:p4, "import is independent"],
 [:p5, "fragment splices"], [:p6, "a move keeps the URI"]].each do |key, label|
  puts format("  %-24s OK %5d  NG %5d", label, stats[:"#{key}_ok"], stats[:"#{key}_ng"])
end
%i[clone_error import_error fragment_error move_error].each do |k|
  puts format("  %-24s %d", k.to_s, stats[k]) if stats[k].positive?
end

diffs.each do |key, list|
  next if list.empty?

  puts
  puts "-" * 72
  puts "## #{key}"
  list.each do |(ctx, a, b)|
    puts
    puts "  context : #{ctx.to_s[0, 260]}"
    puts "  got     : #{a.inspect[0, 220]}"
    puts "  want    : #{b.inspect[0, 220]}" if b
  end
end

ng_total = %i[p1 p2 p3 p4 p5 p6].sum { |k| stats[:"#{k}_ng"] } +
           stats[:clone_error] + stats[:import_error] + stats[:fragment_error] +
           stats[:move_error]
exit(ng_total.positive? ? 1 : 0)
