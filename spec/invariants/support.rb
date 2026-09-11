# frozen_string_literal: true
#
# Shared scaffolding for the invariant checks.
#
# Only what is genuinely ONE thing lives here. The document generators do not:
# each check builds the shapes its property needs (the serialization check stays
# away from elements HTML5 re-parents, the index check sprinkles the attributes
# its predicates look for), and sharing them would erase what each is aiming at.

require "makiri"

# A tiny deterministic PRNG, so every finding replays from its seed.
class Rng
  def initialize(seed) = @s = seed

  def next_int(n)
    @s = (@s * 1_103_515_245 + 12_345) % 2_147_483_648
    n.zero? ? 0 : (@s / 65_536) % n
  end

  def pick(list) = list[next_int(list.length)]
  def chance(num, den) = next_int(den) < num
end

# Counts, and whether anything went wrong. `note` is for the things a check
# reports but does not fail on.
class Tally
  def initialize = (@counts = Hash.new(0); @bad = 0)

  def ok(key) = @counts[key] += 1
  def note(key) = @counts[key] += 1

  def bad(key)
    @counts[key] += 1
    @bad += 1
  end

  def [](key) = @counts[key]
  def failed? = @bad.positive?
  def exit_status = failed? ? 1 : 0
end

# Every element in the subtree, in document order. Walks `children` directly, so
# it goes nowhere near the indexes a check may be measuring.
def elements(node, acc = [])
  node.children.each do |c|
    next unless c.node_type == 1

    acc << c
    elements(c, acc)
  end
  acc
end

# Where a check does its editing: <body> on the HTML backend, the root element
# on the XML one.
def container_of(doc) = doc.at_css("body") || doc.root

# The comparison key for the namespace-carrying checks.
#
# It carries the resolved URI, which is the part serialization does not show and
# the reason these checks exist at all. It compares LOCAL names, not qualified
# ones, and skips xmlns attributes: the serializer picks the prefixes and the
# declarations the output needs, and may invent a prefix where one would
# otherwise have to mean two things, so what must round-trip is the namespace a
# node is in and not the spelling it arrives under.
#
# The serialization check needs a different key and defines its own; see
# `content_fingerprint` there.
def fingerprint(node, out = [])
  node.children.each do |c|
    case c.node_type
    when 1
      out << [1, c.local_name, c.namespace_uri, fingerprint_attrs(c)]
      fingerprint(c, out)
    when 3, 4 then out << [c.node_type, c.content]
    when 8 then out << [8, c.content]
    when 7 then out << [7, c.name, c.content]
    end
  end
  out
end

def fingerprint_attrs(el)
  el.attribute_nodes.reject { |a| a.name.start_with?("xmlns") }
    .map { |a| [a.local_name, a.value, a.namespace_uri] }.sort
end

# The same key for a subtree, the node itself included.
def subtree_fingerprint(node)
  [[1, node.local_name, node.namespace_uri, fingerprint_attrs(node)]] + fingerprint(node)
end
