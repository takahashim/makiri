# frozen_string_literal: true

# Makiri catching a Ruby exception of its own (a failed transcode, say) must
# leave the exception Ruby is already handling alone. CRuby crashes when a
# native extension clears the one that is still propagating and Ruby then keeps
# unwinding with nothing to unwind (https://bugs.ruby-lang.org/issues/22240;
# magnus hit it, its #188). What keeps Makiri clear of it is that every caught
# error is either raised again - which sets `$!` itself - or put back
# (`raw::keeping_errinfo`); these run the situations the bug report names, and
# fail if a path clears `$!` and returns normally.
RSpec.describe "an exception Ruby is already handling" do
  let(:doc) { Makiri::HTML("<p>x</p>") }
  # Transcoding this to UTF-8 raises EncodingError inside Ruby, which Makiri
  # catches and answers with a Makiri::Error of its own.
  let(:bad) { "\xE3".dup.force_encoding("Shift_JIS") }

  def refuses(doc, bad)
    doc.css(bad)
    nil
  rescue Makiri::Error => e
    e
  end

  it "is still the one propagating through an ensure that made Makiri catch another" do
    reached = nil
    begin
      begin
        raise "in flight"
      ensure
        refuses(doc, bad)
      end
    rescue => e
      reached = e
    end
    expect(reached.message).to eq("in flight")
  end

  it "is still $! in a rescue body, and the cause of what Makiri then raises" do
    begin
      raise "handled"
    rescue => handled
      error = refuses(doc, bad)
      expect(error).to be_a(Makiri::Error)
      expect(error.cause).to equal(handled)
      expect($!).to equal(handled)
    end
  end

  it "leaves $! nil when nothing was in flight" do
    refuses(doc, bad)
    expect($!).to be_nil
  end

  it "does not stop a throw from reaching its catch" do
    result = catch(:out) do
      begin
        throw :out, :thrown
      ensure
        refuses(doc, bad)
      end
    end
    expect(result).to eq(:thrown)
  end

  it "does not stop a break out of NodeSet#each" do
    result = doc.css("p").each do
      begin
        break :broken
      ensure
        refuses(doc, bad)
      end
    end
    expect(result).to eq(:broken)
  end

  it "survives a signal handler that calls Makiri while a raise is in flight",
     skip: (Signal.list.key?("USR1") ? false : "no SIGUSR1 here") do
    # Evaluated here: a `let` read inside a trap handler takes RSpec's mutex.
    document = doc
    invalid = bad
    previous = Signal.trap("USR1") { refuses(document, invalid) }
    reached = nil
    begin
      begin
        raise "in flight"
      ensure
        Process.kill("USR1", Process.pid)
        sleep 0.05
      end
    rescue => e
      reached = e
    end
    expect(reached.message).to eq("in flight")
  ensure
    Signal.trap("USR1", previous || "DEFAULT") if Signal.list.key?("USR1")
  end
end
