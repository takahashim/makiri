# frozen_string_literal: true

# The extension does not declare itself Ractor-safe (no rb_ext_ractor_safe), so
# Ruby refuses every native method of it outside the main Ractor, before the
# method runs. That is what keeps an unshareable ivar read (`rb_attr_get` ->
# RactorIsolationError) out of the extension's reach, and so what the C API
# raise survey's `REVIEWED` entry for `rb_attr_get` rests on
# (ext/makiri/rust/lints/makiri_lints/src/raising.rs). Break this and that
# reason no longer holds.
#
# :slow - a Ractor is a thread Valgrind and the sanitizers add nothing to.
RSpec.describe "Makiri outside the main Ractor", :slow do
  # Evaluates `code` in a Ractor and answers the error it died with (a Ractor's
  # block cannot capture this scope, so the code goes in as a String). Ruby 4.0
  # renamed Ractor#take to #value.
  def in_ractor(code)
    previous = Warning[:experimental]
    Warning[:experimental] = false
    ractor = Ractor.new(code) do |source|
      Thread.current.report_on_exception = false
      eval(source) # rubocop:disable Security/Eval
    end
    begin
      ractor.respond_to?(:value) ? ractor.value : ractor.take
      nil
    rescue Ractor::RemoteError => e
      e.cause
    end
  ensure
    Warning[:experimental] = previous
  end

  it "refuses an HTML parse" do
    expect(in_ractor('Makiri::HTML("<p>x</p>")')).to be_a(Ractor::UnsafeError)
  end

  it "refuses an XML parse" do
    expect(in_ractor('Makiri::XML("<r/>")')).to be_a(Ractor::UnsafeError)
  end

  it "refuses an XPathContext" do
    expect(in_ractor("Makiri::XPathContext.new")).to be_a(Ractor::UnsafeError)
  end
end
