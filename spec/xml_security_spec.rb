# frozen_string_literal: true

require "spec_helper"
require "tempfile"

# Security invariants of the native XML reader (plan §10). The parser is
# fail-closed by construction: it does no I/O, defines no entities (there is no
# DTD support), and bounds every resource with a per-document budget. These
# specs pin those guarantees from Ruby. Structural details that need to read the
# built tree are asserted in C (Makiri.__c_selftest, also run under ASan+UBSan);
# here we exercise the externally observable contract: hostile input raises a
# Makiri::Error and never crashes, hangs, leaks a foreign exception, or returns
# a partial document.
RSpec.describe "Makiri::XML security" do
  def parse(src)
    Makiri::XML(src)
  end

  describe "XXE / external entities are structurally impossible (no DTD)" do
    it "fails closed on DOCTYPE (the DTD that XXE needs is unsupported)" do
      expect { parse('<?xml version="1.0"?><!DOCTYPE r SYSTEM "x.dtd"><r/>') }
        .to raise_error(Makiri::XML::SyntaxError)
      expect { parse('<!DOCTYPE r PUBLIC "-//X//Y" "http://example.invalid/x.dtd"><r/>') }
        .to raise_error(Makiri::XML::SyntaxError)
      expect { parse('<!DOCTYPE r [ <!ENTITY xxe SYSTEM "file:///etc/passwd"> ]><r>&xxe;</r>') }
        .to raise_error(Makiri::XML::SyntaxError)
    end

    it "performs zero I/O: behaviour does not depend on the referenced file" do
      # A real, readable sentinel file. If the parser ever resolved a SYSTEM
      # identifier or an external entity, its result would differ from the
      # missing-file case. It does not: both raise the same SyntaxError, because
      # DOCTYPE is rejected outright before any resolution could occur.
      Tempfile.create(["xxe", ".txt"]) do |f|
        f.write("TOP-SECRET-SENTINEL")
        f.flush
        present = %(<!DOCTYPE r SYSTEM "file://#{f.path}"><r/>)
        missing = %(<!DOCTYPE r SYSTEM "file:///no/such/path/xyzzy.dtd"><r/>)
        expect { parse(present) }.to raise_error(Makiri::XML::SyntaxError)
        expect { parse(missing) }.to raise_error(Makiri::XML::SyntaxError)
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

    it "bounds distinct namespace declarations" do
      decls = (1..5000).map { |n| "xmlns:p#{n}='urn:#{n}'" }.join(" ")
      expect { parse("<a #{decls}/>") }.to raise_error(Makiri::XML::LimitExceeded)
    end
  end

  describe "strict decoding never repairs (no U+FFFD substitution)" do
    it "raises a Makiri error on invalid UTF-8 rather than replacing or leaking EncodingError" do
      # A Makiri::Error (here the strict-decode SyntaxError) rather than a bare
      # Ruby EncodingError or a silent success with U+FFFD-repaired content —
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
