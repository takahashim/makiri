# frozen_string_literal: true

require "spec_helper"
require "tempfile"

# Security invariants of the native XML reader (plan §10). The parser is
# fail-closed by construction: it does no I/O and defines no entities (a DOCTYPE
# is recognized but its DTD is NOT processed - no entity/element declarations are
# loaded and no external subset is fetched), and it bounds every resource with a
# per-document budget. These specs pin those guarantees from Ruby. Structural
# details that need to read the built tree are asserted in C (Makiri.__c_selftest,
# also run under ASan+UBSan); here we exercise the externally observable
# contract: hostile input raises a Makiri::Error and never crashes, hangs, leaks
# a foreign exception, or returns a partial document.
RSpec.describe "Makiri::XML security" do
  def parse(src)
    Makiri::XML(src)
  end

  describe "XXE / external entities are structurally impossible (DTD not processed)" do
    it "recognizes a well-formed DOCTYPE but defines none of its entities" do
      # The DOCTYPE itself parses (its name + external id are retained for
      # Document#internal_subset), but nothing in the DTD is processed: a SYSTEM
      # or PUBLIC external subset is never fetched, and an entity an XXE attack
      # would declare in the internal subset stays undefined -> a reference to it
      # is a fatal undefined-entity error.
      expect(parse('<?xml version="1.0"?><!DOCTYPE r SYSTEM "x.dtd"><r/>'))
        .to be_a(Makiri::XML::Document)
      expect(parse('<!DOCTYPE r PUBLIC "-//X//Y" "http://example.invalid/x.dtd"><r/>'))
        .to be_a(Makiri::XML::Document)
      expect { parse('<!DOCTYPE r [ <!ENTITY xxe SYSTEM "file:///etc/passwd"> ]><r>&xxe;</r>') }
        .to raise_error(Makiri::XML::SyntaxError)
    end

    it "performs zero I/O: behaviour does not depend on the referenced file" do
      # A real, readable sentinel file. If the parser ever resolved a SYSTEM
      # identifier or an external entity, its result would differ from the
      # missing-file case. It does not: both parse identically (no fetch happens),
      # so the DOCTYPE's system id never reaches the filesystem.
      Tempfile.create(["xxe", ".txt"]) do |f|
        f.write("TOP-SECRET-SENTINEL")
        f.flush
        present = %(<!DOCTYPE r SYSTEM "file://#{f.path}"><r>ok</r>)
        missing = %(<!DOCTYPE r SYSTEM "file:///no/such/path/xyzzy.dtd"><r>ok</r>)
        a = parse(present)
        b = parse(missing)
        expect(a.root.text).to eq("ok")
        expect(b.root.text).to eq(a.root.text)
        # the sentinel is never read into the document
        expect(a.root.text).not_to include("SENTINEL")
        expect(a.internal_subset.system_id).to eq("file://#{f.path}")
      end
    end

    it "treats undefined entities as a fatal error (no entity can be defined)" do
      # With no DTD, &foo; can never be bound, so HTML entities and any custom
      # name are undefined -> SyntaxError. This is what makes XXE and
      # billion-laughs impossible rather than merely disabled.
      ["<r>&xxe;</r>", "<r>&nbsp;</r>", "<r>&copy;</r>", %(<a x="&mdash;"/>)].each do |src|
        expect { parse(src) }.to raise_error(Makiri::XML::SyntaxError)
      end
    end
  end

  describe "entity-expansion bombs cannot amplify" do
    it "rejects the classic billion-laughs payload (needs entity definitions)" do
      bomb = <<~XML
        <?xml version="1.0"?>
        <!DOCTYPE lolz [
          <!ENTITY lol "lol">
          <!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;">
          <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;">
        ]>
        <lolz>&lol3;</lolz>
      XML
      expect { parse(bomb) }.to raise_error(Makiri::XML::SyntaxError)
    end

    it "accepts only the 5 predefined entities and numeric refs (output <= input)" do
      # No amplification path exists: predefined entities and numeric references
      # each yield at most a handful of bytes and there is no recursion.
      expect(parse("<a>1&lt;2&gt;3&amp;4&apos;5&quot;6 &#65;&#x42;</a>"))
        .to be_a(Makiri::XML::Document)
    end
  end

  describe "resource budgets are fail-closed (§4)" do
    it "bounds element nesting depth" do
      deep = ("<a>" * 5000) + ("</a>" * 5000)
      expect { parse(deep) }.to raise_error(Makiri::XML::LimitExceeded)
    end

    it "bounds attributes per element" do
      flood = "<e " + (1..5000).map { |n| "a#{n}='1'" }.join(" ") + "/>"
      expect { parse(flood) }.to raise_error(Makiri::XML::LimitExceeded)
    end

    it "bounds namespace bindings IN SCOPE at once (the scope-stack depth)" do
      # MKR_XML_MAX_NS caps how many bindings are concurrently in scope, not the
      # document-wide total: 5000 xmlns on ONE element are all in scope together
      # and trip the limit.
      decls = (1..5000).map { |n| "xmlns:p#{n}='urn:#{n}'" }.join(" ")
      expect { parse("<a #{decls}/>") }.to raise_error(Makiri::XML::LimitExceeded)
    end

    it "does NOT count namespaces that go out of scope (popped on element close)" do
      # The same 5000 declarations spread across siblings each leave scope when
      # the element closes, so they never coexist -> no LimitExceeded. This pins
      # the in-scope semantics (a per-binding 'distinct per document' counter
      # would wrongly reject this), and the per-declaration memory is bounded by
      # the byte / attribute budgets instead.
      sibs = (1..5000).map { |n| "<c xmlns:p#{n}='urn:#{n}'/>" }.join
      doc = parse("<root>#{sibs}</root>")
      expect(doc.xpath("count(//c)")).to eq(5000.0)
    end

    it "bounds total arena memory (a tiny-element flood hits the byte budget)" do
      # Node structs are counted against the same arena byte budget as text, so a
      # flood of empty elements is bounded by memory, not only by the 10M node cap
      # (which at ~128 B/node would otherwise permit a >1 GB arena). This stays
      # well under the 256 MiB default; the assertion is only that a
      # pathological-but-bounded doc parses without unbounded growth.
      doc = parse("<r>#{'<a/>' * 200_000}</r>")
      expect(doc.xpath("count(//a)")).to eq(200_000.0)
    end
  end

  describe "per-parse budget override (max_bytes:)" do
    # ~1 MB+ of arena (markup-heavy), comfortably under the 256 MiB default.
    let(:big) { "<root>#{'<item><a>1</a><b>2</b></item>' * 20_000}</root>" }

    it "parses under the default and via Makiri::XML::Document.parse" do
      expect(Makiri::XML(big).xpath("count(//item)")).to eq(20_000.0)
      expect(Makiri::XML::Document.parse("<r/>", max_bytes: 4096).root.name).to eq("r")
    end

    it "rejects a document that exceeds a lowered max_bytes" do
      expect { Makiri::XML(big, max_bytes: 100_000) }
        .to raise_error(Makiri::XML::LimitExceeded)
    end

    it "parses the same document under a raised max_bytes" do
      expect(Makiri::XML(big, max_bytes: 64 * 1024 * 1024).xpath("count(//item)"))
        .to eq(20_000.0)
    end

    it "validates the option (positive Integer; unknown keywords rejected)" do
      expect { Makiri::XML("<r/>", max_bytes: 0) }.to raise_error(ArgumentError)
      expect { Makiri::XML("<r/>", max_bytes: -5) }.to raise_error(ArgumentError) # no unsigned wrap
      expect { Makiri::XML("<r/>", max_bytes: 1.5) }.to raise_error(TypeError)
      expect { Makiri::XML("<r/>", max_bytes: "big") }.to raise_error(TypeError)
      expect { Makiri::XML("<r/>", bogus: 1) }.to raise_error(ArgumentError)
    end
  end

  describe "strict decoding never repairs (no U+FFFD substitution)" do
    it "raises a Makiri error on invalid UTF-8 rather than replacing or leaking EncodingError" do
      # A Makiri::Error (here the strict-decode SyntaxError) rather than a bare
      # Ruby EncodingError or a silent success with U+FFFD-repaired content -
      # EncodingError is not a Makiri::Error, so this assertion excludes it.
      ["<a>\xFF\xFE</a>".b, "<a>\xC0\xAF</a>".b, "<a>\xED\xA0\x80</a>".b].each do |bytes|
        expect { parse(bytes) }.to raise_error(Makiri::Error)
      end
    end

    it "rejects an embedded NUL and other non-XML-Char control bytes" do
      expect { parse("<a>\x00</a>".b) }.to raise_error(Makiri::Error)
      expect { parse("<a>\x01</a>") }.to raise_error(Makiri::XML::SyntaxError)
    end
  end

  describe "fail-closed: an error never yields a partial document" do
    it "always raises (never returns nil or a half-built tree) on malformed input" do
      ["<a>", "<a></b>", "<a/><b/>", "<r><!-- a--b --></r>", "<a:b/>",
       "<a x='1' x='2'/>"].each do |src|
        expect { parse(src) }.to raise_error(Makiri::Error)
      end
    end
  end
end
