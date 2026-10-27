# frozen_string_literal: true

# A non-local exit - a `throw`, a `break`, a thread kill - that a protected
# call caught comes back to Rust as an `Err` and is resumed by the method's
# trampoline as the method returns. A method that dropped that `Err` (read
# a failed call as "no answer") must not lose the exit with it: the
# trampoline resumes whatever is still pending, on the thread that caught it.
# `Makiri.__drop_exit` is that method (see `init.rs`).
RSpec.describe "a non-local exit a method dropped" do
  it "resumes a throw to its catch" do
    result = catch(:out) do
      Makiri.__drop_exit { throw :out, 42 }
      :lost
    end
    expect(result).to eq(42)
  end

  it "resumes a break out of the block" do
    expect(Makiri.__drop_exit { break :broke }).to eq(:broke)
  end

  it "still ends a killed thread" do
    thread = Thread.new do
      Makiri.__drop_exit { Thread.current.kill }
      :survived
    end
    expect(thread.value).to be_nil
    expect(thread.status).to be(false)
  end

  it "leaves nothing pending for the next method" do
    catch(:out) { Makiri.__drop_exit { throw :out } }
    expect(Makiri::HTML("<p>x</p>").at_css("p").text).to eq("x")
  end

  it "leaves an exception in the block a plain error the method dropped" do
    expect(Makiri.__drop_exit { raise "dropped" }).to be_nil
  end
end
