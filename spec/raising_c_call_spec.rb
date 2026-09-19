# frozen_string_literal: true

# Two arguments reach a Ruby C function that refuses them by raising: a Range
# whose bound does not fit a `long`, and an `encoding:` naming no encoding. Both
# are asked inside a Rust frame that owns something - NodeSet#[] holds a copy of
# the set's nodes, to_xml is about to hold the serializer's buffer - so the
# refusal comes back as a return value rather than unwinding past them.
#
# The error Ruby gives is what callers see, unchanged. What the unwind used to
# cost was invisible from Ruby: one leaked copy of the node pointers per call
# (measured at ~900 bytes a call before this), which is why the checks below are
# about the contract, not the leak.
RSpec.describe "arguments a Ruby C function refuses" do
  describe "NodeSet#[] with a Range" do
    let(:set) { Makiri::HTML("<div>#{'<p>x</p>' * 5}</div>").css("p") }

    it "raises RangeError for a bound too large for a long, and stays usable" do
      expect { set[0..10**20] }.to raise_error(RangeError)
      expect(set.length).to eq(5)
      expect(set[0..1].map(&:text)).to eq(%w[x x])
    end

    it "still answers nil for a start past the end" do
      expect(set[9..10]).to be_nil
      expect(set[5..6]).not_to be_nil        # a start AT the end is the empty set
      expect(set[5..6].length).to eq(0)
    end

    it "keeps the ordinary Range forms" do
      expect(set[1..2].length).to eq(2)
      expect(set[-2..].length).to eq(2)
      expect(set[0...2].length).to eq(2)
    end
  end

  describe "XML Node#to_xml(encoding:)" do
    let(:doc) { Makiri::XML("<r><c>t</c></r>") }

    it "raises ArgumentError for an unknown encoding, and still serializes after" do
      expect { doc.root.to_xml(encoding: "NoSuchEnc") }
        .to raise_error(ArgumentError, /unknown encoding name/)
      expect(doc.root.to_xml).to eq("<r><c>t</c></r>")
    end

    it "raises TypeError for something that is not an encoding name" do
      expect { doc.root.to_xml(encoding: Object.new) }.to raise_error(TypeError)
      expect(doc.root.to_xml).to eq("<r><c>t</c></r>")
    end

    it "keeps transcoding to a named encoding" do
      out = doc.root.to_xml(encoding: "Shift_JIS")
      expect(out.encoding).to eq(Encoding::Shift_JIS)
      expect(out.encode("UTF-8")).to eq("<r><c>t</c></r>")
    end
  end
end
