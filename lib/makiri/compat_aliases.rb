# frozen_string_literal: true

module Makiri
  # Nokogiri-compatible class-name aliases.
  #
  # Makiri's canonical node-class names are the WHATWG DOM interface names.
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
