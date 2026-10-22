# frozen_string_literal: true

# A Document's Ruby wrapper is made before the parse that fills it (so an
# allocation failure leaks nothing), and is empty until then - while a parse
# runs with the GVL released, and for good when the parse fails. Such a
# wrapper must not be reachable: ObjectSpace used to hand it out, and every
# method on it raised Makiri::InternalError.
RSpec.describe "a Document's wrapper before it has content" do
  def documents(klass) = ObjectSpace.each_object(klass).to_a

  [
    ["HTML", Makiri::HTML::Document, -> { Makiri::HTML("<div>" * 1000) }],
    ["XML", Makiri::XML::Document, -> { Makiri::XML("<r>") }],
  ].each do |kind, klass, failing_parse|
    it "is not found through ObjectSpace after a failed #{kind} parse" do
      GC.disable
      begin
        before = documents(klass).map(&:object_id)
        expect { failing_parse.call }.to raise_error(Makiri::Error)
        documents(klass).reject { |d| before.include?(d.object_id) }.each do |d|
          expect { d.root }.not_to raise_error
        end
      ensure
        GC.enable
      end
    end
  end

  it "is a Document of its class once parsed" do
    doc = Makiri::HTML("<p>x</p>")
    expect(doc.class).to eq(Makiri::HTML::Document)
    expect(documents(Makiri::HTML::Document)).to include(doc)
  end
end
