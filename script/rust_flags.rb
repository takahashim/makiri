# frozen_string_literal: true

# The "everything" Rust-port configuration, DERIVED rather than written down.
#
# The set of MAKIRI_RUST_* flags lived as five separate string literals - in
# two CI workflows and three container scripts - and had already drifted: the
# sanitizer containers were two flags behind and the nightly security job was
# fifteen behind, so the gates that exist to cover the port were running against
# a build that was mostly still C. A list maintained by hand is a list that goes
# stale the next time a file is ported.
#
# So nothing here is a list. `extconf.rb`'s RUST_PORTS table is the only place
# that decides which flags exist and which cargo feature each turns on, and this
# reads that table back out of it. Adding a flag to extconf adds it here, to CI, and to
# the container scripts, at once - which is the only arrangement that survives
# the remaining ports.
module RustFlags
  EXTCONF = File.expand_path("../ext/makiri/extconf.rb", __dir__)

  class << self
    # Every MAKIRI_RUST_* flag extconf reads, in declaration order.
    def flags
      @flags ||= begin
        found = source.scan(/env: "(MAKIRI_RUST_[A-Z_0-9]+)"/).flatten.uniq
        if found.empty?
          raise "RustFlags: found no MAKIRI_RUST_* flags in #{EXTCONF} - the " \
                "parse is broken, and a silently empty set would make every " \
                "gate below run against the plain C build"
        end
        found
      end
    end

    # Every cargo feature those flags enable. `alloc-inject` and `lexbor-abi`
    # are not here: neither is a port flag (the first is gated by
    # MAKIRI_ALLOC_INJECT, the second is implied by the features that need it).
    def features
      @features ||= begin
        found = source.scan(/feature: "([a-z0-9-]+)"/).flatten.uniq
        raise "RustFlags: found no cargo features in #{EXTCONF}" if found.empty?

        found
      end
    end

    # The C sources the enabled flags replace, relative to ext/makiri. Used by
    # `rake verify` to work out which CBMC proofs this configuration orphans.
    def replaced_sources
      rows = source.scan(/env: "(MAKIRI_RUST_[A-Z_0-9]+)",\s*\n?\s*feature: "[a-z0-9-]+",\s*\n?\s*srcs: (%w\[[^\]]*\]|:xml_dir)/m)
      on = ENV.keys.select { |k| k.start_with?("MAKIRI_RUST_") && ENV[k].to_s.strip == "1" }
      rows.filter_map do |env, srcs|
        next unless on.include?(env)

        srcs == ":xml_dir" ? "xml/" : srcs[3..-2].split
      end.flatten
    end

    # `KEY=1 KEY=1 ...`, for `env $(...)` and for `eval "$(...)"` with `export`.
    def env_assignments
      flags.map { |f| "#{f}=1" }.join(" ")
    end

    def export_line
      "export #{env_assignments}"
    end

    def features_csv
      features.join(",")
    end

    # For a Ruby caller that wants to pass them to a subprocess.
    def env_hash
      flags.to_h { |f| [f, "1"] }
    end

    private

    def source
      @source ||= File.read(EXTCONF)
    end
  end
end
