# frozen_string_literal: true

require "spec_helper"
require "open3"

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
    # stdout and stderr are kept apart: under AddressSanitizer the runtime
    # prints its own warning block to stderr ("==N==WARNING: ASan is ignoring
    # requested __asan_handle_no_return", then "False positive error reports
    # may follow" / "For details see .../issues/189") when Ruby's
    # SystemStackError unwind longjmps past Rust's frames in the small Fiber.
    # These specs compare the child's stdout byte for byte, so folding stderr
    # in (as xml_html_boundary_spec.rb does, for a different noise source)
    # would prefix it. stderr is returned for the failure message instead.
    out, err, status = Open3.capture3(
      { "RUBY_FREE_AT_EXIT" => nil }.merge(env),
      RbConfig.ruby, "-I#{lib}", "-e", %(require "makiri"\n#{code})
    )
    [status, out, err]
  end

  # Nested well inside each count-only cap (CSS's 128, XPath's 256), so the
  # native stack - not the count - is what has to stop it.
  let(:css_nesting) { ":not(" * 127 + "b" + ")" * 127 }
  let(:xpath_nesting) { "not(" * 255 + "1" + ")" * 255 }

  describe "XML CSS lowering, inside a small Fiber" do
    it "raises LimitExceeded instead of SystemStackError, and leaves the shared CSS parser usable" do
      status, out, err = run_isolated({ "RUBY_FIBER_MACHINE_STACK_SIZE" => "131072" }, <<~RUBY)
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
      expect(out).to eq("Makiri::XPath::LimitExceeded:after_ok"), err
    end
  end

  describe "XPath evaluation, inside a small Fiber" do
    it "raises LimitExceeded instead of SystemStackError, on both HTML and XML" do
      status, out, err = run_isolated({ "RUBY_FIBER_MACHINE_STACK_SIZE" => "131072" }, <<~RUBY)
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
      expect(out).to eq("Makiri::XPath::LimitExceeded;Makiri::XPath::LimitExceeded;"), err
    end
  end

  # HTML matching needs no guard at all: `lexbor::css_match` keeps every
  # nesting level (`:is`/`:where`/`:not`, and `:has()` since it moved to
  # `Frame::HasStep`, `of S` since `Frame::NthOfStep`) on its own heap stack.
  # `:has()` and `:nth-child(1 of ...)` nested 300 deep each used to recurse
  # natively: `:has()` crashed the whole process with an uncaught
  # SystemStackError, `of S` raised one that wedged the shared CSS engine for
  # the rest of the process. Now they answer, or raise the work budget - and
  # the engine stays usable either way.
  describe "HTML CSS matching, inside a small Fiber" do
    it "answers deep :has()/:is()/:not() nesting or raises the budget, never SystemStackError" do
      status, out, err = run_isolated({ "RUBY_FIBER_MACHINE_STACK_SIZE" => "131072" }, <<~RUBY)
        def run(doc, sel)
          r = Fiber.new { doc.css(sel) }.resume
          print "n=\#{r.length};"
        rescue Exception => e
          print "\#{e.class}:\#{e.message[/budget|could not/]};"
        end
        deep = Makiri::HTML("<body>" + "<div>" * 302 + "x" + "</div>" * 302 + "</body>")
        run(deep, ":has(" * 300 + "div" + ")" * 300)
        mixed = "body" + ":is(:not(p):has(" * 100 + "div" + "))" * 100
        run(deep, mixed)
        run(Makiri::HTML("<a>x</a>"), ":is(" * 2000 + "a" + ")" * 2000)
        run(Makiri::HTML("<a>x</a>"), ":nth-child(1 of " * 2000 + "a" + ")" * 2000)
        print "after=\#{deep.css('div').length}"
      RUBY
      expect(status).to be_success, err
      expect(out).to eq("Makiri::Error:budget;n=1;n=1;n=1;after=302"), err
    end
  end

  # The selector text `parse_stylesheet` returns is written on a heap work
  # list (`lexbor::selector_text`); Lexbor's own serializer it replaced
  # recursed once per nested list.
  describe "stylesheet selector text, inside a small Fiber" do
    it "writes deeply nested selector lists back in full" do
      # The child keeps RUBY_FREE_AT_EXIT when spec:valgrind sets it: ruby_memcheck
      # tells a leak from a live object only by Ruby freeing everything at exit,
      # and these selector texts are tens of KB of heap Strings our extension
      # made - without the teardown, each one still alive at exit is reported
      # "definitely lost". (stderr's free-at-exit warning is not compared.)
      env = { "RUBY_FIBER_MACHINE_STACK_SIZE" => "131072",
              "RUBY_FREE_AT_EXIT" => ENV.fetch("RUBY_FREE_AT_EXIT", nil) }
      status, out, err = run_isolated(env, <<~RUBY)
        [":is(", ":not(", ":has(", ":nth-child(1 of "].each do |open|
          sel = open * 5000 + ".a\\\\:b" + ")" * 5000
          r = Fiber.new { Makiri::Lexbor::CSS.parse_stylesheet(sel + "{x:y}") }.resume
          print r[0][:selectors][0][:text] == sel ? "ok;" : "differs;"
        end
      RUBY
      expect(status).to be_success, err
      expect(out).to eq("ok;ok;ok;ok;"), err
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
