# frozen_string_literal: true
#
# A mutated tree's namespace state must equal what re-parsing its own output
# computes.
#
#   ruby -Ilib spec/invariants/check_ns_reresolve.rb [documents] [seed]
#
# ## Why this is checked here rather than in a spec
#
# A node's resolved namespace URI is INVISIBLE to serialization.
#
#   <r xmlns:p="urn:p"><p:a/></r>
#
# writes the prefix `p:` and nothing else, so the fact that `p:a` stands for
# `urn:p` never reaches the output. Comparing serialized strings - which is what
# a round-trip spec does - walks straight past this whole surface. What notices
# is XPath, which matches name tests on the resolved URI: get it wrong and only
# the queries are quietly wrong.
#
# So: build a namespaced document, apply a random edit sequence, and compare
#   (a) the mutated tree
#   (b) that tree serialized and read back
# node by node on (kind, local name, namespace URI, attributes). No second
# implementation is needed - the oracle is the parser itself.
#
# The fingerprint compares LOCAL names and namespace URIs, not qualified names,
# and skips xmlns attributes. Both are deliberate: the serializer picks the
# prefixes and the declarations the output needs, and may invent a prefix where
# one would otherwise have to mean two things. What must round-trip is the
# namespace a node is in, not the spelling it arrives under.

require_relative "support"

NS_URIS = ["urn:a", "urn:b", "urn:c"].freeze
PREFIXES = %w[p q].freeze
LOCALS = %w[a b c entry title item].freeze

# A document with namespaces scattered through it: elements that declare and
# elements that do not, a default namespace, prefixed elements and attributes.
def build_doc(rng)
  decls = +""
  decls << %( xmlns:#{PREFIXES[0]}="#{NS_URIS[0]}")
  decls << %( xmlns:#{PREFIXES[1]}="#{NS_URIS[1]}")   # bind both at the root: an
                                                      # unbound prefix makes the
                                                      # document itself invalid,
                                                      # which is not what is
                                                      # being measured
  decls << %( xmlns="#{NS_URIS[2]}") if rng.chance(4, 10)

  body = (0...(2 + rng.next_int(3))).map { build_elem(rng, 2) }.join
  %(<root#{decls}>#{body}</root>)
end

def build_elem(rng, depth)
  local = rng.pick(LOCALS)
  pfx = rng.chance(5, 10) ? rng.pick(PREFIXES) : nil
  name = pfx ? "#{pfx}:#{local}" : local

  attrs = +""
  attrs << %( id="#{rng.next_int(100)}") if rng.chance(6, 10)
  # a declaration that changes what descendants resolve to - the target
  attrs << %( xmlns:#{PREFIXES[0]}="#{rng.pick(NS_URIS)}") if rng.chance(3, 10)
  attrs << %( #{rng.pick(PREFIXES)}:k="v") if rng.chance(3, 10)

  return "<#{name}#{attrs}/>" if depth.zero? || rng.chance(4, 10)

  kids = (0...(1 + rng.next_int(2))).map { build_elem(rng, depth - 1) }.join
  kids += "text" if rng.chance(3, 10)
  "<#{name}#{attrs}>#{kids}</#{name}>"
end

# Edits chosen to disturb namespace state.
def apply_edit(rng, doc)
  return "root-gone" if doc.root.nil?

  els = elements(doc.root) << doc.root
  target = rng.pick(els)
  return "no-target" if target.nil?

  desc = nil
  case rng.next_int(7)
  when 0 # add a declaration
    pfx = rng.pick(PREFIXES); uri = rng.pick(NS_URIS)
    target["xmlns:#{pfx}"] = uri
    desc = "addDecl #{target.name} xmlns:#{pfx}=#{uri}"
  when 1 # remove one
    decl = target.attribute_nodes.find { |a| a.namespace_uri == "http://www.w3.org/2000/xmlns/" }
    (target.delete(decl.name); desc = "delDecl #{target.name} #{decl.name}") if decl
  when 2 # insert a prefixed element
    pfx = rng.pick(PREFIXES)
    local = rng.pick(LOCALS)
    begin
      frag = doc.fragment("<#{pfx}:#{local}><#{pfx}:inner/></#{pfx}:#{local}>")
      child = frag.children.first
      (target.add_child(child); desc = "insertPrefixed #{target.name} <- #{pfx}:#{local}") if child
    rescue StandardError
      nil # an unbound prefix is refused; being refused is not the finding
    end
  when 3 # move a subtree
    src = rng.pick(els)
    if src && !src.equal?(target) && !ancestor?(src, target)
      begin
        src_name = src.name; target.add_child(src); desc = "move #{src_name} -> #{target.name}"
      rescue StandardError
        nil # cycles and the like are refused; not the finding
      end
    end
  when 4 # replace with an element of another name (the DOM has no rename)
    pfx = rng.chance(7, 10) ? "#{rng.pick(PREFIXES)}:" : ""
    begin
      nn = "#{pfx}#{rng.pick(LOCALS)}"; old = target.name
      target.replace(doc.create_element(nn)); desc = "replace #{old} with #{nn}"
    rescue StandardError
      nil
    end
  when 5 # add a prefixed attribute
    begin
      an = "#{rng.pick(PREFIXES)}:#{rng.pick(LOCALS)}"; target[an] = "v"; desc = "setAttr #{target.name} #{an}"
    rescue StandardError
      nil
    end
  else # remove
    (target.remove; desc = "remove #{target.name}") unless target.equal?(doc.root)
  end
  desc
end

def ancestor?(node, maybe_desc)
  n = maybe_desc
  while n
    return true if n.equal?(node)

    n = n.parent
  end
  false
end

ordinary_probe = Makiri::XML(%(<r xmlns="urn:d" xmlnsfoo="must-survive"/>))
ordinary_before = fingerprint(ordinary_probe)
ordinary_probe.root.delete("xmlnsfoo")
raise "fingerprint dropped the ordinary xmlnsfoo attribute" if fingerprint(ordinary_probe) == ordinary_before

declaration_probe = Makiri::XML(%(<r xmlns:p="urn:a"><x/></r>))
declaration_before = fingerprint(declaration_probe)
declaration_probe.root["xmlns:p"] = "urn:b"
raise "fingerprint included a namespace declaration" unless fingerprint(declaration_probe) == declaration_before

count = (ARGV[0] || 2000).to_i
seed  = (ARGV[1] || 20_260_911).to_i

stats = Hash.new(0)
ops = Hash.new(0)   # which edits appeared in the failing cases
diffs = []

count.times do |i|
  rng = Rng.new(seed + i * 7919)
  xml = build_doc(rng)
  doc = begin
    Makiri::XML(xml)
  rescue StandardError
    stats[:bad_doc] += 1
    next
  end

  log = Array.new(1 + rng.next_int(4)) { apply_edit(rng, doc) }.compact

  if doc.root.nil?
    # An edit sequence that removes the root is a harness artefact, not an
    # invariant failure; counted, not reported.
    stats[:root_gone] += 1
    next
  end

  mutated = fingerprint(doc)
  serialized = doc.root.to_xml
  reparsed = begin
    fingerprint(Makiri::XML(serialized))
  rescue StandardError => e
    stats[:reparse_error] += 1
    log.each { |x| ops[x.split(" ").first.to_s] += 1 }
    if diffs.length < 6
      diffs << [i, xml, "#{log.join(' ; ')}\n                -> #{serialized}",
                "reparse raised #{e.class}: #{e.message}", nil]
    end
    next
  end

  if mutated == reparsed
    stats[:agree] += 1
  else
    stats[:differ] += 1
    log.each { |x| ops[x.split(" ").first.to_s] += 1 }
    if diffs.length < 6
      pair = mutated.zip(reparsed).find { |a, b| a != b }
      diffs << [i, xml, "#{log.join(' ; ')}\n                -> #{serialized}", pair&.first, pair&.last]
    end
  end
end

puts "=" * 72
puts "namespace state after mutation vs serialize -> re-parse"
puts "  documents       : #{count}"
puts "  agree           : #{stats[:agree]}"
puts "  differ          : #{stats[:differ]}"
puts "  reparse failed  : #{stats[:reparse_error]}"
puts "  unparsable doc  : #{stats[:bad_doc]}"
puts "  root removed    : #{stats[:root_gone]}"
unless ops.empty?
  puts "  edits in the failures: " + ops.sort_by { |_, v| -v }.map { |k, v| "#{k}=#{v}" }.join(" ")
end

diffs.each do |(i, xml, serialized, a, b)|
  puts
  puts "-" * 72
  puts "##{i}"
  puts "  initial     : #{xml[0, 200]}"
  puts "  edits / out : #{serialized}"
  puts "  mutated     : #{a.inspect[0, 220]}"
  puts "  re-parsed   : #{b.inspect[0, 220]}"
end

exit(stats[:differ] + stats[:reparse_error] > 0 ? 1 : 0)
