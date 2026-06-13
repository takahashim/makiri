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

  describe "source: a String or any #read-able object (IO / File / StringIO)" do
    require "stringio"
    require "tempfile"

    it "reads a StringIO / IO via #read" do
      expect(Makiri::XML(StringIO.new("<r>io</r>")).root.text).to eq("io")
    end

    it "reads a File (and Makiri::XML::Document.parse accepts one too)" do
      Tempfile.create(["t", ".xml"]) do |f|
        f.write("<r>file</r>")
        f.flush
        expect(Makiri::XML(File.open(f.path)).root.text).to eq("file")
        expect(Makiri::XML::Document.parse(File.open(f.path)).root.text).to eq("file")
      end
    end

    it "autodetects the encoding of a file read in binary mode (BOM)" do
      Tempfile.create(["u", ".xml"]) do |f|
        f.binmode
        f.write("\xFE\xFF".b + "<r>x</r>".encode("UTF-16BE").b) # UTF-16BE BOM
        f.flush
        expect(Makiri::XML(File.open(f.path, "rb")).root.text).to eq("x")
        expect(Makiri::XML(File.binread(f.path)).root.text).to eq("x")
      end
    end

    it "still honours keyword options (max_bytes) with an IO source" do
      io = StringIO.new("<r>#{'<a/>' * 100}</r>")
      expect { Makiri::XML(io, max_bytes: 50) }.to raise_error(Makiri::XML::LimitExceeded)
    end
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
      "literal ]]> in content"    => "<a>foo]]>bar</a>", # §2.4
      "literal ]]> alone"         => "<a>]]></a>",
      "missing S between attrs"    => %(<a x="1"y="2"/>), # §3.1
      "missing S, end tag form"    => %(<a x="1"y="2"></a>),
    }.each do |label, src|
      it(label) { expect { Makiri::XML(src) }.to raise_error(Makiri::XML::SyntaxError) }
    end

    it "still accepts a lone ] or ]] in content (only ]]> is forbidden)" do
      expect(Makiri::XML("<a>1]2]]3</a>").root.text).to eq("1]2]]3")
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

    it "requires a lowercase 'x' for a hex character reference (§4.1)" do
      expect(ok?("<a>&#x58;</a>")).to be true                              # lowercase x ok
      expect { Makiri::XML("<a>&#X58;</a>") }.to raise_error(Makiri::XML::SyntaxError) # uppercase X
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
      expect(ok?("<?xml version='1.0' encoding='UTF-8' standalone='yes'?><r/>")).to be true
      expect(ok?("<?xml version='1.0'?><?xml-stylesheet href='s.xsl'?><r/>")).to be true
    end

    it "validates the XML declaration pseudo-attribute grammar (§2.8)" do
      # version is required and first; encoding then standalone, lowercase,
      # in order, each at most once, S-separated, quoted, with valid values.
      [
        "<?xml VERSION='1.0'?><r/>",                       # keyword case
        "<?xml version='1.0' STANDALONE='yes'?><r/>",      # keyword case
        "<?xml version='1.0' standalone='YES'?><r/>",      # value case
        "<?xml encoding='UTF-8'?><r/>",                    # version required
        "<?xml encoding='UTF-8' version='1.0'?><r/>",      # version must be first
        "<?xml version='1.0'encoding='UTF-8'?><r/>",       # missing S separator
        "<?xml version='1.0' version='1.0'?><r/>",         # duplicate
        "<?xml version='1.0' standalone='yes' encoding='UTF-8'?><r/>", # order
        "<?xml version='1.0' valid='no'?><r/>",            # unknown pseudo-attr
        %(<?xml version="1.0' ?><r/>),                     # mismatched quotes
        "<?xml version='1.0^'?><r/>",                      # bad VersionNum char
        "<?xml version='1.0' standalone=yes?><r/>",        # unquoted value
      ].each { |src| expect { Makiri::XML(src) }.to raise_error(Makiri::XML::SyntaxError), src.inspect }
    end

    it "accepts only XML 1.0, rejecting other versions with a clear message" do
      expect(ok?("<?xml version='1.0'?><r/>")).to be true
      expect(ok?("<r/>")).to be true # no declaration -> XML 1.0 by default
      # well-formed but a version Makiri does not implement -> fail closed, not silently 1.0
      expect { Makiri::XML("<?xml version='1.1'?><r/>") }
        .to raise_error(Makiri::XML::SyntaxError, /unsupported XML version/)
      expect { Makiri::XML("<?xml version='1.5'?><r/>") }
        .to raise_error(Makiri::XML::SyntaxError, /unsupported XML version/)
      # "2.0" is not even a valid VersionNum ('1.' digits) -> a plain syntax error
      expect { Makiri::XML("<?xml version='2.0'?><r/>") }.to raise_error(Makiri::XML::SyntaxError)
    end

    it "reserves the PI target 'xml' (any case) but allows a Name target with a colon" do
      expect { Makiri::XML("<?XML version='1.0'?><r/>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<?xmL version='1.0'?><r/>") }.to raise_error(Makiri::XML::SyntaxError)
      # §2.6: PITarget is a Name, not an NCName, so a colon is well-formed.
      expect(ok?("<r><?a:b data?></r>")).to be true
      pi = Makiri::XML("<r><?a:b data?></r>").at_xpath("//processing-instruction()")
      expect(pi.name).to eq("a:b")
      expect(ok?("<r><?xml-stylesheet href='s'?></r>")).to be true # not the reserved 3-letter name
    end

    it "recognizes DOCTYPE but does not process the DTD (XXE structurally impossible)" do
      # A well-formed DOCTYPE parses; the DTD is not processed (no entities, no
      # external-subset I/O). See xml_doctype_spec / xml_security_spec for detail.
      expect(ok?("<!DOCTYPE r><r/>")).to be true
      expect(ok?("<!DOCTYPE r SYSTEM 'x.dtd'><r/>")).to be true
      expect(Makiri::XML("<!DOCTYPE r SYSTEM 'x.dtd'><r/>").internal_subset.name).to eq("r")
      # an entity the DTD would define stays undefined -> referencing it is fatal
      expect { Makiri::XML("<!DOCTYPE r [ <!ENTITY x 'y'> ]><r>&x;</r>") }
        .to raise_error(Makiri::XML::SyntaxError)
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

  describe "strict names (§9.2b)" do
    it "accepts XML 1.0 names including Unicode NameChar" do
      expect(ok?("<café/>")).to be true
      expect(ok?("<a-1.b/>")).to be true              # '-' '.' digits are NameChar
      expect(ok?("<_x/>")).to be true
      expect(ok?("<日本語 値='1'/>")).to be true
    end

    it "rejects names that violate NameStartChar / NCName" do
      expect { Makiri::XML("<1bad/>") }.to raise_error(Makiri::XML::SyntaxError)        # digit start
      expect { Makiri::XML("<-bad/>") }.to raise_error(Makiri::XML::SyntaxError)        # '-' start
      expect { Makiri::XML("<.bad/>") }.to raise_error(Makiri::XML::SyntaxError)        # '.' start
      expect { Makiri::XML("<a:1b xmlns:a='u'/>") }.to raise_error(Makiri::XML::SyntaxError) # local starts with digit
      expect { Makiri::XML("<a b@d='1'/>") }.to raise_error(Makiri::XML::SyntaxError)   # '@' not a NameChar
    end
  end

  describe "duplicate attributes (§9.3)" do
    it "rejects two attributes with the same resolved (namespace, local name)" do
      expect { Makiri::XML("<a x='1' x='2'/>") }.to raise_error(Makiri::XML::SyntaxError)
      same_uri = "<e xmlns:a='u' xmlns:b='u' a:x='1' b:x='2'/>"
      expect { Makiri::XML(same_uri) }.to raise_error(Makiri::XML::SyntaxError) # a:x and b:x collapse to {u}x
      expect { Makiri::XML("<a xmlns='d' xmlns='d'/>") }.to raise_error(Makiri::XML::SyntaxError) # default ns twice
    end

    it "allows attributes that differ in namespace or local name" do
      expect(ok?("<e xmlns:a='u1' xmlns:b='u2' a:x='1' b:x='2'/>")).to be true  # different URIs
      expect(ok?("<e xmlns:a='u' x='1' a:x='2'/>")).to be true                  # no-ns vs {u}
      expect(ok?("<e xmlns:a='u1' xmlns:b='u2'/>")).to be true                  # two xmlns decls
    end
  end

  describe "line-ending normalization (§9.3b-A)" do
    it "accepts CRLF and lone CR (folded to LF before tokenizing)" do
      expect(ok?("<a>x\r\ny</a>")).to be true
      expect(ok?("<a>x\ry</a>")).to be true
      expect(ok?("<a x=\"p\r\nq\"/>")).to be true
      expect(ok?("<r>\r\n  <c/>\r\n</r>")).to be true
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

  describe "prolog/epilog comments and PIs are document-node children (like Nokogiri)" do
    let(:doc) do
      Makiri::XML(<<~XML)
        <?xml version="1.0"?>
        <?stylesheet href="a.xsl"?>
        <!-- top -->
        <root><c/></root>
        <!-- tail -->
        <?after x?>
      XML
    end

    it "exposes them to XPath in document order" do
      expect(doc.xpath("//comment()").map(&:text)).to eq([" top ", " tail "])
      expect(doc.xpath("//processing-instruction()").map(&:name)).to eq(%w[stylesheet after])
      expect(doc.xpath("count(/node())")).to eq(5.0) # PI, comment, root, comment, PI
    end

    it "makes them children of the document node, around the root, no whitespace text" do
      kinds = doc.children.map { |n| n.class.name.split("::").last }
      expect(kinds).to eq(%w[ProcessingInstruction Comment Element Comment ProcessingInstruction])
      expect(doc.at_xpath("//comment()").parent).to be_a(Makiri::XML::Document)
      expect(doc.root.name).to eq("root") # the root element is still #root
    end

    it "still rejects the XML declaration anywhere but the very start" do
      expect { Makiri::XML("<!-- c --><?xml version='1.0'?><r/>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<r/><?xml version='1.0'?>") }.to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<![CDATA[x]]><r/>") }.to raise_error(Makiri::XML::SyntaxError) # char data in prolog
    end
  end

  describe "character encoding: BOM + declaration autodetection (XML 1.0 App. F)" do
    it "strips a leading UTF-8 BOM (it is the encoding signature, not content)" do
      expect(Makiri::XML("﻿<r>x</r>").root.text).to eq("x")        # UTF-8 string
      expect(Makiri::XML("\xEF\xBB\xBF<r>x</r>".b).root.text).to eq("x") # raw bytes
      # a U+FEFF that is NOT leading stays as ordinary content
      expect(Makiri::XML("<r>a﻿b</r>").root.text).to eq("a﻿b")
    end

    it "decodes a tagged non-UTF-8 String to the right characters" do
      expect(Makiri::XML("<r>日</r>".encode("Shift_JIS")).root.text).to eq("日")
      expect(Makiri::XML("<r>x</r>".encode("UTF-16LE")).root.text).to eq("x")
      expect(Makiri::XML("<r>x</r>".encode("UTF-16BE")).root.text).to eq("x")
    end

    it "autodetects raw bytes (ASCII-8BIT) from the BOM" do
      bytes = "\xFE\xFF".b + "<r>x</r>".encode("UTF-16BE").b # UTF-16BE BOM + content
      expect(Makiri::XML(bytes).root.text).to eq("x")
    end

    it "autodetects raw bytes from the encoding declaration (no BOM)" do
      sjis = ("<?xml version='1.0' encoding='Shift_JIS'?><r>".encode("Shift_JIS") +
              "日".encode("Shift_JIS") + "</r>".encode("Shift_JIS")).b
      expect(Makiri::XML(sjis).root.text).to eq("日")
    end

    it "rejects a BOM that contradicts the declaration or the String encoding" do
      # UTF-8 BOM but the declaration claims a different encoding (cf. xmlconf ht-bh)
      expect { Makiri::XML("\xEF\xBB\xBF<?xml version='1.0' encoding='iso-8859-1'?><x/>".b) }
        .to raise_error(Makiri::XML::SyntaxError)
      # UTF-16 BOM but the declaration claims utf-8
      u16 = "\xFE\xFF".b + "<?xml version='1.0' encoding='utf-8'?><x/>".encode("UTF-16BE").b
      expect { Makiri::XML(u16) }.to raise_error(Makiri::XML::SyntaxError)
      # a concrete (UTF-8) String tag with a UTF-16 BOM in the bytes
      expect { Makiri::XML("\xFF\xFE<r/>".dup.force_encoding("UTF-8")) }
        .to raise_error(Makiri::XML::SyntaxError)
    end

    it "rejects a declaration that disagrees with a concrete String encoding" do
      # Shift_JIS String but the declaration claims UTF-8 -> self-inconsistent.
      expect { Makiri::XML("<?xml version='1.0' encoding='UTF-8'?><r>x</r>".encode("Shift_JIS")) }
        .to raise_error(Makiri::XML::SyntaxError)
      expect { Makiri::XML("<?xml version='1.0' encoding='iso-8859-1'?><r>x</r>") } # UTF-8 String
        .to raise_error(Makiri::XML::SyntaxError)
      # but a declaration that agrees (or is an ASCII subset) is fine
      expect(ok?("<?xml version='1.0' encoding='Shift_JIS'?><r>x</r>".encode("Shift_JIS"))).to be true
      expect(ok?("<?xml version='1.0' encoding='utf-8'?><r>x</r>")).to be true
      expect(ok?("<?xml version='1.0' encoding='Shift_JIS'?><r>x</r>".encode("US-ASCII"))).to be true
    end
  end
end
