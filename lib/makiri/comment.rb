# frozen_string_literal: true

module Makiri
  class Comment < Node
    # Create a detached comment owned by +document+ (Nokogiri-style constructor;
    # the document comes FIRST for Comment / CDATASection / ProcessingInstruction, unlike
    # Element / Text). Delegates to {Document#create_comment}.
    #
    # @param document [Makiri::Document]
    # @param content [String]
    # @return [Makiri::Comment]
    def self.new(document, content)
      Makiri::Document.coerce!(document).create_comment(content)
    end
  end
end
