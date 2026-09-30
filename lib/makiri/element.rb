# frozen_string_literal: true

module Makiri
  # An element node (HTML or XML). Attribute access lives in the extension.
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

    private

    # See {NodePath#path}. An unprefixed name test selects elements in the
    # HTML namespace in HTML and in none in XML; SVG, MathML, namespaced XML
    # and names like "o:p" are written by expanded name.
    def path_step
      name_step("", unprefixed_element_namespace)
    end

    # See {NodePath#path}. An element step selects by expanded name, not by the
    # string of the test: `*[local-name()='BR' and ...]`, written for `h:BR`,
    # also selects the unprefixed `BR` beside it. A bare name test on an HTML
    # element folds ASCII case, as the engine (and browsers) match it, so `div`
    # also selects the `DIV` createElementNS makes. Compared as strings, each
    # pair was counted apart, and the path found the other one first.
    def path_selects?(other, test)
      return false unless other.element? && other.namespace_uri.to_s == namespace_uri.to_s

      mine = local_name
      theirs = other.local_name
      return mine == theirs unless test == name && unprefixed_element_namespace

      mine.downcase(:ascii) == theirs.downcase(:ascii)
    end
  end
end
