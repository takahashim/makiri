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

    it "as ASCII-8BIT, from a String's bytes read as they are, whatever its encoding" do
      bin = "\xFF\x00\xFE".b
      copy = NupurTest.binary_copy(bin)
      expect(copy).to eq(bin)
      expect(copy.encoding).to eq(Encoding::ASCII_8BIT)
      expect(copy).not_to equal(bin)

      utf16 = "ab".encode("UTF-16LE")
      expect(NupurTest.binary_copy(utf16)).to eq(utf16.b)
      expect(NupurTest.binary_copy("")).to eq("".b)
      expect(NupurTest.binary_copy("\x82".dup.force_encoding("Shift_JIS"))).to eq("\x82".b)
    end
  end

  describe "a wrapped object shared between Ractors" do
    it "is frozen and shareable from birth, with what its slot holds" do
      held = +"slot"
      o = NupurTest::Shared.new(7, held)
      expect(o).to be_frozen
      expect(Ractor.shareable?(o)).to be(true)
      expect(held).to be_frozen
      expect(o.held).to equal(held)
    end

    it "is read from other Ractors without a copy" do
      skip "no Ractor" unless defined?(Ractor)
      o = NupurTest::Shared.new(42, "x")
      rs = 3.times.map { Ractor.new(o) { |shared| [shared.n, shared.object_id] } }
      results = rs.map { |r| r.respond_to?(:value) ? r.value : r.take }
      expect(results).to all(eq([42, o.object_id]))
    end

    it "refuses a slot value that cannot be made shareable" do
      not_shareable = Object.new.instance_eval { proc { self } }
      expect { NupurTest::Shared.new(1, not_shareable) }.to raise_error(Ractor::Error)
    end

    it "is not shareable when its type does not ask to be" do
      expect(Ractor.shareable?(NupurTest::Holder.new(1))).to be(false)
      expect { Ractor.make_shareable(NupurTest::Holder.new(1)) }.to raise_error(Ractor::Error)
    end
  end

  describe "a String read in place" do
    it "lends its bytes to a closure that cannot touch Ruby" do
      expect(NupurTest.lent_sum("abc")).to eq(97 + 98 + 99)
      expect(NupurTest.lent_sum("")).to eq(0)
      expect(NupurTest.lent_sum("x" * 10_000)).to eq(120 * 10_000)
    end

    it "is locked while lent to a reader that may run Ruby, and unlocked after" do
      s = +"hello"
      expect(NupurTest.locked_len(s) { |str| str.length }.first).to eq(5)
      expect { NupurTest.locked_len(s) { |str| str << "!" } }.to raise_error(RuntimeError, /locked/)
      expect { s << "!" }.not_to raise_error
      expect(s).to eq("hello!")
    end

    it "leaves the String unlocked when the reader's Ruby raises" do
      s = +"x"
      expect { NupurTest.locked_len(s) { raise IOError, "inside" } }.to raise_error(IOError, "inside")
      expect { s << "y" }.not_to raise_error
    end

    it "answers whether Ruby holds a String to be UTF-8, scanning once and keeping it" do
      expect(NupurTest.knows_utf8(+"abc")).to be(true)
      expect(NupurTest.knows_utf8(+"é")).to be(true)
      expect(NupurTest.knows_utf8("\xFF".dup.force_encoding("UTF-8"))).to be(false)
      expect(NupurTest.knows_utf8("\xC3\xA9".b)).to be(false)
      expect(NupurTest.knows_utf8("ab".b)).to be(true)
    end
  end

  describe "a value's kind" do
    it "is read once, with the handle" do
      expect([nil, true, false, 1, 2**70, 1.5, 1e300, "s", :s, [], {}, Object.new, 1r].map { NupurTest.kind_of(_1) })
        .to eq(%i[nil true false fixnum bignum float float string symbol array hash other other])
    end
  end

  describe "values built without a call each" do
    def bits(x) = [x].pack("D").unpack1("Q")

    it "makes the same Float Ruby makes, a flonum where Ruby's is one" do
      edge = [0.0, -0.0, 1.0, -1.0, 0.1, 1.5, 1e-77, 1.72723e-77, 1e77, 1e78, 1e-78, 5e-324,
              Float::MAX, Float::MIN, Float::INFINITY, -Float::INFINITY, Float::EPSILON,
              2.0**255, 2.0**256, 2.0**-255, 2.0**-256]
      rng = Random.new(7)
      randoms = Array.new(20_000) { [rng.rand(2**64)].pack("Q").unpack1("D") }.reject(&:nan?)
      (edge + randoms).each do |x|
        made = NupurTest.float_of_bits(bits(x))
        expect(bits(made)).to eq(bits(x))
        # An immediate is the same object as Ruby's own; a heap Float never is.
        expect(made.equal?(x)).to eq(x.equal?(x + 0.0))
      end
      expect(NupurTest.float_of_bits(bits(Float::NAN))).to be_nan
    end

    it "makes an Array of several values, and appends several, in one call each" do
      expect(NupurTest.ary_bulk(1, "b", :c, nil, 2.5)).to eq([1, "b", :c, nil, 2.5])
      expect(NupurTest.ary_bulk).to eq([])
    end

    it "inserts several pairs in one call, a later key winning" do
      expect(NupurTest.hash_bulk("a", 1, :b, 2, "a", 3)).to eq({ "a" => 3, b: 2 })
      expect(NupurTest.hash_bulk).to eq({})
    end

    it "refuses a key Ruby would hash with a method call, and an odd count" do
      expect { NupurTest.hash_bulk([1], 2) }.to raise_error(TypeError)
      expect { NupurTest.hash_bulk(Class.new(String).new("k"), 2) }.to raise_error(TypeError)
      expect { NupurTest.hash_bulk("a", 1, "b") }.to raise_error(ArgumentError, /odd/)
    end
  end

  describe "a method called" do
    it "calls a public method with its arguments and answers its value" do
      expect(NupurTest.send_public([3, 1, 2], "sort")).to eq([1, 2, 3])
      expect(NupurTest.send_public(5, "+", 2)).to eq(7)
    end

    it "refuses a private method, as public_send does" do
      o = Class.new { private def secret = 1 }.new
      expect { NupurTest.send_public(o, "secret") }.to raise_error(NoMethodError, /private method/)
    end

    it "passes the method's raise, throw and break back through" do
      o = Object.new
      def o.boom = raise(IOError, "from Ruby")
      def o.toss = throw(:out, 42)
      expect { NupurTest.send_public(o, "boom") }.to raise_error(IOError, "from Ruby")
      expect(catch(:out) { NupurTest.send_public(o, "toss") }).to eq(42)
      expect([1, 2].each { |x| break x * 10 if NupurTest.send_public(x, "odd?") }).to eq(10)
    end

    it "may call back into the extension" do
      o = Object.new
      def o.again = NupurTest.send_public(self, "object_id")
      expect(NupurTest.send_public(o, "again")).to eq(o.object_id)
    end

    it "keeps the value it answers alive under GC.stress" do
      o = Object.new
      def o.fresh = Array.new(100) { |i| "s#{i}" }
      GC.stress = true
      begin
        v = NupurTest.send_public(o, "fresh")
      ensure
        GC.stress = false
      end
      GC.start
      expect(v.last).to eq("s99")
    end

    it "asks respond_to? for a public method, honouring respond_to_missing?" do
      expect(NupurTest.responds_to_s(Object.new)).to be(true)
      expect(NupurTest.responds_to_s(BasicObject.new)).to be(false)
      hidden = Class.new { private def to_s = "x" }.new
      expect(NupurTest.responds_to_s(hidden)).to be(false)
    end

    it "tells whether a constant is defined, loading nothing" do
      expect(NupurTest.object_const_defined("String")).to be(true)
      expect(NupurTest.object_const_defined("NoSuchConstantHere")).to be(false)
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

  describe "immediates and numbers" do
    it "makes true and false" do
      expect(NupurTest.booleans).to eq([true, false])
    end

    it "reads an Integer as a usize, refusing a negative one and one past it" do
      expect(NupurTest.usize_of(0)).to eq(0)
      expect(NupurTest.usize_of(2**62)).to eq(2**62)
      expect(NupurTest.usize_of(2**64 - 1)).to eq(2**64 - 1)
      expect { NupurTest.usize_of(-1) }.to raise_error(RangeError)
      expect { NupurTest.usize_of(2**64) }.to raise_error(RangeError)
    end

    it "tells a builtin class's own instance from a subclass's and a singleton's" do
      expect(NupurTest.exact_class([])).to eq(:Array)
      expect(NupurTest.exact_class({})).to eq(:Hash)
      expect(NupurTest.exact_class(+"s")).to eq(:String)
      expect(NupurTest.exact_class(2**70)).to eq(:Integer)
      # 1e300 is past a flonum's range, so a heap Float with a header.
      expect(NupurTest.exact_class(1e300)).to eq(:Float)
      expect(NupurTest.exact_class(1.5)).to be_nil
      expect(NupurTest.exact_class(Class.new(Hash).new)).to be_nil
      s = +"s"
      def s.to_json(*) = "x"
      expect(NupurTest.exact_class(s)).to be_nil
      # Immediates have no header: a Fixnum, a Symbol, nil.
      expect(NupurTest.exact_class(1)).to be_nil
      expect(NupurTest.exact_class(:a)).to be_nil
      expect(NupurTest.exact_class(nil)).to be_nil
    end

    it "writes an Integer's decimal digits, a Bignum's included" do
      [0, 7, -7, 2**62, -(2**62), 2**63, -(2**63), 10**40, -(10**40)].each do |n|
        expect(NupurTest.decimal_of(n)).to eq(n.to_s)
      end
    end

    it "makes an Integer from decimal digits, refusing anything else" do
      %w[0 -1 9223372036854775807 9223372036854775808 -9223372036854775809].each do |d|
        expect(NupurTest.int_from_decimal(d)).to eq(Integer(d))
      end
      expect(NupurTest.int_from_decimal("1" * 400)).to eq(Integer("1" * 400))
      ["", "-", "1_000", " 1", "1e3", "0x10", "+1", "1\0"].each do |bad|
        expect { NupurTest.int_from_decimal(bad) }.to raise_error(ArgumentError)
      end
    end

    it "reads a Symbol's name" do
      expect(NupurTest.symbol_name(:abc)).to eq("abc")
      expect(NupurTest.symbol_name(:"é")).to eq("é")
    end

    it "reads a Float as an f64, bit for bit" do
      bits = ->(x) { [x].pack("D").unpack1("Q") }
      [0.0, -0.0, 1.5, 1e300, -2.5e-310, Float::INFINITY, Float::NAN].each do |x|
        expect(NupurTest.f64_bits_of(x)).to eq(bits.(x))
      end
    end
  end
end
