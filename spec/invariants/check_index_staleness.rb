# frozen_string_literal: true
#
# The cached per-document indexes are never stale after a mutation.
#
#   ruby -Ilib spec/invariants/check_index_staleness.rb [documents] [seed] [html|xml]
#
# ## What is at stake
#
# Makiri builds three indexes lazily per document and drops them from the
# mutation hook:
#
#   attr->owner  dom_adapter/dom_index.c   an attribute's owning element; the
#                                          parent and ancestor axes need it
#   element      (rides along)             tag id -> elements, for the //tag path
#   text         dom_adapter/text_index.c  a flat array of BORROWED slices, one
#                                          per text node
#
# If one mutation path forgets to invalidate, queries quietly return the wrong
# answer. And because the text index holds borrowed slices, staleness there is a
# memory bug as well - the reason mkr_xml_node.c's comment insists both indexes
# drop from the SAME hook, so a borrowed slice can never point at storage that
# has been reallocated or detached.
#
# spec/text_index_spec.rb checks a hand-written set of cases against a plain
# walk. What this adds is the mutation verbs, exhaustively and at random.
#
# ## How
#
# After every edit, compare the path that uses an index against one that does
# not. The walking side reads only leaf values (a text node's own `content`), so
# it never goes through the subtree index.
#
#   T1 Node#text        text index         vs a recursive walk of the children
#   T2 //tag            element index      vs walking and filtering by name
#   T3 //@a/..          attr->owner        vs collecting elements with that attribute
#   T4 //*[@a='v']      attribute fast path vs walking and filtering by value
#   T5 at_xpath(e)      first-match path   vs xpath(e).first
#   T6 Node#path        the round trip     vs at_xpath(path) landing back
#
# Under ASan this is also where a borrowed slice pointing at freed storage would
# surface:
#
#   MAKIRI_SANITIZE=address,undefined bundle exec rake compile
#   DYLD_INSERT_LIBRARIES=<asan.dylib> ruby -Ilib spec/invariants/check_index_staleness.rb 500

require_relative "support"

TAGS = %w[div span b i p section ul li].freeze
IDS = %w[k0 k1 k2 k3].freeze

def build_el(rng, depth)
  tag = rng.pick(TAGS)
  attrs = rng.chance(7, 10) ? %( id="#{rng.pick(IDS)}") : ""
  attrs += %( data-v="#{rng.next_int(4)}") if rng.chance(5, 10)
  return "<#{tag}#{attrs}></#{tag}>" if depth.zero? || rng.chance(3, 10)

  kids = Array.new(1 + rng.next_int(2)) { build_el(rng, depth - 1) }.join
  kids += "t#{rng.next_int(100)}" if rng.chance(6, 10)
  "<#{tag}#{attrs}>#{kids}</#{tag}>"
end

def build_html(rng)
  "<!DOCTYPE html><html><head><title>t</title></head><body>" \
    "#{Array.new(2 + rng.next_int(3)) { build_el(rng, 2) }.join}</body></html>"
end

def build_xml(rng)
  "<root>#{Array.new(2 + rng.next_int(3)) { build_el(rng, 2) }.join}</root>"
end

# --- the paths that do NOT use an index -------------------------------

# A subtree's text, read only from the leaves, so the subtree index is never
# consulted.
def walk_text(node, out = +"")
  node.children.each do |c|
    case c.node_type
    when 3, 4 then out << c.content
    when 1 then walk_text(c, out)
    end
  end
  out
end

class Stale < StandardError; end

def ids(nodes) = nodes.map(&:pointer_id)

def check_indexes(rng, doc)
  bad = []
  root = doc.root
  all = elements(root)

  # T1
  targets = [root]
  targets << rng.pick(all) unless all.empty?
  targets.each do |n|
    got = n.text
    want = walk_text(n)
    bad << "T1 #{n.name}: text differs from the walk (#{got.bytesize}B vs #{want.bytesize}B)" if got != want
  end

  # T2
  tag = rng.pick(TAGS)
  got = ids(doc.xpath("//#{tag}").to_a)
  want = ids(all.select { |e| e.name == tag })
  bad << "T2 //#{tag}: #{got.length} vs #{want.length} from the walk" if got != want

  # T3
  got = ids(doc.xpath("//@id/..").to_a)
  want = ids(all.select { |e| e.attribute_nodes.any? { |a| a.name == "id" } })
  bad << "T3 //@id/..: #{got.length} vs #{want.length} from the walk" if got != want

  # T4
  id = rng.pick(IDS)
  got = ids(doc.xpath("//*[@id='#{id}']").to_a)
  want = ids(all.select { |e| e["id"] == id })
  bad << "T4 //*[@id='#{id}']: #{got.length} vs #{want.length} from the walk" if got != want

  # T5
  expr = rng.chance(1, 2) ? "//#{tag}" : "//*[@id='#{id}']"
  a = doc.at_xpath(expr)
  b = doc.xpath(expr).to_a.first
  bad << "T5 at_xpath(#{expr}) differs from xpath(...).first" unless a&.pointer_id == b&.pointer_id

  # T6. The generators use no prefixes, so a path is unambiguous here.
  unless all.empty?
    n = rng.pick(all)
    back = begin
      doc.at_xpath(n.path)
    rescue StandardError => e
      bad << "T6 #{n.path} does not evaluate: #{e.class}"
      nil
    end
    bad << "T6 #{n.path} resolved to a different node" if back && back.pointer_id != n.pointer_id
  end

  bad
end

# Edits that can leave an index behind, weighted towards the ones that move text
# around.
def apply_edit(rng, doc)
  body = container_of(doc) or return nil
  els = elements(body)
  return nil if els.empty?

  target = rng.pick(els)

  case rng.next_int(12)
  when 0 then (target.add_child(doc.create_element(rng.pick(TAGS))); "add_child")
  when 1 then (target.add_child(doc.create_text_node("x#{rng.next_int(1000)}")); "add_text")
  when 2
    return nil if target.parent.nil?

    target.remove
    "remove"
  when 3 then (target.content = "c#{rng.next_int(1000)}"; "content=")
  when 4 then (target.content = ""; "content=empty")
  when 5 then (target["id"] = rng.pick(IDS); "setId")
  when 6 then (target.delete("id"); "delId")
  when 7
    return nil if target.parent.nil?

    target.replace(doc.create_element(rng.pick(TAGS)))
    "replace"
  when 8
    src = rng.pick(els)
    return nil if src.pointer_id == target.pointer_id

    begin
      target.add_child(src)
      "move"
    rescue Makiri::Error
      "move-rejected"
    end
  when 9 then (target.add_child(doc.fragment("<b>f#{rng.next_int(100)}</b>tail")); "fragment")
  when 10 then (body.add_child(target.clone_node(true)); "clone")
  else # remove a text node outright, moving the text index's runs
    t = target.children.find { |c| c.node_type == 3 }
    return nil if t.nil?

    t.remove
    "removeText"
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
kinds = Hash.new(0)

count.times do |i|
  rng = Rng.new(seed + i * 7919)
  doc = xml ? Makiri::XML(build_xml(rng)) : Makiri::HTML(build_html(rng))
  log = []

  begin
    # Query once up front so the indexes are actually built - checking only the
    # unbuilt state would never show a missing invalidation.
    check_indexes(rng, doc)

    (2 + rng.next_int(10)).times do
      desc = apply_edit(rng, doc)
      next if desc.nil?

      log << desc
      op_counts[desc] += 1

      bad = check_indexes(rng, doc)
      unless bad.empty?
        bad.each { |b| kinds[b[0, 2]] += 1 }
        raise Stale, bad.join(" | ")
      end
    end
    stats[:ok] += 1
  rescue Stale => e
    stats[:stale] += 1
    failures << [i, log, e.message] if failures.length < 5
  rescue StandardError => e
    stats[:error] += 1
    failures << [i, log, "#{e.class}: #{e.message}"] if failures.length < 5
  end
end

puts "=" * 72
puts "#{backend.upcase} - index staleness (#{count} documents)"
puts "  agree            : #{stats[:ok]}"
puts "  stale            : #{stats[:stale]}"
puts "  unexpected error : #{stats[:error]}"
puts "  broken checks    : #{kinds.sort.map { |k, v| "#{k}=#{v}" }.join(' ')}" unless kinds.empty?
puts "  edits applied    : " + op_counts.sort_by { |_, v| -v }.map { |k, v| "#{k}=#{v}" }.join(" ")

failures.each do |(i, log, msg)|
  puts
  puts "-" * 72
  puts "##{i}"
  puts "  edits : #{log.join(' ; ')[0, 300]}"
  puts "  broke : #{msg[0, 400]}"
end

exit(stats[:stale] + stats[:error] > 0 ? 1 : 0)
