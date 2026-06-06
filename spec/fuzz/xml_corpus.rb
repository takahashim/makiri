# frozen_string_literal: true
#
# XML parser fuzz corpus + mutator (§10 security finishing).
#
# Unlike the XPath/CSS fuzzer (which throws queries at a parsed document), the
# XML target fuzzes the *parser* itself: it feeds well-formed and deliberately
# hostile XML byte streams to Makiri::XML and asserts the parser never does
# anything worse than raise a Makiri::Error - no segfault, no hang, and no
# non-Makiri Ruby exception leaking (a strict-decode / contract violation).
#
# The seeds cover the whole grammar (elements, attributes, namespaces, CDATA,
# comments, PIs, the XML declaration, entities) plus the security-relevant
# adversarial shapes: XXE/DOCTYPE, billion-laughs, undefined entities, deep
# nesting, attribute floods, unterminated constructs, raw control bytes and
# invalid UTF-8. Mutation then stirs the corpus into mostly-malformed input.

module XmlFuzz
  module_function

  # Well-formed seeds - the parser must accept these (after mutation, usually
  # not). They double as the "valid" anchors the mutator edits away from.
  WELL_FORMED = [
    "<a/>",
    "<a></a>",
    "<a><b/><c></c></a>",
    %(<a x="1" y='two' z="a b"/>),
    "<a>hello <b>x</b> world</a>",
    "  \n <root/> \n ",
    "<a>1&lt;2&gt;3&amp;4&apos;5&quot;6</a>",
    "<a>&#65;&#x42;&#x1F600;</a>",
    "<atom:feed xmlns:atom='http://www.w3.org/2005/Atom'><atom:entry/></atom:feed>",
    "<a xmlns='urn:d'><b xmlns=''><c/></b></a>",
    "<a xml:lang='ja'/>",
    "<r><!-- a comment --><![CDATA[ raw < & > ]]><?pi data?></r>",
    "<?xml version='1.0' encoding='UTF-8'?><?xml-stylesheet href='s.xsl'?><r/>",
    "<café><日本語 値='1'/></café>",
    "<r>\r\n  <c a=\"p\r\nq\"/>\r\n</r>",
  ].freeze

  # Hostile / malformed seeds - every one of these must be rejected with a
  # Makiri::Error (SyntaxError or LimitExceeded), never a crash or a foreign
  # exception. These encode the §10 security invariants directly.
  HOSTILE = [
    # XXE: a DTD is unsupported, so DOCTYPE itself is fail-closed (no I/O).
    %(<?xml version="1.0"?><!DOCTYPE r SYSTEM "file:///etc/passwd"><r/>),
    %(<!DOCTYPE r [ <!ENTITY xxe SYSTEM "file:///etc/passwd"> ]><r>&xxe;</r>),
    %(<!DOCTYPE r PUBLIC "-//X//Y" "http://example.invalid/x.dtd"><r/>),
    # billion-laughs: needs entity definitions, which require a DTD -> rejected.
    %(<!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;">]><lolz>&lol2;</lolz>),
    # undefined entities (no DTD = no custom entity can ever be defined).
    "<a>&nbsp;</a>", "<a>&copy;</a>", "<r>&undefined;</r>",
    # malformed structure.
    "<a>", "<a></b>", "<a/><b/>", "junk<a/>", "</a>", "<>",
    %(<a x=>), %(<a x="<"/>), %(<a x="1>),
    # malformed §9 constructs.
    "<r><!-- a--b --></r>", "<r><!-- unterminated </r>", "<r><![CDATA[unterminated</r>",
    "<r><?pi unterminated</r>", " <?xml version='1.0'?><r/>", "<?xml?><r/>",
    "<![CDATA[outside]]><r/>",
    # strict names / namespaces / duplicate attributes.
    "<1bad/>", "<a b@d='1'/>", "<a:1b xmlns:a='u'/>", "<a:b/>",
    "<a x='1' x='2'/>", "<e xmlns:a='u' xmlns:b='u' a:x='1' b:x='2'/>",
    # invalid characters.
    "<a>\x01</a>", "<a>&#0;</a>", "<a>&#xB;</a>", "<a>&#xD800;</a>", "<a>&#x110000;</a>",
    "<a>&#999999999999;</a>",
    # budget pressure (small here; the mutator amplifies).
    "<a>#{'<b>' * 64}#{'</b>' * 64}</a>",
    "<e #{(1..64).map { |n| "a#{n}='1'" }.join(' ')}/>",
  ].freeze

  ALL_SEEDS = (WELL_FORMED + HOSTILE).freeze

  # Bytes that are "interesting" to splice into XML: structural metacharacters,
  # whitespace/newlines, a NUL, high control bytes, and lone invalid UTF-8
  # continuation/lead bytes (to exercise strict decode and the tokenizer edges).
  POISON = [
    "<", ">", "&", ";", "/", "?", "!", "=", '"', "'", "[", "]", "-",
    " ", "\t", "\n", "\r", "\0",
    "\x01", "\x1f", "\x7f",
    "\xC0", "\xC0\xAF", "\xE0\x80", "\xED\xA0\x80", "\xF5\x80\x80\x80", "\xFF",
    "&#x", "]]>", "-->", "?>", "<![CDATA[", "<!--", "<!DOCTYPE", "<?xml",
  ].map(&:b).freeze

  # One fuzz input: ~70% a mutated seed, ~30% a raw seed (so valid documents
  # keep flowing through too). Returns an ASCII-8BIT string that may be invalid
  # UTF-8 on purpose - the parser must still fail closed.
  def next_input(rng)
    seed = ALL_SEEDS.sample(random: rng)
    rng.rand(100) < 70 ? mutate(seed, rng) : seed.b
  end

  # Byte-level mutation biased toward XML-significant edits: splice poison
  # tokens, delete/duplicate ranges, and occasionally deep-nest or attr-flood to
  # probe the depth / per-element attribute budgets.
  def mutate(s, rng)
    out = s.b.dup
    case rng.rand(20)
    when 0 then return "<a>#{'<b>' * rng.rand(2_000..6_000)}".b   # depth blow-up (unbalanced)
    when 1 then return "<e #{(1..rng.rand(5_000..9_000)).map { |n| "a#{n}='1'" }.join(' ')}/>".b
    when 2 then return ("<n>" * rng.rand(1..40) + "x" + "</n>" * rng.rand(1..40)).b
    end

    edits = 1 + rng.rand(4)
    edits.times do
      next if out.empty?
      case rng.rand(6)
      when 0 then out[rng.rand(out.bytesize)] = ""                                   # delete
      when 1 then out.insert(rng.rand(out.bytesize + 1), POISON.sample(random: rng)) # splice poison
      when 2 then out[rng.rand(out.bytesize)] = POISON.sample(random: rng)           # overwrite
      when 3
        i = rng.rand(out.bytesize)
        out.insert(i, out[i, rng.rand(1..6)] || "")                                  # duplicate range
      when 4 then out.insert(rng.rand(out.bytesize + 1), rng.rand(0x20..0x7e).chr)   # random ASCII
      when 5 then out << POISON.sample(random: rng)                                  # append poison
      end
    end
    out
  end
end
