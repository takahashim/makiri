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

  # A class of wrapped objects is the extension's own: one defined first is
  # refused before anything is defined, so the load can be rescued and the
  # process goes on - the same check that refuses a second `Init_` after a
  # `require` an exit cut short.
  it "raises LoadError when Makiri::Node is defined before the extension" do
    lib = File.expand_path("../lib", __dir__)
    code = <<~RUBY
      module Makiri; class Node; end; end
      begin
        require "makiri"
        puts "loaded"
      rescue LoadError => e
        puts "LoadError: \#{e.message}"
      end
      puts Makiri.const_defined?(:Element, false) ? "defined" : "nothing defined"
    RUBY
    out, err, status = Open3.capture3(
      { "RUBY_FREE_AT_EXIT" => nil }, RbConfig.ruby, "-I#{lib}", "-e", code
    )
    expect(status).to be_success, err
    expect(out).to include("LoadError: Makiri::Node is already defined")
    expect(out).to include("nothing defined")
  end

  # Init stopped half-way - here by a hook of the program's, as a Timeout or
  # a Thread#raise could - reaches the `require` as it was raised (or, for a
  # `throw`, as it was thrown), not as `fatal`; the methods already defined
  # refuse to run, and a second `require` is a LoadError.
  { "an exception" => ['raise "stop here"', "rescue => e; puts \"stopped: \#{e.message}\""],
    "a throw" => ["throw :stop", ""] }.each do |kind, (stop, rescue_clause)|
    it "leaves the process running when #{kind} stops Init_, and the extension unusable" do
      lib = File.expand_path("../lib", __dir__)
      code = <<~RUBY
        module Makiri
          def self.singleton_method_added(m) = (#{stop} if m == :__panic)
        end
        catch(:stop) do
          begin
            require "makiri"
            puts "loaded"
          #{rescue_clause}
          end
        end
        begin
          Makiri.__alloc_inject?
          puts "ran"
        rescue Exception => e
          puts "refused: \#{e.class}: \#{e.message}"
        end
        begin
          require "makiri/makiri"
        rescue LoadError => e
          puts "again: \#{e.message}"
        end
      RUBY
      out, err, status = Open3.capture3(
        { "RUBY_FREE_AT_EXIT" => nil }, RbConfig.ruby, "-I#{lib}", "-e", code
      )
      expect(status).to be_success, err
      expect(out.lines.map(&:chomp)).not_to include("loaded")
      expect(out).to include("stopped: stop here") if kind == "an exception"
      expect(out).to include("refused: Makiri::InternalError: the extension stopped while defining itself")
      expect(out).to include("again: Makiri::Node is already defined")
    end
  end
end
