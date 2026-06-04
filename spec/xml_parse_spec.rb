# frozen_string_literal: true

require "spec_helper"

# Minimal XML tokenizer + tree builder (§14 steps 5-6): elements, attributes and
# character data, with fail-closed well-formedness errors. Namespaces, entities,
# comments/CDATA/PI/XML-decl/DOCTYPE arrive in later steps. Tree *structure* is
# asserted in C (Makiri.__c_selftest -> mkr_xml_parse_selftest, also run under
# the sanitizer); these specs pin the accept/reject contract from Ruby.
RSpec.describe "Makiri::XML minimal parse" do
  def ok?(src)
    Makiri::XML(src).is_a?(Makiri::XML::Document)
  end

  describe "accepts well-formed documents" do
    %w[
      <a/>
      <a></a>
      <a><b/><c></c></a>
    ].each do |src|
      it(src) { expect(ok?(src)).to be true }
    end

    it "attributes (single and double quoted)" do
      expect(ok?(%(<a x="1" y='two' z="a b"/>))).to be true
    end

    it "mixed character data and child elements" do
      expect(ok?("<a>hello <b>x</b> world</a>")).to be true
    end

    it "leading/trailing whitespace around the root (misc)" do
      expect(ok?("  \n <root/> \n ")).to be true
    end
  end

  describe "rejects malformed documents (fail-closed SyntaxError)" do
    {
      "unclosed element"          => "<a>",
      "mismatched end tag"        => "<a></b>",
      "multiple root elements"    => "<a/><b/>",
      "content before root"       => "junk<a/>",
      "content after root"        => "<a/>tail",
      "stray end tag"             => "</a>",
      "missing attribute value"   => "<a x=>",
      "unquoted attribute value"  => "<a x=1/>",
      "raw < in attribute value"  => %(<a x="<"/>),
      "unterminated attr value"   => %(<a x="1>),
      "unterminated start tag"    => "<a ",
      "empty element name"        => "<>",
    }.each do |label, src|
      it(label) { expect { Makiri::XML(src) }.to raise_error(Makiri::XML::SyntaxError) }
    end

  end

  it "the depth budget is fail-closed (deeply nested input does not crash)" do
    deep = "<a>" * 5000 + "</a>" * 5000
    expect { Makiri::XML(deep) }.to raise_error(Makiri::XML::LimitExceeded)
  end

  describe "character data: entities + XML Char (§9.1/§9.2)" do
    it "expands the 5 predefined entities and numeric references (in text and attrs)" do
      # structural correctness is asserted in C (mkr_xml_parse_selftest); here we
      # just confirm well-formed entity usage is accepted.
      expect(ok?("<a>1&lt;2&gt;3&amp;4&apos;5&quot;6</a>")).to be true
      expect(ok?("<a>&#65;&#x42;&#x1F600;</a>")).to be true
      expect(ok?(%(<a x="p&amp;q" y="&#65;"/>))).to be true
    end

    it "rejects undefined entities (no DTD -> billion-laughs impossible)" do
      expect { Makiri::XML("<a>&nbsp;</a>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<a>&copy;</a>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML(%(<a x="&mdash;"/>)) }.to raise_error(Makiri::XML::SyntaxError)
    end

    it "rejects a bare ampersand and malformed references" do
      ["<a>x & y</a>", "<a>&;</a>", "<a>&#;</a>", "<a>&lt</a>", "<a>&#xZ;</a>"].each do |src|
        expect { Makiri::XML(src) }.to raise_error(Makiri::XML::SyntaxError)
      end
    end

    it "rejects references and literals outside the XML Char set" do
      expect { Makiri::XML("<a>&#0;</a>") }.to raise_error(Makiri::XML::SyntaxError)      # NUL
      expect { Makiri::XML("<a>&#xB;</a>") }.to raise_error(Makiri::XML::SyntaxError)     # vertical tab
      expect { Makiri::XML("<a>&#xD800;</a>") }.to raise_error(Makiri::XML::SyntaxError)  # surrogate
      expect { Makiri::XML("<a>&#x110000;</a>") }.to raise_error(Makiri::XML::SyntaxError) # > U+10FFFF
      expect { Makiri::XML("<a>\x01</a>") }.to raise_error(Makiri::XML::SyntaxError)      # literal control char
    end

    it "accepts the maximum valid codepoint U+10FFFF" do
      expect(ok?("<a>&#x10FFFF;</a>")).to be true
    end

    it "rejects a numeric reference that would overflow (no integer wrap -> false accept)" do
      # On a naive `cp = cp*base + digit; if (cp > 0x10FFFF)` this wraps uint32 and
      # could land back inside the valid range; the pre-multiply guard must catch it.
      ["<a>&#9999999999;</a>", "<a>&#x100000000;</a>",
       "<a>&#99999999999999999999;</a>"].each do |src|
        expect { Makiri::XML(src) }.to raise_error(Makiri::XML::SyntaxError)
      end
    end
  end

  describe "namespaces (§7)" do
    # Resolved (ns_uri, prefix) is asserted in C (mkr_xml_parse_selftest); these
    # pin the accept/reject contract.
    it "accepts default and prefixed namespace declarations" do
      expect(ok?("<a xmlns='urn:d'><b/></a>")).to be true
      expect(ok?("<atom:feed xmlns:atom='http://www.w3.org/2005/Atom'><atom:entry/></atom:feed>")).to be true
      expect(ok?(%(<a xmlns:x='urn:x' x:attr='v' plain='w'/>))).to be true
      expect(ok?("<a xmlns:xml='http://www.w3.org/XML/1998/namespace'/>")).to be true # xml -> its own ns ok
      expect(ok?("<a xml:lang='ja'/>")).to be true # predefined xml prefix needs no declaration
      expect(ok?("<a xmlns='urn:d'><b xmlns=''><c/></b></a>")).to be true # undeclare default
    end

    it "rejects unbound prefixes" do
      expect { Makiri::XML("<a:b/>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<a><b:c/></a>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<a x:y='1'/>") }.to raise_error(Makiri::XML::SyntaxError)
    end

    it "enforces reserved-prefix rules (§7.1)" do
      expect { Makiri::XML("<a xmlns:xml='urn:wrong'/>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<a xmlns:xmlns='urn:x'/>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<a:b xmlns:a=''/>") }.to raise_error(Makiri::XML::SyntaxError) # empty URI
    end

    it "rejects malformed QNames" do
      ["<:a/>", "<a:/>", "<a:b:c/>", "<a ::='1'/>"].each do |src|
        expect { Makiri::XML(src) }.to raise_error(Makiri::XML::SyntaxError)
      end
    end

    it "the namespace count budget is fail-closed" do
      decls = (1..5000).map { |n| "xmlns:p#{n}='urn:#{n}'" }.join(" ")
      expect { Makiri::XML("<a #{decls}/>") }.to raise_error(Makiri::XML::LimitExceeded)
    end
  end

  describe "comments, CDATA, PIs and the XML declaration (§9)" do
    it "accepts comments, CDATA sections and processing instructions" do
      expect(ok?("<r><!-- a comment --></r>")).to be true
      expect(ok?("<r><![CDATA[ raw < & > text ]]></r>")).to be true
      expect(ok?("<r><?target some data?></r>")).to be true
      expect(ok?("<r><?target?></r>")).to be true                  # empty PI data
      expect(ok?("<!-- prolog --><r/><!-- epilog -->")).to be true # misc around the root
    end

    it "accepts the XML declaration and prolog processing instructions" do
      expect(ok?("<?xml version='1.0'?><r/>")).to be true
      expect(ok?("<?xml version='1.0' encoding='UTF-8'?><r/>")).to be true
      expect(ok?("<?xml version='1.0'?><?xml-stylesheet href='s.xsl'?><r/>")).to be true
    end

    it "rejects DOCTYPE (DTDs unsupported -> fail-closed, XXE structurally impossible)" do
      expect { Makiri::XML("<!DOCTYPE r><r/>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<!DOCTYPE r SYSTEM 'x.dtd'><r/>") }.to raise_error(Makiri::XML::SyntaxError)
    end

    it "rejects malformed comments (-- in content, unterminated, --->)" do
      expect { Makiri::XML("<r><!-- a--b --></r>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<r><!-- unterminated </r>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<r><!-- bad --->") }.to raise_error(Makiri::XML::SyntaxError)
    end

    it "rejects misplaced/unterminated XML declarations, PIs and CDATA" do
      expect { Makiri::XML(" <?xml version='1.0'?><r/>") }.to raise_error(Makiri::XML::SyntaxError) # not at start
      expect { Makiri::XML("<r/><?xml version='1.0'?>") }.to raise_error(Makiri::XML::SyntaxError)  # in epilog
      expect { Makiri::XML("<?xml?><r/>") }.to raise_error(Makiri::XML::SyntaxError)                # no version
      expect { Makiri::XML("<r><?pi unterminated</r>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<![CDATA[outside]]><r/>") }.to raise_error(Makiri::XML::SyntaxError)     # CDATA outside root
      expect { Makiri::XML("<r><![CDATA[unterminated</r>") }.to raise_error(Makiri::XML::SyntaxError)
    end
  end

  describe "budgets" do
    it "caps attributes PER ELEMENT (not document-wide)" do
      # > 4096 attributes on a single element is fail-closed...
      big = "<e " + (1..5000).map { |n| "a#{n}='1'" }.join(" ") + "/>"
      expect { Makiri::XML(big) }.to raise_error(Makiri::XML::LimitExceeded)

      # ...but a large *total* across many elements is fine (regression: the cap
      # must not be a document-wide attribute budget).
      many = "<r>" + ("<e a='1' b='2'/>" * 3000) + "</r>" # 6000 attributes total
      expect(ok?(many)).to be true
    end
  end
end
