# frozen_string_literal: true

require "spec_helper"

# DOCTYPE handling: the native XML reader RECOGNIZES a `<!DOCTYPE ...>` and
# retains its name + external identifiers (Document#internal_subset, a
# Makiri::XML::DocumentType; the Nokogiri name DTD is an alias), but it does NOT
# process the DTD - no
# entity/element declarations are loaded and no external subset is fetched. The
# security consequences (undefined entities, zero I/O) live in xml_security_spec.
RSpec.describe "Makiri::XML DOCTYPE / internal_subset" do
  def dtd(src)
    Makiri::XML(src).internal_subset
  end

  it "returns nil when the document has no DOCTYPE" do
    expect(Makiri::XML("<r>hi</r>").internal_subset).to be_nil
  end

  it "exposes a name-only DOCTYPE with nil external/system ids" do
    d = dtd("<!DOCTYPE html><html/>")
    expect(d).to be_a(Makiri::XML::DocumentType)
    expect(d.name).to eq("html")
    expect(d.external_id).to be_nil
    expect(d.public_id).to be_nil
    expect(d.system_id).to be_nil
  end

  it "exposes a SYSTEM external id" do
    d = dtd(%(<!DOCTYPE root SYSTEM "root.dtd"><root/>))
    expect(d.name).to eq("root")
    expect(d.external_id).to be_nil
    expect(d.system_id).to eq("root.dtd")
  end

  it "exposes a PUBLIC external id (public_id aliases external_id)" do
    d = dtd(%(<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" ) +
            %("http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"><html/>))
    expect(d.name).to eq("html")
    expect(d.external_id).to eq("-//W3C//DTD XHTML 1.0 Strict//EN")
    expect(d.public_id).to eq(d.external_id)
    expect(d.system_id).to eq("http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd")
  end

  it "distinguishes an absent id (nil) from an empty literal (\"\")" do
    d = dtd(%(<!DOCTYPE r PUBLIC "" ""><r/>))
    expect(d.public_id).to eq("")
    expect(d.system_id).to eq("")
  end

  it "skips an internal subset without processing it" do
    d = dtd(%(<!DOCTYPE r [ <!ELEMENT r (#PCDATA)> <!ATTLIST r a CDATA #IMPLIED> ]><r>x</r>))
    expect(d.name).to eq("r")
    # the internal subset is balanced (brackets / quoted '>') but not parsed
    expect(d.external_id).to be_nil
    expect(d.system_id).to be_nil
  end

  it "keeps the DOCTYPE off the tree (XPath 1.0 has no doctype node)" do
    doc = Makiri::XML(%(<!DOCTYPE r SYSTEM "r.dtd"><r><a/><b/></r>))
    expect(doc.xpath("//node()").map(&:name)).to eq(%w[r a b])
    expect(doc.xpath("count(/node())")).to eq(1.0) # only the root element under /
    expect(doc.root.previous).to be_nil            # no doctype sibling before the root
  end

  it "rejects a second DOCTYPE and a DOCTYPE after the root" do
    expect { Makiri::XML("<!DOCTYPE a><!DOCTYPE b><a/>") }
      .to raise_error(Makiri::XML::SyntaxError)
    expect { Makiri::XML("<a/><!DOCTYPE a>") }
      .to raise_error(Makiri::XML::SyntaxError)
  end

  it "serializes back to a DOCTYPE declaration" do
    expect(dtd("<!DOCTYPE html><html/>").to_xml).to eq("<!DOCTYPE html>")
    expect(dtd(%(<!DOCTYPE r SYSTEM "r.dtd"><r/>)).to_xml).to eq(%(<!DOCTYPE r SYSTEM "r.dtd">))
  end
end
