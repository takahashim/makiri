# frozen_string_literal: true

module Makiri
  # An HTML element node. Attribute access lives in C.
  class Element < Node
    # Create a detached element named +name+ owned by +document+ (Nokogiri-style
    # constructor; delegates to {Document#create_element}). Attach it with
    # {Node#add_child} and friends.
    #
    # @param name [String]
    # @param document [Makiri::Document]
    # @return [Makiri::Element]
    def self.new(name, document)
      document.create_element(name)
    end
  end
end
