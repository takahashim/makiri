# frozen_string_literal: true

# `rake spec:valgrind` - run the spec suite under Valgrind memcheck via
# ruby_memcheck (Linux CI; see .github/workflows/valgrind.yml). The gem ships
# Ruby's own Valgrind suppression files (matched by Ruby version) and filters
# the report down to errors whose stack touches our extension, so we no longer
# have to fetch ruby.supp from ruby/ruby (that path was removed upstream).
#
# The job catches *invalid reads/writes* (incl. intra-arena overflows) and
# any "definitely lost" block whose stack touches our extension - the one leak
# gate with no frequency threshold (`rake leaks`, macOS, flags only stacks
# repeated >= 30x). It once meant to leave leaks out and could not: see
# `--leak-check` below.
#
# It does NOT check uninitialised values (`--undef-value-errors=no`, which is
# ruby_memcheck's default too). It did, and every report it ever kept was
# CRuby's: the conservative GC scans the machine stack and reads words a
# returned frame left behind, and Valgrind names as their origin whichever
# frame last owned that stack - often one of ours. Which words are stale
# shifts with the build's stack layout, so the suppressions (~45 entries)
# grew with every runner image and never stayed whole. Safe Rust cannot read
# uninitialised memory; `falloc`'s and `utf8_copy`'s `unsafe` runs under Miri;
# and nupur's reads are of memory Ruby initialised. What is given up is an
# uninitialised read in nupur's own `unsafe`.
#
# `filter_all_errors: true` applies ruby_memcheck's "stack must touch the
# makiri binary" filter to every error kind, not only leaks
# (`ValgrindError#should_filter? = filter_all_errors? || kind_leak?`), so the
# `:isolated` specs' child processes under `--trace-children=yes` do not bury
# the run in Ruby's own reports. A real invalid access in our code still has
# a makiri frame and is still reported. `suppressions/ruby.supp` (auto-loaded
# by ruby_memcheck: it globs `<dir>/<ruby-version>.supp`, and the bare
# `ruby.supp` matches every version) holds the leaks the VM never frees
# before exit.
#
# Guarded: ruby_memcheck lives in the optional :valgrind bundler group, so a
# normal `bundle exec rake` (without that group) must not fail to load.
begin
  require "ruby_memcheck"
  require "ruby_memcheck/rspec/rake_task"

  # Valgrind 3.26.0 (Ubuntu 26.04) writes `<still_reachable>` in its XML
  # leak summary WITHOUT the closing tag when nothing is still reachable:
  #
  #   <still_reachable>
  #     <bytes>0</bytes>
  #     <blocks>0</blocks>
  #   <suppressed>
  #
  # (with a block still reachable the tag closes). ruby_memcheck 3.0.1 then
  # cannot parse the file ("Opening and ending tag mismatch: still_reachable
  # and leak_summary"), on every process that frees everything, after every
  # example has passed. Close the tag before ruby_memcheck reads the file; a
  # file Valgrind closed itself is not touched. Drop this once Valgrind or
  # ruby_memcheck fixes it.
  require "zlib"
  module RepairUnclosedStillReachable
    UNCLOSED = %r{(<still_reachable>\s*<bytes>[^<]*</bytes>\s*<blocks>[^<]*</blocks>\s*)(?=<suppressed>)}

    def self.repair(xml)
      xml.gsub(UNCLOSED) { "#{Regexp.last_match(1)}</still_reachable>\n  " }
    end

    private

    def parse_valgrind_output
      keep = ENV["VALGRIND_XML_KEEP"]
      FileUtils.mkdir_p(keep) if keep
      valgrind_xml_files.each do |file|
        xml = File.read(file)
        fixed = RepairUnclosedStillReachable.repair(xml)
        File.write(file, fixed) unless fixed == xml
        # ruby_memcheck deletes the XML once parsed; keep a gzipped copy to read
        # an error's whole stacks (VALGRIND_XML_KEEP=<dir>, set by valgrind.yml).
        Zlib::GzipWriter.open(File.join(keep, "#{File.basename(file)}.gz")) { |gz| gz.write(fixed) } if keep
      end
      super
    end
  end
  RubyMemcheck::TestTaskReporter.prepend(RepairUnclosedStillReachable)

  RubyMemcheck.config(
    binary_name: "makiri",
    filter_all_errors: true,    # apply the binary-touch filter to ALL error kinds,
                                # not just leaks (see note above)
    valgrind_options: [
      "--num-callers=50",
      "--error-limit=no",
      "--trace-children=yes",   # spec processes may fork
      "--undef-value-errors=no",  # see the note above; also ruby_memcheck's default
      # Leaks ARE checked here, whatever this says: ruby_memcheck always runs
      # valgrind with --xml=yes, and in XML mode memcheck writes its leak
      # records even under --leak-check=no (a "definitely lost" in a spec's
      # child process failed run 37864917248 that way). So say what happens,
      # and keep ruby_memcheck's own leak kinds - these options REPLACE its
      # defaults rather than adding to them, and without the line below
      # valgrind's default (definite,possible) fails the job on a "possibly
      # lost" too. A spec that runs Ruby in a child must leave
      # RUBY_FREE_AT_EXIT set there (it is inherited): without it the child's
      # live strings at exit are reported as definitely lost through our
      # frames (PR #55).
      "--leak-check=full",
      "--show-leak-kinds=definite",
      # Options of a manual run (valgrind.yml's `valgrind_options`).
      *ENV.fetch("VALGRIND_EXTRA", "").split,
    ],
  )

  namespace :spec do
    desc "Run the spec suite under Valgrind memcheck (ruby_memcheck; needs the " \
         ":valgrind bundler group and the valgrind binary)"
    RubyMemcheck::RSpec::RakeTask.new(valgrind: :compile) do |t|
      # Let spec_helper skip :slow examples (fail-closed limit tests whose sheer
      # volume dominates memcheck without adding memory-safety coverage).
      ENV["VALGRIND"] = "1"

      # Optional sharding for CI wall-clock: with VALGRIND_SHARDS=N (>1) the spec
      # files are partitioned into N groups and this run does only group
      # VALGRIND_SHARD_INDEX, so N parallel matrix jobs cover the whole suite in
      # ~1/N the time WITHOUT dropping any coverage. Files are packed
      # largest-first onto the currently-lightest shard (greedy LPT by file size,
      # a proxy for runtime), so the slowest shard - which sets wall-clock - is
      # balanced rather than left to name order. Deterministic, so every shard
      # computes the same partition and takes its own index. Default N=1 = the
      # whole suite (local `rake spec:valgrind`).
      shards = Integer(ENV.fetch("VALGRIND_SHARDS", "1"))
      if shards > 1
        index   = Integer(ENV.fetch("VALGRIND_SHARD_INDEX", "0"))
        buckets = Array.new(shards) { { load: 0, files: [] } }
        # sort by (size desc, name) so the packing is fully deterministic across
        # shards regardless of Array#sort stability.
        Dir["spec/**/*_spec.rb"].sort_by { |f| [-File.size(f), f] }.each do |f|
          b = buckets.min_by { |x| x[:load] }
          b[:files] << f
          b[:load] += File.size(f)
        end
        t.pattern = buckets.fetch(index)[:files]
      end
    end
  end
rescue LoadError
  # ruby_memcheck not installed (optional :valgrind group absent) - skip the task.
end
