# frozen_string_literal: true

# A Rust panic must reach Ruby as an exception, not kill the host process.
#
# This is the one property with no other test. `panic = "abort"` - what the
# crate used to build with - would leave every other example green and the gem
# lethal to its host: an abort runs no `ensure`, no `at_exit` and no destructor,
# and takes the whole process down with SIGABRT. `Makiri.__panic(kind)` exists
# to be caught here (see `init.rs`).
#
# What a panic becomes is Ruby's `fatal`, via magnus's `catch_unwind`. Ruby does
# not let a `rescue` in the SAME frame catch a `fatal`, so the examples below
# test it the way a host actually meets it: on the thread that ran the query.
# There the process survives, the error is catchable at the join, and `ensure`
# has already run.
RSpec.describe "a Rust panic" do
  KINDS = {
    0 => "deliberate panic",
    1 => "index out of bounds",
    2 => "subtract with overflow",
    3 => "unwrap",
    # Below `rb_thread_call_without_gvl`, so the unwind starts under a C frame.
    # Rust turns an unwind at an `extern "C"` boundary into an ABORT, so this
    # one only reaches Ruby because `bridge::gvl` latches it and re-raises once
    # the GVL is back - the same shape every Lexbor callback now uses. It is
    # the parser's own path: the whole parse runs under that frame.
    4 => "panic below the GVL"
  }.freeze

  around do |example|
    was = Thread.report_on_exception
    Thread.report_on_exception = false
    example.run
  ensure
    Thread.report_on_exception = was
  end

  # The panic, as the host meets it: raised on the thread that ran the query.
  def panic_on_thread(kind)
    Thread.new { Makiri.__panic(kind) }.value
    nil
  rescue Exception => e # rubocop:disable Lint/RescueException
    e
  end

  KINDS.each do |kind, fragment|
    it "kind #{kind} (#{fragment}) surfaces as a catchable exception" do
      e = panic_on_thread(kind)
      expect(e).not_to be_nil, "the panic did not surface as an exception"
      expect(e.message).to include(fragment)
    end
  end

  it "kills only the thread that panicked" do
    panic_on_thread(0)
    expect(Makiri::HTML("<p>after</p>").at_css("p").text).to eq("after")
    expect(Thread.new { Makiri::XML("<r>ok</r>").root.text }.value).to eq("ok")
  end

  it "runs ensure blocks on the way out" do
    released = false
    Thread.new do
      Makiri.__panic(0)
    ensure
      released = true
    end.join
  rescue Exception # rubocop:disable Lint/RescueException
    expect(released).to be true
  end

  it "is not a StandardError - an internal error is not a bad argument" do
    expect(panic_on_thread(0)).not_to be_a(StandardError)
  end

  it "leaves the shared CSS engine usable after a panic" do
    # `lexbor::selectors::PanicReset` is what guarantees this. The engine is
    # process-global, so a panic that skipped its reset would leave the parser
    # in a non-CLEAN stage for every LATER query, not just the failed one.
    panic_on_thread(1)
    doc = Makiri::HTML("<div id='a'><p class='c'>x</p></div>")
    expect(doc.css("div p.c").length).to eq(1)
    expect(doc.at_css("#a")).not_to be_nil
    expect { doc.css("div >>> p") }.to raise_error(Makiri::CSS::SyntaxError)
    expect(doc.css("p").length).to eq(1)
  end

  it "leaves parsing and serialization usable after a panic" do
    panic_on_thread(2)
    doc = Makiri::HTML("<html><body><div>a<span>b</span></div></body></html>")
    expect(doc.at_css("div").to_html).to eq("<div>a<span>b</span></div>")
    expect(doc.at_css("div").text).to eq("ab")
    expect(doc.at_xpath("//span").line).to eq(1)
  end

  it "leaves parsing usable after one below the GVL" do
    # The parse runs under `rb_thread_call_without_gvl`; a panic there used to
    # abort at the `extern "C"` boundary before anything could release.
    panic_on_thread(4)
    expect(Makiri::HTML("<html><body><p>a</p></body></html>").at_css("p").text).to eq("a")
    expect(Makiri::XML("<r><c>b</c></r>").at_xpath("//c").text).to eq("b")
  end

  describe "inside an entry point exposed to untrusted input" do
    # `bridge::ruby::entry` wraps the methods a crafted document or expression
    # reaches - parse, fragment, xpath/at_xpath/evaluate, css/at_css/matches?,
    # the serializers and the text readers. There a panic is not a `fatal`: the
    # host can catch it and turn one request into a 500 without a thread
    # boundary to re-raise at.
    it "raises Makiri::InternalError, in the frame that called it" do
      expect { Makiri.__panic(5) }
        .to raise_error(Makiri::InternalError, /panic inside a guarded entry/)
    end

    it "is not a StandardError, so a bare rescue still passes it through" do
      expect(Makiri::InternalError.ancestors).to include(Exception)
      expect(Makiri::InternalError <= StandardError).to be_falsey

      swallowed = begin
        Makiri.__panic(5)
      rescue StandardError
        :swallowed
      rescue Makiri::InternalError
        :passed_through
      end
      expect(swallowed).to eq(:passed_through)
    end

    it "leaves the process working" do
      begin
        Makiri.__panic(5)
      rescue Makiri::InternalError
        nil
      end
      expect(Makiri::HTML("<p>after</p>").at_css("p").text).to eq("after")
    end
  end

  it "rejects an unknown kind with an ordinary ArgumentError" do
    expect { Makiri.__panic(99) }.to raise_error(ArgumentError, /kind must be/)
  end
end
