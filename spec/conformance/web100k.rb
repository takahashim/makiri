# frozen_string_literal: true
#
# The ported HTML tree builder against Lexbor's over web100k
# (https://github.com/EmilStenstrom/web100k): 100,000 real homepages in ten
# zstd-compressed release batches (~3.1 GB in all). Each batch is downloaded,
# its pages decompressed with the corpus's zstd dictionary, and the crate's
# `the_port_builds_lexbors_tree_on_web100k` test run over them - the same tree,
# source offsets included, and the same guard verdict, page for page. A batch
# is deleted once checked unless KEEP=1, so the disk needed is one batch
# (~2 GB decompressed). Not run in CI.
#
#   ruby spec/conformance/web100k.rb            # all ten batches
#   BATCHES=1,2 ruby spec/conformance/web100k.rb
#
# Needs `curl`, `tar`, `zstd` and the vendored Lexbor build (`rake compile`).
# Pages that differ are copied to spec/conformance/data/web100k/diff/.

require "fileutils"
require "open3"

module Web100k
  REPO = "https://github.com/EmilStenstrom/web100k.git"
  # The commit html.dict is read from; the batches are its release assets.
  COMMIT = "b3222801339f1f9505a6806041874b7c4278758e"
  RELEASE = "https://github.com/EmilStenstrom/web100k/releases/download"
  DIR = File.expand_path("data/web100k", __dir__)
  RUST = File.expand_path("../../ext/makiri/rust", __dir__)

  module_function

  def sh(*cmd)
    system(*cmd) or abort "failed: #{cmd.join(' ')}"
  end

  # The zstd dictionary every page was compressed with.
  def dict
    path = File.join(DIR, "html.dict")
    return path if File.exist?(path)

    repo = File.join(DIR, "repo")
    FileUtils.mkdir_p(repo)
    sh("git", "-C", repo, "init", "-q")
    system("git", "-C", repo, "remote", "add", "origin", REPO, err: File::NULL)
    sh("git", "-C", repo, "fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", COMMIT)
    File.binwrite(path, IO.popen(["git", "-C", repo, "show", "FETCH_HEAD:html.dict"], &:read))
    path
  end

  # Download batch `i`, decompress its pages into a directory of `*.html`, and
  # return that directory.
  def pages(i)
    tag = format("web100k-batch-%03d", i)
    arc = File.join(DIR, "#{tag}.tar.zst")
    out = File.join(DIR, tag)
    unless File.exist?(arc)
      puts "[#{tag}] downloading"
      sh("curl", "-sSL", "--retry", "4", "-o", arc, "#{RELEASE}/#{tag}/#{tag}.tar.zst")
    end
    FileUtils.rm_rf(out)
    FileUtils.mkdir_p(out)
    sh("sh", "-c", 'zstd -q -d -c "$1" | tar -x -C "$2"', "sh", arc, out)
    zst = Dir.glob(File.join(out, "**", "*.html.zst"))
    # Decompressed beside the archive members, then gathered flat.
    zst.each_slice(500) { |s| sh("zstd", "-q", "-f", "--rm", "-D", dict, "-d", *s) }
    Dir.glob(File.join(out, "**", "*.html")).each do |f|
      dest = File.join(out, File.basename(f))
      FileUtils.mv(f, dest) unless f == dest
    end
    puts "[#{tag}] #{Dir.glob(File.join(out, '*.html')).size} pages"
    [out, arc]
  end

  # Run the crate's comparison over `dir`; true when every page agreed.
  def compare(dir)
    env = { "WEB100K_DIR" => dir, "WEB100K_OUT" => File.join(DIR, "diff") }
    cmd = %w[cargo test --release --no-default-features --features lexbor
             the_port_builds_lexbors_tree_on_web100k -- --ignored --nocapture]
    out, status = Open3.capture2e(env, *cmd, chdir: RUST)
    puts out.lines.grep(/\Aweb100k:/)
    puts out unless status.success?
    status.success?
  end

  def run(batches)
    FileUtils.mkdir_p(DIR)
    failed = batches.reject do |i|
      dir, arc = pages(i)
      ok = compare(dir)
      unless ENV["KEEP"] == "1"
        FileUtils.rm_rf(dir)
        FileUtils.rm_f(arc)
      end
      ok
    end
    abort "web100k: batches #{failed.join(', ')} differ" unless failed.empty?
    puts "web100k: batches #{batches.join(', ')} agree"
  end
end

if $PROGRAM_NAME == __FILE__
  batches = ENV.fetch("BATCHES", "1,2,3,4,5,6,7,8,9,10").split(",").map(&:to_i)
  Web100k.run(batches)
end
