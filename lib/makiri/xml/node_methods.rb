# frozen_string_literal: true

module Makiri
  module XML
    # Ruby additions over the C-defined XML node readers, mirroring
    # Makiri::HTML::NodeMethods so the XML node surface matches the HTML one for
    # the methods consumers (e.g. Dommy) rely on. Each is guarded with
    # `method_defined?` so a future native implementation on this module takes
    # precedence rather than being shadowed.
    module NodeMethods
      # Element ancestors, nearest first, excluding self (element nodes only) —
      # matching Makiri::HTML's #ancestors.
      unless method_defined?(:ancestors)
        def ancestors
          out = []
          node = parent
          while node
            out << node if node.node_type == 1
            node = node.respond_to?(:parent) ? node.parent : nil
          end
          out
        end
      end

      # Whether an attribute with the given qualified name is present
      # (case-sensitive, per XML).
      unless method_defined?(:key?)
        def key?(name)
          wanted = name.to_s
          attribute_nodes.any? { |attr| attr.name == wanted }
        end
      end

      alias_method :has_attribute?, :key? unless method_defined?(:has_attribute?)
    end
  end
end
