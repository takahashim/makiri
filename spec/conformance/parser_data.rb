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

  # The named character references. The list is closed by the HTML Standard
  # ("this list is static and will not be expanded or changed in the
  # future"), and html-build generates the file rather than keeping it in git,
  # so it is pinned by its digest instead of a commit.
  ENTITIES_URL = "https://html.spec.whatwg.org/entities.json"
  ENTITIES_SHA256 = "d741d877ac77c4194c4ad526b5b4a19aef8dfe411ab840a466891cdbb9f362e6"
  ENTITIES_PATH = File.expand_path("data/whatwg/entities.json", __dir__)

  module_function

  # Fetch entities.json unless the pinned copy is there.
  def ensure_entities!
    require "digest"
    return if File.exist?(ENTITIES_PATH) && Digest::SHA256.file(ENTITIES_PATH).hexdigest == ENTITIES_SHA256

    require "open-uri"
    puts "fetching #{ENTITIES_URL} -> #{ENTITIES_PATH}"
    body = URI.parse(ENTITIES_URL).open("rb", &:read)
    got = Digest::SHA256.hexdigest(body)
    abort "entities.json digest #{got} is not the pinned #{ENTITIES_SHA256}" unless got == ENTITIES_SHA256
    FileUtils.mkdir_p(File.dirname(ENTITIES_PATH))
    File.binwrite(ENTITIES_PATH, body)
  end

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
  # The entries that hold for `parser` ("lexbor" or "port"): a line's third
  # field names the parsers that still diverge, both when absent.
  def known_divergences(parser)
    File.readlines(File.expand_path("parser_known_divergences.txt", __dir__), chomp: true)
        .map(&:strip)
        .reject { |l| l.empty? || l == "#" || l.start_with?("# ") }
        .map(&:split)
        .select { |_, _, parsers| parsers.nil? || parsers.split(",").include?(parser) }
        .to_h { |key, id, _| [key, id] }
  end
end

if $PROGRAM_NAME == __FILE__
  ParserData.ensure!(ParserData::WPT)
  ParserData.ensure!(ParserData::HTML5LIB)
  ParserData.ensure_entities!
end
