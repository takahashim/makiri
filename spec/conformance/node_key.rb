# frozen_string_literal: true

# The key the Nokogiri differentials compare nodes by: the absolute path with
# each step reduced to its local name.
#
# The two libraries' DOM trees are isomorphic, but they render the path of a
# foreign (SVG/MathML) element differently - Nokogiri qualifies it
# ("svg:circle"), Makiri names it by expanded name
# ("*[local-name()='circle' and namespace-uri()='...']", which needs no prefix
# registered to evaluate). That is a Node#path rendering nuance, NOT a
# selection difference, so it is normalised away here, keeping the comparison
# about which nodes matched. The expanded-name form is reduced first: its URI
# holds slashes, which the per-step split would cut through.
module NodeKey
  EXPANDED_STEP = /(@?)\*\[local-name\(\)='([^']*)' and namespace-uri\(\)='[^']*'\]/
  PREFIXED_STEP = /\A(@?)[\w-]+:/

  def self.of(node)
    node.path
        .gsub(EXPANDED_STEP) { "#{Regexp.last_match(1)}#{Regexp.last_match(2)}" }
        .split("/").map { |seg| seg.sub(PREFIXED_STEP, "\\1") }.join("/")
  end
end
