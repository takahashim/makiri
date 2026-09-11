# frozen_string_literal: true
#
# Serialization neither adds nor loses anything, and settles.
#
#   ruby -Ilib spec/invariants/check_serialize_fixpoint.rb [documents] [seed] [html|xml]
#
# ## Why
#
# A DOM layer's innerHTML / outerHTML goes straight through here, and a missed
# escape in an attribute value is an injection: if
# `node["data-x"] = %q{a" onerror="evil()}` closes the quote on the way out, the
# tree read back carries an event handler nobody put there.
#
# ## Two properties, because one is not enough
#
#   t  = the tree (parsed, then edited)
#   s1 = serialize(t)
#   t1 = parse(s1)
#   s2 = serialize(t1)
#
#   A (faithful)  content_fingerprint(t) == content_fingerprint(t1) - nothing added or lost
#   F (fixpoint)  s1 == s2                          - the shape does not drift
#
# F alone cannot catch the injection. With the escape broken, s1 says
# `data-p="a" onerror="evil()"` and t1 ALREADY has two attributes; everything
# from there is stable and F passes. A is what notices. (The negative control
# below demonstrates exactly this.)
#
# ## What the generator stays away from
#
# A is only a fair question where the parser can reproduce the tree, and editing
# leaves that range easily:
#
#   - <ul> inside <p>: HTML5 closes the <p>, so they come back as siblings;
#   - <li> inside <span>: likewise;
#   - adjacent text nodes: the DOM allows them, serialization has no way to
#     write the boundary.
#
# None of that is a serializer defect. So the generator uses only elements that
# nest freely (div / span / b / i / section), and the fingerprint joins adjacent
# text before comparing. Mixing them in buries the real divergences under
# spec-conformant noise.
#
# ## Where the spec does not promise a fixpoint
#
# The HTML serialization algorithm writes two things literally:
#
#   (a) the children of style / script / xmp / iframe / noembed / noframes /
#       plaintext
#   (b) comment data
#
# so a `</script>` placed in a script element's content escapes the element, and
# so does `-->` in a comment. Browsers do the same. Those are pinned in a table
# at the end rather than asserted as round trips - if the pinned value ever
# moves, that is worth knowing, but it is not a fixpoint failure.

require_relative "support"

# Elements that nest freely; p / ul / li would be re-shaped by HTML5's
# auto-closing rules and are not a fair target for A.
TAGS = %w[div span b i section].freeze

# Hostile values, placed in both attributes and text.
PAYLOADS = [
  %(a" onerror="evil()),
  %(a' onerror='evil()),
  "a onerror=evil",
  "<script>evil()</script>",
  "a>b<c",
  "a&b",
  "a&amp;b",
  "&lt;script&gt;",
  "a\nb",
  "a\tb",
  "`a`",
  "a  b",
  "]]>",
  " x",          # NBSP; HTML serialization writes &nbsp;
  "",
].freeze

# The elements whose children the algorithm writes literally. The generator
# stays out of them.
SPEC_RAW_TEXT = %w[style script xmp iframe noembed noframes plaintext].freeze

def build_el(rng, depth)
  tag = rng.pick(TAGS)
  attrs = rng.chance(6, 10) ? %( id="i#{rng.next_int(100)}") : ""
  return "<#{tag}#{attrs}></#{tag}>" if depth.zero? || rng.chance(3, 10)

  kids = Array.new(1 + rng.next_int(2)) { build_el(rng, depth - 1) }.join
  kids += "t#{rng.next_int(100)}" if rng.chance(5, 10)
  kids += "<!--c#{rng.next_int(10)}-->" if rng.chance(3, 10)
  "<#{tag}#{attrs}>#{kids}</#{tag}>"
end

def build_html(rng)
  "<!DOCTYPE html><html><head><title>t</title></head><body>" \
    "#{Array.new(2 + rng.next_int(3)) { build_el(rng, 2) }.join}</body></html>"
end

def build_xml(rng)
  "<root>#{Array.new(2 + rng.next_int(3)) { build_el(rng, 2) }.join}</root>"
end

# This check needs a DIFFERENT key from support.rb's `fingerprint`: it asks
# whether serialization lost or added anything, not which namespace a node is
# in, so it compares qualified names and ignores namespaces entirely.
#
# Attributes by name and value, order-insensitive. Adjacent text is joined:
# the DOM can hold the boundary, serialization cannot write it, and nothing
# requires it to survive - so counting it as a divergence would be wrong.
def content_fingerprint(node, out = [])
  node.children.each do |c|
    case c.node_type
    when 1
      out << [1, c.name, c.attribute_nodes.map { |a| [a.name, a.value] }.sort]
      content_fingerprint(c, out)
    when 3, 4
      if out.last && out.last[0] == 3
        out[-1] = [3, out.last[1] + c.content]
      else
        out << [3, c.content]
      end
    when 8 then out << [8, c.content]
    when 7 then out << [7, c.name, c.content]
    end
  end
  out.reject { |e| e[0] == 3 && e[1].empty? }   # empty text is not written
end

# Scatter the hostile values across attributes and text, staying out of the
# places the algorithm writes literally.
def apply_edit(rng, doc)
  body = container_of(doc) or return nil
  els = elements(body).reject { |e| SPEC_RAW_TEXT.include?(e.name) }
  return nil if els.empty?

  target = rng.pick(els)
  v = rng.pick(PAYLOADS)

  case rng.next_int(6)
  when 0 then (target["data-p"] = v; "setAttr")
  when 1 then (target["id"] = v; "setId")
  when 2 then (target.content = v; "content=")
  when 3 then (target.add_child(doc.create_text_node(v)); "addText")
  when 4 then (target.add_child(doc.create_element(rng.pick(TAGS))); "addEl")
  else
    return nil if target.parent.nil?

    target.remove
    "remove"
  end
end

count   = (ARGV[0] || 2000).to_i
seed    = (ARGV[1] || 20_260_911).to_i
backend = (ARGV[2] || "html").downcase
abort "backend must be html or xml" unless %w[html xml].include?(backend)
xml = backend == "xml"

stats = Hash.new(0)
failures = []

count.times do |i|
  rng = Rng.new(seed + i * 7919)
  doc = xml ? Makiri::XML(build_xml(rng)) : Makiri::HTML(build_html(rng))
  log = []

  begin
    (1 + rng.next_int(6)).times do
      d = apply_edit(rng, doc)
      log << d if d
    end

    ser = xml ? ->(d) { d.root.to_xml } : ->(d) { d.to_html }
    parse = xml ? ->(s) { Makiri::XML(s) } : ->(s) { Makiri::HTML(s) }

    s1 = ser.call(doc)
    t1 = parse.call(s1)
    s2 = ser.call(t1)
    s3 = ser.call(parse.call(s2))

    f0 = content_fingerprint(doc.root)
    f1 = content_fingerprint(t1.root)

    if f0 != f1
      stats[:tree_differ] += 1
      if failures.length < 5
        pair = f0.zip(f1).find { |a, b| a != b }
        failures << [i, log, "A faithful: the tree read back differs",
                     pair&.first.inspect, pair&.last.inspect]
      end
    elsif s2 != s3
      # not settled even after one round: the real failure
      stats[:not_fixed] += 1
      if failures.length < 5
        j = (0...[s2.length, s3.length].max).find { |k| s2[k] != s3[k] }
        failures << [i, log, "F fixpoint: s2 != s3 (at #{j})",
                     s2[[0, j - 40].max, 90].inspect, s3[[0, j - 40].max, 90].inspect]
      end
    elsif s1 != s2
      # settles after one round; an empty text node, which libxml2 does too
      stats[:settles] += 1
      if stats[:settles] <= 2 && failures.length < 5
        j = (0...[s1.length, s2.length].max).find { |k| s1[k] != s2[k] }
        failures << [i, log, "(informational) s1 != s2 but s2 == s3",
                     s1[[0, j - 40].max, 90].inspect, s2[[0, j - 40].max, 90].inspect]
      end
    else
      stats[:ok] += 1
    end
  rescue StandardError => e
    stats[:error] += 1
    failures << [i, log, "#{e.class}", e.message[0, 120], nil] if failures.length < 5
  end
end

puts "=" * 72
puts "#{backend.upcase} - serialization fidelity and fixpoint (#{count} documents)"
puts "  both hold        : #{stats[:ok]}"
puts "  A broken         : #{stats[:tree_differ]}"
puts "  F broken         : #{stats[:not_fixed]}   (s2 != s3: never settles)"
puts "  settles in one   : #{stats[:settles]}   (s1 != s2, s2 == s3: an empty text node)"
puts "  error            : #{stats[:error]}"

failures.each do |(i, log, why, a, b)|
  puts
  puts "-" * 72
  puts "##{i}  #{why}"
  puts "  edits : #{log.join(' ; ')[0, 200]}"
  puts "  got   : #{a}"
  puts "  want  : #{b}" if b
end

# --- negative control: does A actually catch an injection? --------------
#
# A green result means nothing until it has been shown it can go red. Undo the
# escaping in Makiri's own output and feed that back: the same input a
# serializer that forgot to escape a quote would produce.

puts
puts "=" * 72
puts "negative control - A against a broken escaper"

probe = Makiri::HTML(%(<html><body><x id="t"></x></body></html>))
probe.at_css("x")["data-p"] = %(a" onerror="evil())
good = probe.to_html
broken = good.gsub("&quot;", '"')

good_fp = content_fingerprint(Makiri::HTML(good).root)
broken_fp = content_fingerprint(Makiri::HTML(broken).root)
orig_fp = content_fingerprint(probe.root)

def attrs_of(fp) = fp.find { |e| e[0] == 1 && e[1] == "x" }&.last

puts "  correct : #{good[/<x[^>]*>/].inspect}"
puts "            #{attrs_of(good_fp).inspect}"
puts "  broken  : #{broken[/<x[^>]*>/].inspect}"
puts "            #{attrs_of(broken_fp).inspect}"

teeth_ok = (orig_fp == good_fp) && (orig_fp != broken_fp)
if teeth_ok
  puts "  -> A passes the correct output and catches the injection"
else
  puts "  *** the negative control does not hold: A is not seeing the injection"
  stats[:no_teeth] = 1
end

# --- pin the places the spec writes literally ---------------------------

unless xml
  puts
  puts "=" * 72
  puts "written literally by the algorithm (no fixpoint promised; pinned)"

  pinned = []

  SPEC_RAW_TEXT.each do |tag|
    d = Makiri::HTML("<html><head></head><body></body></html>")
    host = d.at_css("head") || d.at_css("body")
    el = d.create_element(tag)
    host.add_child(el)
    el.add_child(d.create_text_node("a<b&c"))
    inner = el.to_html[/<#{tag}[^>]*>(.*)<\/#{tag}>/m, 1]
    pinned << [tag, inner == "a<b&c" ? "literal" : "escaped", inner.to_s]
  end

  # noscript is parsed with scripting disabled - its children are elements, not
  # raw text - so escaping is what makes it round-trip.
  d = Makiri::HTML("<html><body><noscript></noscript></body></html>")
  el = d.at_css("noscript")
  el.add_child(d.create_text_node("a<b&c"))
  inner = el.to_html[/<noscript[^>]*>(.*)<\/noscript>/m, 1]
  pinned << ["noscript", inner == "a<b&c" ? "literal" : "escaped", inner.to_s]

  d = Makiri::HTML("<html><body><!--x--></body></html>")
  c = d.at_css("body").children.first
  c.content = "a-->b"
  ser = d.at_css("body").inner_html
  pinned << ["comment", ser == "<!--a-->b-->" ? "literal" : "escaped", ser]

  pinned.each { |(what, mode, out)| puts format("  %-11s %-8s %s", what, mode, out.inspect[0, 44]) }

  expected = {
    "style" => "literal", "script" => "literal", "xmp" => "literal",
    "iframe" => "literal", "noembed" => "literal", "noframes" => "literal",
    "plaintext" => "literal",
    "noscript" => "escaped",   # scripting is disabled, so this is what round-trips
    "comment" => "literal",    # per the algorithm; Nokogiri escapes to &gt; instead
  }
  drift = pinned.reject { |(what, mode, _)| expected[what] == mode }
  if drift.empty?
    puts "  -> as pinned"
  else
    puts "  *** changed: #{drift.map { |(w, m, _)| "#{w} is now #{m}" }.join(', ')}"
    stats[:drift] = drift.length
  end
end

exit(stats[:not_fixed] + stats[:tree_differ] + stats[:error] +
     stats[:drift].to_i + stats[:no_teeth].to_i > 0 ? 1 : 0)
