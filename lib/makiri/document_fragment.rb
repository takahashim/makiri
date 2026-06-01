# frozen_string_literal: true

module Makiri
  # A detached group of sibling nodes. Build one with {.parse} (its own backing
  # document) or {Makiri::Document#fragment} (bound to an existing document, so
  # its nodes can be spliced in with {Node#add_child} and friends). Inserting a
  # fragment contributes its children, not the fragment node itself.
  #
  # +.parse+ is defined in C (ext/makiri/glue/ruby_doc.c).
  class DocumentFragment < Node
  end
end
