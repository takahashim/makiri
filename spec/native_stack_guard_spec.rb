# frozen_string_literal: true

require "spec_helper"

# `crate::stack`/`bridge::stack` (see there for why): a recursion COUNT alone
# assumes the caller's own frame started with a full-sized native stack, which
# is not true in a small Fiber - `RUBY_FIBER_MACHINE_STACK_SIZE` can be as low
# as 128 KiB, well under what the count-only caps (CSS's MAX_SELECTOR_NESTING,
# XPath's max_recursion_depth) assumed when they were sized. Before the guard,
# a selector/expression comfortably inside those counts still overflowed the
# REAL stack in a small Fiber: Ruby's SystemStackError handling longjmps past
# Rust's frames, skipping every Drop on the way - which for the XML CSS lowering
# meant the process-global CSS parser's borrow was never released, wedging
# EVERY later XML css/at_css/matches? call in the process behind "CSS parser is
# already in use".
#
# `RUBY_FIBER_MACHINE_STACK_SIZE` is read once at boot, so each case here runs
# in a child process (the same isolation `xml_html_boundary_spec.rb` uses) with
# it set below the default (512 KiB) before Ruby starts.
RSpec.describe "native stack guard" do
  def run_isolated(env, code)
    lib = File.expand_path("../lib", __dir__)
    out = IO.popen([env.merge("RUBY_FREE_AT_EXIT" => nil), RbConfig.ruby, "-I#{lib}",
                    "-e", %(require "makiri"\n#{code})],
                   err: %i[child out], &:read)
    [$?, out]
  end

  # Nested well inside each count-only cap (CSS's 128, XPath's 256), so the
  # native stack - not the count - is what has to stop it.
  let(:css_nesting) { ":not(" * 127 + "b" + ")" * 127 }
  let(:xpath_nesting) { "not(" * 255 + "1" + ")" * 255 }

  describe "XML CSS lowering, inside a small Fiber" do
    it "raises LimitExceeded instead of SystemStackError, and leaves the shared CSS parser usable" do
      status, out = run_isolated({ "RUBY_FIBER_MACHINE_STACK_SIZE" => "131072" }, <<~RUBY)
        x = Makiri::XML("<r><a/></r>")
        begin
          Fiber.new { x.css(#{css_nesting.inspect}) }.resume
          print "no_error"
        rescue => e
          print e.class
        end
        print ":"
        begin
          x.css("a")
          print "after_ok"
        rescue => e
          print "after_\#{e.message}"
        end
      RUBY
      expect(status).to be_success
      expect(out).to eq("Makiri::XPath::LimitExceeded:after_ok")
    end
  end

  describe "XPath evaluation, inside a small Fiber" do
    it "raises LimitExceeded instead of SystemStackError, on both HTML and XML" do
      status, out = run_isolated({ "RUBY_FIBER_MACHINE_STACK_SIZE" => "131072" }, <<~RUBY)
        h = Makiri::HTML("<p>x</p>")
        x = Makiri::XML("<r/>")
        [h, x].each do |doc|
          begin
            Fiber.new { doc.xpath(#{xpath_nesting.inspect}) }.resume
            print "no_error;"
          rescue => e
            print "\#{e.class};"
          end
        end
      RUBY
      expect(status).to be_success
      expect(out).to eq("Makiri::XPath::LimitExceeded;Makiri::XPath::LimitExceeded;")
    end
  end

  describe "at the default Fiber stack size" do
    it "is unaffected: the same inputs answer exactly as outside a Fiber" do
      x = Makiri::XML("<r><a/></r>")
      outside = x.css(css_nesting) rescue $! # rubocop:disable Style/RescueModifier
      inside = Fiber.new { x.css(css_nesting) rescue $! }.resume # rubocop:disable Style/RescueModifier
      expect(inside.class).to eq(outside.class)
    end
  end
end
