# frozen_string_literal: true
#
# Tree-shape invariants hold after every mutation, on both backends.
#
#   ruby -Ilib spec/invariants/check_tree_invariants.rb [documents] [seed] [html|xml]
#
# ## Why the oracle is the invariants themselves
#
# The other checks here compare a mutated tree against re-parsing its own
# output. That does not work for HTML, because HTML parsing is not idempotent:
# put a <div> inside a <p>, serialize, read it back, and the <p> has closed
# around it and they are siblings. The tree changed, and HTML5 says it should.
# Run that comparison and the output is a pile of false positives.
#
# So the oracle here is the shape of the tree itself - no reference
# implementation, no re-parse, and a broken tree is caught directly. It is also
# the right target: the doubly-linked child list is exactly what the mutation
# primitives maintain.
#
#   I1 parent/child agree    a child's parent points back at the parent
#   I2 prev/next agree       next's prev is this node, prev's next is this node
#   I3 ends                  first_child has no prev, last_child has no next
#   I4 uniqueness            no node appears at two places in the tree
#   I5 acyclic               a node is not among its own descendants
#   I6 detachment            a removed node has no parent
#   I7 reachability          walking parents from any node reaches the root
#
# Identity is `pointer_id`: Makiri hands out a fresh Ruby wrapper on every
# traversal, so `object_id` says nothing (this is also why Dommy keys on
# pointer_id).
#
# Everything is checked after EVERY edit, so a break stops on the operation that
# caused it. The other document is checked too: after a cross-document insert,
# the far side being intact matters as much as this one.
#
# Under ASan this also catches the use-after-free and the overwrite that would
# precede a broken invariant:
#
#   MAKIRI_SANITIZE=address,undefined bundle exec rake compile
#   DYLD_INSERT_LIBRARIES=<asan.dylib> ruby -Ilib spec/invariants/check_tree_invariants.rb 500

require_relative "support"

TAGS = %w[div span b i section ul li].freeze
MAX_DEPTH = 40

def build_html(rng)
  body = Array.new(2 + rng.next_int(3)) { build_el(rng, 2) }.join
  "<!DOCTYPE html><html><head><title>t</title></head><body>#{body}</body></html>"
end

def build_xml(rng)
  "<root>#{Array.new(2 + rng.next_int(3)) { build_el(rng, 2) }.join}</root>"
end

def build_el(rng, depth)
  tag = rng.pick(TAGS)
  attrs = rng.chance(6, 10) ? %( id="n#{rng.next_int(1000)}") : ""
  attrs += %( class="c#{rng.next_int(5)}") if rng.chance(4, 10)
  return "<#{tag}#{attrs}></#{tag}>" if depth.zero? || rng.chance(3, 10)

  kids = Array.new(1 + rng.next_int(2)) { build_el(rng, depth - 1) }.join
  kids += "text#{rng.next_int(100)}" if rng.chance(4, 10)
  kids += "<!--c-->" if rng.chance(2, 10)
  "<#{tag}#{attrs}>#{kids}</#{tag}>"
end

class Violation < StandardError; end

# Returns the violations as strings; empty means sound.
def check_tree(root)
  bad = []
  seen = {} # pointer_id => the path it was found at

  walk = lambda do |node, depth, path|
    raise Violation, "I5 deeper than #{MAX_DEPTH} (a cycle?): #{path}" if depth > MAX_DEPTH

    key = node.pointer_id
    if seen.key?(key)
      bad << "I4 the same node appears twice: #{path} and #{seen[key]}"
      return
    end
    seen[key] = path

    kids = node.children.to_a
    kids.each_with_index do |c, i|
      here = "#{path}/#{c.name}[#{i}]"

      p = c.parent
      bad << "I1 a child's parent is not the parent: #{here}" if p.nil? || p.pointer_id != key

      prev = c.previous_sibling
      nxt = c.next_sibling
      if i.zero?
        bad << "I3 first_child has a prev: #{here}" unless prev.nil?
      elsif prev.nil? || prev.pointer_id != kids[i - 1].pointer_id
        bad << "I2 prev is not the preceding sibling: #{here}"
      end
      if i == kids.length - 1
        bad << "I3 last_child has a next: #{here}" unless nxt.nil?
      elsif nxt.nil? || nxt.pointer_id != kids[i + 1].pointer_id
        bad << "I2 next is not the following sibling: #{here}"
      end

      walk.call(c, depth + 1, here)
    end
  end

  walk.call(root, 0, root.name.to_s)
  bad
end

def reaches_root?(node, root)
  n = node
  steps = 0
  while n && steps <= MAX_DEPTH
    return true if n.pointer_id == root.pointer_id

    n = n.parent
    steps += 1
  end
  false
end

def ancestor?(node, maybe_desc)
  n = maybe_desc
  steps = 0
  while n && steps <= MAX_DEPTH
    return true if n.pointer_id == node.pointer_id

    n = n.parent
    steps += 1
  end
  false
end

def retained_snapshot(node)
  value = node.node_type == 2 ? node.value : node.content
  [node.class.name, node.node_type, node.name, node.namespace_uri, value]
end

def retain(retained, node)
  retained << [node, retained_snapshot(node)] if node && retained.length < 32
end

def check_retained(doc, retained)
  return if retained.empty?

  connected = ([doc.root] + elements(doc.root)).compact
  connected.concat(connected.flat_map { |node| node.attribute_nodes.to_a })
  connected_ids = connected.map(&:pointer_id)
  retained.each do |node, expected|
    raise Violation, "detached wrapper became connected: #{node.name}" if connected_ids.include?(node.pointer_id)
    raise Violation, "detached wrapper regained a parent: #{node.name}" unless node.parent.nil?
    raise Violation, "detached wrapper changed: #{node.name}" unless retained_snapshot(node) == expected
  rescue Violation
    raise
  rescue StandardError => e
    raise Violation, "detached wrapper became unreadable: #{e.class}: #{e.message}"
  end
end

# The mutation surface Dommy exercises. An operation the implementation refuses
# (a cycle, say) counts as normal - what must hold is that the tree survives the
# refusal intact, which is checked after every edit either way.
def apply_edit(rng, doc, other = nil, retained = nil)
  body = container_of(doc) or return nil
  els = elements(body)
  return nil if els.empty?

  target = rng.pick(els)

  case rng.next_int(13)
  when 0
    target.add_child(doc.create_element(rng.pick(TAGS)))
    "add_child"
  when 1
    return nil if target.parent.nil?

    target.add_previous_sibling(doc.create_element(rng.pick(TAGS)))
    "before"
  when 2
    return nil if target.parent.nil?

    target.add_next_sibling(doc.create_element(rng.pick(TAGS)))
    "after"
  when 3
    return nil if target.parent.nil?

    retain(retained, target)
    target.remove
    "remove"
  when 4 # a move; into its own subtree the implementation must refuse
    src = rng.pick(els)
    return nil if src.pointer_id == target.pointer_id

    if ancestor?(src, target)
      begin
        target.add_child(src)
        "*** a cycle was accepted #{src.name} -> #{target.name}"
      rescue StandardError
        "cycle-rejected"
      end
    else
      target.add_child(src)
      "move"
    end
  when 5
    return nil if target.parent.nil?

    retain(retained, target)
    target.replace(doc.create_element(rng.pick(TAGS)))
    "replace"
  when 6
    target["data-k"] = "v#{rng.next_int(100)}"
    "setAttr"
  when 7
    attr = target.attribute_nodes.find { |a| a.name == "id" }
    target.delete("id")
    retain(retained, attr)
    "delAttr"
  when 8
    target.children.to_a.each { |child| retain(retained, child) }
    target.content = "z#{rng.next_int(100)}"
    "content="
  when 9
    target.add_child(doc.fragment("<b>f</b>tail<i>g</i>"))
    "fragment"
  when 10
    body.add_child(target.clone_node(true))
    "clone"
  when 11 # import from the other document (the path Dommy's adopt takes)
    return nil if other.nil?

    src = elements(container_of(other))
    return nil if src.empty?

    target.add_child(doc.import_node(rng.pick(src), true))
    "import"
  else # a node from the other document, inserted directly: this ADOPTS it
    return nil if other.nil?

    src_container = container_of(other)
    src = elements(src_container)
    return nil if src.empty?

    node = rng.pick(src)
    before = src_container.children.length
    detached = node.parent.nil?
    begin
      ins = target.add_child(node)
      if ins.document.equal?(other)
        "*** a foreign node was linked while still owned by its document"
      elsif !detached && src_container.children.length == before && node.parent
        "*** adopted but not removed from the source document"
      else
        "foreign-adopted"
      end
    rescue Makiri::Error
      "foreign-rejected"
    end
  end
end

count   = (ARGV[0] || 500).to_i
seed    = (ARGV[1] || 20_260_911).to_i
backend = (ARGV[2] || "html").downcase
abort "backend must be html or xml" unless %w[html xml].include?(backend)
xml = backend == "xml"

stats = Hash.new(0)
failures = []
op_counts = Hash.new(0)

count.times do |i|
  rng = Rng.new(seed + i * 7919)
  make = xml ? ->(r) { Makiri::XML(build_xml(r)) } : ->(r) { Makiri::HTML(build_html(r)) }
  doc = make.call(rng)
  other = make.call(rng)                  # the far side of the cross-document edits
  log = []
  retained = []

  begin
    (2 + rng.next_int(10)).times.with_index(1) do |_, edit_number|
      desc = begin
        apply_edit(rng, doc, other, retained)
      rescue Makiri::Error => e
        "rejected(#{e.message[0, 40]})"
      end
      unless desc.nil?
        log << desc
        op_counts[desc.split(" ").first] += 1
      end

      bad = check_tree(doc.root) + check_tree(other.root).map { |m| "[other] #{m}" }
      elements(doc.root).each do |e|
        unless reaches_root?(e, doc.root)
          bad << "I7 walking parents does not reach the root: #{e.name}"
          break
        end
      end
      bad << desc if desc&.start_with?("***")

      raise Violation, bad.join(" | ") unless bad.empty?

      if (edit_number % 8).zero?
        GC.start
        GC.compact
      end
      check_retained(doc, retained)
    end
    unless retained.empty?
      GC.start
      GC.compact
      check_retained(doc, retained)
    end
    stats[:ok] += 1
  rescue Violation => e
    stats[:violation] += 1
    failures << [i, log, e.message] if failures.length < 5
  rescue StandardError => e
    stats[:error] += 1
    failures << [i, log, "#{e.class}: #{e.message}"] if failures.length < 5
  end
end

puts "=" * 72
puts "#{backend.upcase} mutation - tree invariants (#{count} documents)"
puts "  sound              : #{stats[:ok]}"
puts "  invariant broken   : #{stats[:violation]}"
puts "  unexpected error   : #{stats[:error]}"
puts "  edits applied      : " + op_counts.sort_by { |_, v| -v }.map { |k, v| "#{k}=#{v}" }.join(" ")

failures.each do |(i, log, msg)|
  puts
  puts "-" * 72
  puts "##{i}"
  log.last(12).each { |l| puts "  edit  : #{l}" }
  puts "  broke : #{msg[0, 400]}"
end

exit(stats[:violation] + stats[:error] > 0 ? 1 : 0)
