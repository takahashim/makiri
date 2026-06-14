# frozen_string_literal: true

module Makiri
  # An XML/HTML processing-instruction node. Rare in HTML5 (the parser usually
  # treats "<?...>" as a bogus comment), present mainly for completeness.
  class ProcessingInstruction < Node
    # Create a detached processing instruction owned by +document+ (Nokogiri-style,
    # document first). Delegates to {Document#create_processing_instruction}.
    #
    # @param document [Makiri::Document]
    # @param target [String]
    # @param content [String]
    # @return [Makiri::ProcessingInstruction]
    def self.new(document, target, content)
      Makiri::Document.coerce!(document).create_processing_instruction(target, content)
    end

    # DOM `ProcessingInstruction#target` — the PI's target name. Defined once on
    # the shared base so both backends expose it: the XML node reaches it here
    # (its target IS its node name), while the HTML node overrides it with a
    # native implementation earlier in the ancestor chain.
    def target
      name
    end
  end
end
