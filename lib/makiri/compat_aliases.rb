# frozen_string_literal: true

module Makiri
  # Nokogiri-compatible class-name aliases.
  #
  # Makiri's canonical node-class names are the WHATWG DOM interface names
  # (Element, Attr, Text, Comment, CDATASection, ProcessingInstruction,
  # DocumentType, Document, DocumentFragment). Two of those differ from the
  # libxml2/Nokogiri spelling; we expose the Nokogiri names as aliases so a
  # snippet ported from Nokogiri (or an is_a?/case check) resolves unchanged:
  #
  #   CDATASection  <- CDATA   (Nokogiri::XML::CDATA)
  #   DocumentType  <- DTD     (Nokogiri::XML::DTD)
  #
  # An alias is the same class object, so #is_a? works under either name; only
  # #class.name (and inspect) report the canonical DOM name. Defined at all three
  # levels (the abstract base and the HTML::/XML:: leaves).
  CDATA = CDATASection
  DTD = DocumentType

  module HTML
    CDATA = CDATASection
    DTD = DocumentType
  end

  module XML
    CDATA = CDATASection
    DTD = DocumentType
  end
end
