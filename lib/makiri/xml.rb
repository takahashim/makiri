# frozen_string_literal: true

module Makiri
  # XML-specific node leaves and document conveniences (§12), mirroring
  # Makiri::HTML. The XML nodes and the document are defined in the extension
  # (ext/makiri/rust/src/glue/xml_doc.rs, xml_node/); the per-class Ruby
  # additions live in this namespace's files (xml/node_methods.rb,
  # xml/document.rb, xml/builder.rb).
  module XML
  end
end
