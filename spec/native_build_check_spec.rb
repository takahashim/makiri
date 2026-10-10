# frozen_string_literal: true

require "open3"
require "rbconfig"

# lib/makiri.rb refuses a native extension built as another makiri version
# (the check after the extension's require); one built for another Ruby is
# refused by the extension's own init, before it defines anything.
RSpec.describe "native extension build check" do
  it "states the gem version and Ruby API version it was built as" do
    expect(Makiri::NATIVE_VERSION).to eq(Makiri::VERSION)
    expect(Makiri::NATIVE_RUBY_API_VERSION).to eq(RUBY_VERSION[/\A\d+\.\d+/])
    expect(Makiri::NATIVE_VERSION).to be_frozen
    expect(Makiri::NATIVE_RUBY_API_VERSION).to be_frozen
  end

  # The binary cannot be swapped for a stale one here, so the gem's version is
  # changed instead, before lib/makiri.rb reads it: version.rb is already
  # loaded, so its require_relative does not put the real one back.
  # A binary compiled before the constants existed defines neither; that is
  # the stale case the check is for, so it must be a LoadError, not NameError.
  it "raises LoadError when the binary does not state what it was built as" do
    lib = File.expand_path("../lib", __dir__)
    code = <<~RUBY
      require "makiri/version"
      require "makiri/makiri"
      Makiri.send(:remove_const, :NATIVE_VERSION)
      begin
        require "makiri"
        puts "loaded"
      rescue LoadError => e
        puts "LoadError: \#{e.message}"
      end
    RUBY
    out, err, status = Open3.capture3(
      { "RUBY_FREE_AT_EXIT" => nil }, RbConfig.ruby, "-I#{lib}", "-e", code
    )
    expect(status).to be_success, err
    expect(out).to include("LoadError: makiri's native extension was built as makiri (unknown),")
  end

  it "raises LoadError when the binary was built as another gem version" do
    lib = File.expand_path("../lib", __dir__)
    code = <<~RUBY
      require "makiri/version"
      Makiri.send(:remove_const, :VERSION)
      Makiri::VERSION = "0.0.0-stale"
      begin
        require "makiri"
        puts "loaded"
      rescue LoadError => e
        puts e.message
      end
    RUBY
    out, err, status = Open3.capture3(
      { "RUBY_FREE_AT_EXIT" => nil }, RbConfig.ruby, "-I#{lib}", "-e", code
    )
    expect(status).to be_success, err
    expect(out).to include("built as makiri #{Makiri::VERSION}, but")
    expect(out).to include("loaded by makiri 0.0.0-stale")
  end
end
