# frozen_string_literal: true
#
# The pinned external data the HTML parser conformance runners read, fetched on
# demand (sparse, blobless, one commit) into spec/conformance/data/ (gitignored).
# The pins are the ones docs/parser_differences.ja.md fixes beside the HTML
# Standard commit; bump them together.
#
#   ruby spec/conformance/parser_data.rb          # fetch whatever is missing
#
# Used by html5lib_runner.rb and by `rake conformance:parser`, which runs the
# crate's parser tests over the same files.

require "fileutils"

module ParserData
  Source = Struct.new(:name, :repo, :commit, :path, :glob, keyword_init: true) do
    def dir = File.expand_path("data/#{name}", __dir__)
    def data_dir = File.join(dir, path)
    def present? = !Dir.glob(File.join(data_dir, glob)).empty?
  end

  # Tree construction: the html5lib .dat files, maintained in WPT since 2026-06.
  WPT = Source.new(name: "wpt", repo: "https://github.com/web-platform-tests/wpt.git",
                   commit: "5ffee20edef3bd97fad8ab03c5e2108c2f48acec",
                   path: "html/syntax/parsing/resources", glob: "*.dat")
  # Tokenizer: still maintained in html5lib-tests.
  HTML5LIB = Source.new(name: "html5lib-tests", repo: "https://github.com/html5lib/html5lib-tests.git",
                        commit: "c777c408b61078ea2eb4acefc2535f54dbc8b28a",
                        path: "tokenizer", glob: "*.test")

  module_function

  # Fetch `src` unless its files are there and checked out at the pinned commit.
  def ensure!(src)
    head = `git -C #{src.dir} rev-parse HEAD 2>/dev/null`.strip
    return if src.present? && head == src.commit

    puts "fetching #{src.name}@#{src.commit[0, 12]} #{src.path} (sparse) -> #{src.dir}"
    FileUtils.mkdir_p(src.dir)
    run = lambda do |*args|
      system("git", "-C", src.dir, *args) or abort "git #{args.first} failed"
    end
    run.call("init", "-q")
    system("git", "-C", src.dir, "remote", "add", "origin", src.repo, err: File::NULL)
    run.call("sparse-checkout", "init", "--cone")
    run.call("sparse-checkout", "set", src.path)
    # GitHub permits fetching an exact commit by SHA; --depth 1 keeps it shallow.
    run.call("fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", src.commit)
    run.call("checkout", "-q", "FETCH_HEAD")
  end

  # The classified known divergences: { "tests1.dat#40" => "P1" }.
  def known_divergences
    File.readlines(File.expand_path("parser_known_divergences.txt", __dir__), chomp: true)
        .map(&:strip)
        .reject { |l| l.empty? || l == "#" || l.start_with?("# ") }
        .to_h { |l| l.split(/\s+/, 3).first(2) }
  end
end

if $PROGRAM_NAME == __FILE__
  ParserData.ensure!(ParserData::WPT)
  ParserData.ensure!(ParserData::HTML5LIB)
end
