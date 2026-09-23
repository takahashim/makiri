# frozen_string_literal: true

module Makiri
  # An XML/HTML processing-instruction node. The HTML parser produces one for
  # <tt><?target data?></tt> (the HTML Standard's processing-instruction token;
  # Nokogiri::HTML5 still reads it as a comment). {#target} is native on both
  # representations.
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

  end
end
