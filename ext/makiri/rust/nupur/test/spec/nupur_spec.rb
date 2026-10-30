# frozen_string_literal: true

# nupur's guarantees, checked through its test extension alone
# (`rake nupur:spec`): no raise crosses Rust, no panic reaches C, an exit is
# resumed as it left, a wrapped object keeps what its slot holds, a String is
# read as checked UTF-8.
require "nupur_test"

RSpec.describe "nupur" do
  describe "a method's result and error" do
    it "returns what the method answered" do
      obj = Object.new
      expect(NupurTest.echo(obj)).to equal(obj)
    end

    it "raises the method's Err once it has returned" do
      expect { NupurTest.arg_error }.to raise_error(ArgumentError, "nupur: a refused argument")
    end
  end

  describe "a panic" do
    it "is the registered internal error, which a bare rescue lets through" do
      expect { NupurTest.panic_in(0, nil) }.to raise_error(NupurTest::InternalError, /panic in a method/)
      expect(NupurTest::InternalError.ancestors).not_to include(StandardError)
    end

    it "with the GVL released is caught there and raised once it is back" do
      expect { NupurTest.panic_in(1, nil) }.to raise_error(NupurTest::InternalError, /without the GVL/)
    end

    it "in a Hash walk's callback is latched, not unwound into C" do
      expect { NupurTest.panic_in(2, { a: 1 }) }.to raise_error(NupurTest::InternalError, /Hash walk/)
    end

    it "leaves the process usable" do
      3.times { expect { NupurTest.panic_in(0, nil) }.to raise_error(NupurTest::InternalError) }
      expect(NupurTest.echo(1)).to eq(1)
    end
  end

  describe "a block's exit" do
    it "passes a break out as it left" do
      seen = []
      result = NupurTest.yield_each(5) do |i|
        seen << i
        break :early if i == 2
      end
      expect(result).to eq(:early)
      expect(seen).to eq([0, 1, 2])
    end

    it "passes a throw and an exception out as they left" do
      expect(catch(:out) { NupurTest.yield_each(3) { throw :out, :thrown } }).to eq(:thrown)
      expect { NupurTest.yield_each(3) { raise IOError, "from the block" } }.to raise_error(IOError, "from the block")
    end

    it "is resumed even when the method dropped it" do
      expect(catch(:out) { NupurTest.drop_exit { throw :out, :kept } ; :lost }).to eq(:kept)
    end
  end

  describe "in several Ractors at once" do
    # Each Ractor runs methods under its own GVL, so a pending exit kept for
    # the whole process would be forgotten, or resumed, by another Ractor's
    # method running between its catch and its resumption.
    def in_ractors(count, code)
      previous = Warning[:experimental]
      Warning[:experimental] = false
      ractors = Array.new(count) { Ractor.new(code) { |source| eval(source) } } # rubocop:disable Security/Eval
      ractors.map { |r| r.respond_to?(:value) ? r.value : r.take }
    ensure
      Warning[:experimental] = previous
    end

    it "resumes each Ractor's exit in that Ractor" do
      results = in_ractors(4, <<~RUBY)
        kept = 0
        20_000.times do |i|
          kept += 1 if catch(:out) { NupurTest.drop_exit { throw :out, :kept }; :lost } == :kept
          NupurTest.yield_each(2) { |j| break j } if i.odd?
          NupurTest.echo(i)
        end
        kept
      RUBY
      expect(results).to eq([20_000] * 4)
    end

    it "cannot share, copy or move a wrapped object to another Ractor" do
      holder = NupurTest::Holder.new("x")
      expect { Ractor.make_shareable(holder) }.to raise_error(Ractor::Error)
      [false, true].each do |move|
        previous = Warning[:experimental]
        Warning[:experimental] = false
        ractor = Ractor.new { Ractor.receive }
        expect { ractor.send(holder, move: move) }.to raise_error(StandardError)
        ractor.send(nil)
        ractor.respond_to?(:value) ? ractor.value : ractor.take
      ensure
        Warning[:experimental] = previous
      end
      expect(holder.held).to eq("x")
    end
  end

  describe "a wrapped object" do
    it "keeps its slot's value alive and in place across collections" do
      holders = Array.new(50) { |i| NupurTest::Holder.new("held #{i}" * 3) }
      GC.start
      GC.compact if GC.respond_to?(:compact)
      expect(holders.map(&:held)).to eq(Array.new(50) { |i| "held #{i}" * 3 })
    end

    it "lends its data through its handle under GC.stress" do
      h = NupurTest::Holder.new(:x)
      GC.stress = true
      begin
        counts = Array.new(20) { h.bump }
      ensure
        GC.stress = false
      end
      expect(counts).to eq((1..20).to_a)
    end

    it "is refused as another type with Ruby's own TypeError" do
      expect { NupurTest::Holder.instance_method(:held).bind_call(Object.new) }.to raise_error(TypeError)
    end

    it "cannot be allocated without its payload" do
      expect { NupurTest::Holder.allocate }.to raise_error(TypeError)
    end
  end

  describe "a String read as checked UTF-8" do
    it "copies UTF-8, and transcodes another encoding first" do
      expect(NupurTest.read_text("café")).to eq("café")
      expect(NupurTest.read_text("café".encode("Shift_JIS", fallback: { "é" => "e" }))).to eq("cafe")
      expect(NupurTest.read_text("日本".encode("Shift_JIS"))).to eq("日本")
    end

    it "refuses what the caller's check refuses, invalid UTF-8, and a String with no UTF-8 form" do
      expect { NupurTest.read_text("a\0b") }.to raise_error(ArgumentError, "nupur: holds a NUL")
      expect { NupurTest.read_text("\xff".b) }.to raise_error(ArgumentError, "nupur: not UTF-8")
      expect { NupurTest.read_text("\x82".dup.force_encoding("Shift_JIS")) }
        .to raise_error(ArgumentError, "nupur: no UTF-8 form")
    end

    it "keeps the caller's $! when the String's encoding is at fault" do
      begin
        raise "outer"
      rescue RuntimeError
        expect { NupurTest.read_text("\x82".dup.force_encoding("Shift_JIS")) }.to raise_error(ArgumentError)
        expect($!.message).to eq("outer")
      end
    end

    it "holds a capped read to the UTF-8 the String becomes" do
      expect(NupurTest.read_capped("abc", 3)).to eq(3)
      expect { NupurTest.read_capped("abcd", 3) }.to raise_error(RangeError, "nupur: too long")
      long = ("x" * 100).encode("UTF-16LE")
      expect { NupurTest.read_capped(long, 10) }.to raise_error(RangeError, "nupur: too long")
    end

    it "tells a String read as it is from one transcoded, and from the String's fault" do
      expect(NupurTest.transcode_kind("abc")).to eq(:as_is)
      expect(NupurTest.transcode_kind("abc".b)).to eq(:as_is)
      expect(NupurTest.transcode_kind("abc".encode("UTF-16LE"))).to eq(:transcoded)
      expect(NupurTest.transcode_kind("\x82".dup.force_encoding("Shift_JIS"))).to eq(:fault)
    end
  end

  describe "a String made" do
    it "is written in place to its full length" do
      expect(NupurTest.filled(5)).to eq("aaaaa")
      expect(NupurTest.filled(5).encoding).to eq(Encoding::UTF_8)
    end

    it "from slices, refusing a total they fall short of" do
      expect(NupurTest.slices(false)).to eq("abcd")
      expect { NupurTest.slices(true) }.to raise_error(NupurTest::InternalError)
    end
  end

  describe "a method's arguments" do
    it "reads a keyword, nil when absent; a braced Hash is positional" do
      expect(NupurTest.kw(a: 1)).to eq(1)
      expect(NupurTest.kw).to be_nil
      expect { NupurTest.kw({ a: 1 }) }.to raise_error(ArgumentError, /given 1, expected 0/)
    end

    it "words an arity error as Ruby does" do
      expect(NupurTest.shape(1)).to eq(1)
      expect(NupurTest.shape(1, 2)).to eq(2)
      expect { NupurTest.shape }.to raise_error(ArgumentError, "wrong number of arguments (given 0, expected 1..2)")
      expect { NupurTest.shape(1, 2, 3) }.to raise_error(ArgumentError, /given 3, expected 1\.\.2/)
    end
  end

  describe "a Hash, the GVL, and frozen values" do
    it "walks a Hash's pairs" do
      expect(NupurTest.pairs({ a: 1, "b" => 2 })).to eq([[:a, 1], ["b", 2]])
    end

    it "runs a body with the GVL released and returns its answer" do
      expect(NupurTest.sum_without_gvl(1000)).to eq(499_500)
    end

    it "answers a frozen value with Ruby's own FrozenError" do
      expect(NupurTest.frozen_check(+"x")).to be_nil
      expect { NupurTest.frozen_check("x".freeze) }.to raise_error(FrozenError)
    end
  end
end
