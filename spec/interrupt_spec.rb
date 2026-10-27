# frozen_string_literal: true

# An XPath evaluation holds the GVL, so a signal handler, a `Timeout` or a
# `Thread#raise` could not reach its thread until it returned. Its budget now
# asks, every ~260k ops, whether an interrupt waits (lib/makiri's
# `crate::interrupt`): if one does, the evaluation STOPS - every guard dropped -
# the interrupt is delivered, and the evaluation runs again (an evaluation only
# reads). It does that at most twice, then runs the last pass unasked, so it
# always finishes. These need an interrupt that arrives while the evaluation
# runs; a child process sends signals, which set Ruby's flag without the GVL.
#
# :slow - signals and fork add nothing under Valgrind or the sanitizers.
RSpec.describe "an interrupt during a query", :slow,
               skip: (Signal.list.key?("USR1") && Process.respond_to?(:fork) ? false : "needs fork and SIGUSR1") do
  # ~3000 paragraphs; the expression visits ~4.5M nodes - tens of ms, over the
  # budget's first chunk, and well under the 50M-op budget.
  let(:doc) do
    Makiri::HTML("<div>#{(1..3000).map { |i| "<p>#{i}</p>" }.join}</div>")
  end
  let(:query) { "//p[count(following::p) > 1000]" }
  let(:expected) { 1999 } # paragraphs 1..1999 have more than 1000 after them

  HandlerRaised = Class.new(StandardError)

  def elapsed
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  end

  # Sends SIGUSR1 to this process every few ms until killed.
  def with_signals(every: 0.004)
    parent = Process.pid
    child = fork do
      loop do
        Process.kill("USR1", parent)
        sleep every
      end
    end
    yield
  ensure
    if child
      Process.kill("KILL", child)
      Process.wait(child)
    end
    # Signals still waiting are dropped, not run by the handler a test set.
    Signal.trap("USR1", "IGNORE")
  end

  around do |example|
    previous = Signal.trap("USR1", "IGNORE")
    example.run
  ensure
    Signal.trap("USR1", previous || "DEFAULT")
  end

  it "reaches a handler that raises, long before the evaluation would have ended" do
    document = doc
    expression = query
    full = elapsed { document.xpath(expression) }
    skip "the query is too quick here to catch (#{(full * 1000).round} ms)" if full < 0.02

    Signal.trap("USR1") { raise HandlerRaised, "from the handler" }
    error = nil
    took = elapsed do
      with_signals do
        document.xpath(expression)
      rescue HandlerRaised => e
        error = e
      end
    end
    expect(error&.message).to eq("from the handler")
    expect(took).to be < full
  end

  it "runs the query again after a handler that returns, and answers as it would have" do
    document = doc
    expression = query
    handled = 0
    Signal.trap("USR1") { handled += 1 }

    answers = with_signals do
      Array.new(15) { document.xpath(expression).size }
    end
    expect(answers.uniq).to eq([expected])
    expect(handled).to be > 0
  end

  it "delivers with the document free: a handler's edit made mid-query is not refused" do
    # The handler runs when the interrupt is delivered. Had the evaluation kept
    # its read guard, the edit would be refused; and it must run well BEFORE the
    # query returns (a handler that waited for the return would run at its very
    # end), or nothing was delivered mid-query at all.
    document = doc
    expression = query
    now = -> { Process.clock_gettime(Process::CLOCK_MONOTONIC) }
    full = elapsed { document.xpath(expression) }
    skip "the query is too quick here to catch (#{(full * 1000).round} ms)" if full < 0.02

    edits = []
    Signal.trap("USR1") do
      outcome = begin
        document.at_css("div").add_child(document.create_element("i"))
        :edited
      rescue Makiri::Error => e
        e.message
      end
      edits << [now.call, outcome]
    end
    returned_at = nil
    with_signals(every: 0.002) do
      document.xpath(expression)
      returned_at = now.call
    end
    early = edits.select { |at, _| returned_at - at > full * 0.25 }
    expect(early).not_to be_empty
    expect(early.map(&:last).uniq).to eq([:edited])
  end
end
