# frozen_string_literal: true

require "spec_helper"

# Steps of the pinned HTML Standard that Lexbor, the parser the port was
# translated from, lacks, and the port takes (docs/parser_differences.ja.md,
# the P items). Lexbor's DOM (MAKIRI_LEXBOR_DOM=1) still answers the old way,
# so these run on the port only.
RSpec.describe "HTML Standard steps the port takes and Lexbor does not" do
  before { skip "Lexbor's DOM answers the old way" if Makiri.send(:__lexbor_dom?) }

  # P1: whatwg/html#12908 - "after head", anything else: the implicit <body>
  # sets frameset-ok back to "ok".
  it "lets a frameset replace the implicit body after a template in the head" do
    doc = Makiri::HTML("<head><template></template></head><p><frameset>")
    expect(doc.at_xpath("/html/frameset")).not_to be_nil
    expect(doc.at_xpath("/html/body")).to be_nil
  end
end
