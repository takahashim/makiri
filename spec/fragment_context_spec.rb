# frozen_string_literal: true

require "spec_helper"

# The element a fragment is parsed in the context of, when that element is one
# only the DOM API makes - an element of another document - and a String
# context.
RSpec.describe "fragment parsing in an API-made context" do
  let(:doc) { Makiri::HTML("<html><body></body></html>") }

  describe "an element of another document" do
    it "gives the fragment a namespace of its own document, which outlives the other" do
      frag = nil
      uri = "urn:#{"a" * 64}"
      1.times do
        other = Makiri::HTML("<p>")
        frag = doc.fragment("<qq/>", context: other.create_element_ns(uri, "zzcustom"))
      end
      el = frag.children.first
      GC.start
      GC.start
      # Reuse the memory the other document's tables held.
      _keep = Array.new(500) { |i| Makiri::HTML("<p>").tap { |d| d.create_element_ns("urn:#{"B" * (i % 80 + 1)}", "x#{i}") } }
      expect(el.namespace_uri).to eq(uri)
      expect(el.xpath("namespace-uri(.)")).to eq(uri)
    end

    it "resolves an HTML context the same as one of the target document" do
      td_ctx = Makiri::HTML("<table><tr></tr></table>").at_css("tr")
      expect(doc.fragment("<td>y", context: td_ctx).children.map(&:name)).to eq(["td"])
    end
  end

  describe "a String context" do
    # Names Lexbor's tag table holds for nodes that are not elements.
    ["#text", "#document", "#comment", "!--", "!doctype", "?ProcessingInstruction",
     "#end-of-file", "#fragment-context"].each do |name|
      it "refuses #{name.inspect}" do
        expect { doc.fragment("<b>", context: name) }.to raise_error(ArgumentError, /unknown fragment context/)
        expect { Makiri::DocumentFragment.parse("<b>", context: name) }.to raise_error(ArgumentError)
      end
    end
  end
end
