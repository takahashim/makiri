# frozen_string_literal: true

module Makiri
  # An XML/HTML processing-instruction node. Rare in HTML5 (the parser usually
  # treats "<?...>" as a bogus comment), present mainly for completeness.
  class ProcessingInstruction < Node
  end
end
