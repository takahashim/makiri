# frozen_string_literal: true
#
# The text-input contract holds at every boundary.
#
#   ruby -Ilib spec/invariants/check_text_input.rb
#
# ## Why
#
# CLAUDE.md's "Text-input contract" states a precise set of rules that nothing
# checked mechanically. Getting an encoding wrong is also a classic way to
# inject: Makiri honours the input String's encoding, transcoding anything that
# is not UTF-8 / US-ASCII / ASCII-8BIT before the parser ever sees it, and that
# "convert first" decision is what determines how multibyte encodings behave.
#
# Two directions are dangerous, and only one of them is Makiri's to control:
#
#   (1) a syntax character (< > " ' &) that was NOT in the input APPEARS after
#       conversion - markup nobody wrote, an injection;
#   (2) a syntax character that WAS in the input DISAPPEARS - a multibyte lead
#       byte swallowing the ASCII after it. Shift_JIS 0x81 0x3C is one
#       character, not `<`. That is the encoding's definition, and NOT
#       swallowing it would be the dangerous behaviour.
#
# E3 checks (1) by parsing with Makiri and looking at the tree. (2) is pinned by
# E4, which compares against transcoding in Ruby first. A real-world injection
# happens when Makiri and some other component disagree about the encoding, and
# no library can close that alone; what Makiri can promise is that it reads the
# encoding it was handed, which is E4.
#
#   E1 encoding honoured  Shift_JIS / EUC-JP / Windows-31J / UTF-16 are read
#   E2 lenient decoding   invalid UTF-8 never fails the parse; the DOM is valid
#   E3 no manufactured    conversion invents no syntax character (all 2-byte
#                         sequences, through Makiri)
#   E4 equivalence        raw bytes == transcoding in Ruby first
#   E5 strict APIs        invalid UTF-8 raises at XPath / CSS / attributes / content=
#   E6 the NUL two-tier   accepted in HTML data content, refused in names and
#                         engine input; Makiri::XML refuses it everywhere

require "makiri"

SYNTAX = %w[< > " ' &].freeze
MB_ENCODINGS = %w[Shift_JIS EUC-JP Windows-31J].freeze
INVALID_UTF8 = "\xC3".b.force_encoding("UTF-8")
NUL = "a\x00b"

$fail = 0

def row(label, ok, extra = "")
  $fail += 1 unless ok
  puts format("  %-44s %s %s", label, ok ? "OK" : "NG", extra)
end

def section(title)
  puts
  puts "=" * 72
  puts title
end

# --- E1 ---------------------------------------------------------------

section "E1 the input String's encoding is honoured"
SAMPLE = "日本語テキスト"
(MB_ENCODINGS + %w[UTF-16BE UTF-16LE]).each do |enc|
  src = "<html><body><p>#{SAMPLE}</p></body></html>"
  bytes = begin
    src.encode(enc)
  rescue StandardError => e
    next row(enc, false, e.class.to_s)
  end
  got = Makiri::HTML(bytes).at_css("p")&.text
  row(enc, got == SAMPLE, got.inspect[0, 26])
end
plain = "<html><body><p>abc</p></body></html>"
{ "ASCII-8BIT" => plain.dup.force_encoding("ASCII-8BIT"),
  "US-ASCII" => plain.dup.force_encoding("US-ASCII"),
  "UTF-8" => plain }.each do |label, s|
  row("#{label} (passed through)", Makiri::HTML(s).at_css("p")&.text == "abc")
end

# --- E2 ---------------------------------------------------------------

section "E2 invalid UTF-8 never fails the parse; the DOM stays valid UTF-8"
{
  "a truncated 2-byte sequence" => "\xC3".b,
  "a truncated 3-byte sequence" => "\xE3\x81".b,
  "not UTF-8 at all"            => "\xFF\xFE".b,
  "a surrogate"                 => "\xED\xA0\x80".b,
  "an overlong NUL"             => "\xC0\x80".b,
  "out of range"                => "\xF4\x90\x80\x80".b,
  "a stray continuation byte"   => "\x80\x80".b,
}.each do |label, bytes|
  s = ("<html><body><p>A#{bytes}B</p></body></html>").force_encoding("UTF-8")
  begin
    t = Makiri::HTML(s).at_css("p")&.text
    ok = !t.nil? && t.valid_encoding? && t.encoding == Encoding::UTF_8 &&
         t.start_with?("A") && t.end_with?("B")
    row(label, ok, t.inspect[0, 24])
  rescue StandardError => e
    row(label, false, "#{e.class}: #{e.message[0, 40]}")
  end
end

# --- E3 ---------------------------------------------------------------

section "E3 conversion invents no syntax character (every 2-byte sequence)"

TEMPLATE_HEAD = "<html><body><p id=\"q\">A".b
TEMPLATE_TAIL = "B</p></body></html>".b

# Returns why the document was disturbed, or nil.
#
# The judgement is made over the whole <body>: looking only inside <p> misses
# markup that CLOSES it and lands outside (the negative control below found
# exactly that hole). The text CONTENT is not judged - a lead byte swallowing
# the following ASCII is correct decoding (Shift_JIS 0x81 0x42 is a character,
# not `B`), and that is direction (2), not an injection. An injection shows up
# in the structure.
def injection_reason(raw)
  body = Makiri::HTML(raw).at_css("body")
  return "no body" if body.nil?

  kids = body.children.to_a
  return "body has #{kids.length} children (only <p> expected)" if kids.length != 1

  el = kids.first
  return "body's child is #{el.name}" unless el.node_type == 1 && el.name == "p"

  attrs = el.attribute_nodes.map(&:name)
  return "attributes are #{attrs.inspect}" if attrs != ["id"]

  ekids = el.children.to_a
  return "p's children are #{ekids.map(&:node_type).inspect}" if ekids.any? { |c| c.node_type != 3 }

  nil
rescue StandardError => e
  "#{e.class}: #{e.message[0, 40]}"
end

MB_ENCODINGS.each do |enc|
  made = []
  checked = 0
  256.times do |a|
    next if SYNTAX.any? { |ch| ch.ord == a }

    256.times do |b|
      next if SYNTAX.any? { |ch| ch.ord == b }

      checked += 1
      raw = (TEMPLATE_HEAD + [a, b].pack("C*") + TEMPLATE_TAIL).force_encoding(enc)
      why = injection_reason(raw)
      made << [a, b, why] if why
    end
  end
  row("#{enc} (#{checked} sequences)", made.empty?,
      made.empty? ? "" : "#{made.length}: #{made.first(3).inspect[0, 80]}")
end

# The detector has to be shown catching something, or a green E3 says nothing.
section "E3 negative control - does the detector see an injection?"

{
  "a tag"                  => "<script>evil()</script>",
  "closing out of the tag" => "</p><div>x",
  "an attribute"           => %(<b onerror="evil()">),
  "nothing (control)"      => "plain text",
}.each do |label, payload|
  raw = (TEMPLATE_HEAD + payload.b + TEMPLATE_TAIL).force_encoding("ASCII-8BIT")
  why = injection_reason(raw)
  want = label != "nothing (control)"
  row("  #{label}", why.nil? != want, why || "not detected")
end

# --- E4 ---------------------------------------------------------------

section "E4 raw bytes == transcoding in Ruby first"
MB_ENCODINGS.each do |enc|
  mismatch = []
  checked = 0
  256.times do |b|
    raw = "<html><body><p>A#{b.chr}B</p></body></html>".b.force_encoding(enc)
    want = begin
      raw.encode("UTF-8", invalid: :replace, undef: :replace)
    rescue StandardError
      next
    end
    checked += 1
    got = Makiri::HTML(raw).at_css("p")&.text
    expect = Makiri::HTML(want).at_css("p")&.text
    mismatch << b if got != expect
  end
  row("#{enc} (#{checked} bytes)", mismatch.empty?,
      mismatch.empty? ? "" : "#{mismatch.length} differ: #{mismatch.first(6).inspect}")
end

# --- E5 ---------------------------------------------------------------

section "E5 the programmatic APIs refuse invalid UTF-8"
d = Makiri::HTML("<html><body><p id=x>t</p></body></html>")
p1 = d.at_css("p")
{
  "xpath(expr)"      => -> { d.xpath("//#{INVALID_UTF8}") },
  "css(selector)"    => -> { d.css(INVALID_UTF8) },
  "attribute name"   => -> { p1[INVALID_UTF8] = "v" },
  "attribute value"  => -> { p1["k"] = INVALID_UTF8 },
  "content="         => -> { p1.content = INVALID_UTF8 },
  "name="            => -> { p1.name = INVALID_UTF8 },
  "create_element"   => -> { d.create_element(INVALID_UTF8) },
  "create_text_node" => -> { d.create_text_node(INVALID_UTF8) },
  "create_comment"   => -> { d.create_comment(INVALID_UTF8) },
}.each do |label, blk|
  begin
    blk.call
    row(label, false, "no exception")
  rescue Makiri::Error, ArgumentError => e
    row(label, true, e.class.to_s)
  rescue StandardError => e
    row(label, false, "#{e.class}: #{e.message[0, 40]}")
  end
end

# --- E6 ---------------------------------------------------------------

section "E6 the NUL two-tier contract"

def expect_ok(label, expected)
  got = yield
  row(label, got == expected, got.inspect[0, 20])
rescue StandardError => e
  row(label, false, "#{e.class}: #{e.message[0, 40]}")
end

def expect_raise(label)
  yield
  row(label, false, "no exception")
rescue Makiri::Error, ArgumentError => e
  row(label, true, e.class.to_s)
rescue StandardError => e
  row(label, false, "#{e.class}: #{e.message[0, 40]}")
end

puts "  HTML data content accepts it"
expect_ok("  create_text_node", NUL) { d.create_text_node(NUL).content }
expect_ok("  create_comment", NUL)   { d.create_comment(NUL).content }
expect_ok("  content=", NUL)         { p1.content = NUL; p1.text }
expect_ok("  attribute value", NUL)  { p1["k"] = NUL; p1["k"] }

puts "  names and engine input refuse it"
expect_raise("  create_element") { d.create_element(NUL) }
expect_raise("  attribute name") { p1[NUL] = "v" }
expect_raise("  name=")          { p1.name = NUL }
expect_raise("  xpath")          { d.xpath("//#{NUL}") }
expect_raise("  css")            { d.css(NUL) }

puts "  Makiri::XML refuses it everywhere"
x = Makiri::XML("<r><a k='v'/></r>")
xa = x.root.children.first
expect_raise("  create_text_node") { x.create_text_node(NUL) }
expect_raise("  content=")         { xa.content = NUL }
expect_raise("  attribute value")  { xa["k"] = NUL }
expect_raise("  create_element")   { x.create_element(NUL) }

puts
puts "=" * 72
puts $fail.zero? ? "the contract holds" : "*** #{$fail} departures from the contract"
exit($fail.zero? ? 0 : 1)
