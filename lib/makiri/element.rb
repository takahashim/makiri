# frozen_string_literal: true

module Makiri
  # An element node (HTML or XML). Attribute access lives in C.
  class Element < Node
    # Create a detached element named +name+ owned by +document+ (Nokogiri-style
    # constructor; delegates to {Document#create_element}, so its representation
    # follows the document). Attach it with {Node#add_child} and friends.
    #
    # @param name [String]
    # @param document [Makiri::Document]
    # @return [Makiri::Element]
    def self.new(name, document)
      Makiri::Document.coerce!(document).create_element(name)
    end
  end
end
