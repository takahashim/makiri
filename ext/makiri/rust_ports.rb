# frozen_string_literal: true

# Which C files the Rust port replaces, and what turns each replacement on.
#
# One table, required by both sides that need it - `ext/makiri/extconf.rb` (which
# decides what to compile) and the `Rakefile` (which tells CI and the container
# scripts what the full configuration is). It briefly lived only in extconf, with
# a second script reading it back out by regular expression; that meant four
# separate parses of one table's formatting, and a rubocop-shaped change to the
# layout would have broken one of them silently. A file both sides `require` has
# no formatting to agree about.
#
# This ships in the gem: extconf sits beside it and the gemspec takes
# `ext/makiri/*`.
module RustPorts
  # env:     the MAKIRI_RUST_* flag that enables the replacement
  # feature: the cargo feature it turns on
  # srcs:    the C sources it replaces, relative to ext/makiri
  #          (`:xml_dir` means the whole xml/ directory)
  ALL = [
  { env: "MAKIRI_RUST_XPATH_XML",           feature: "xpath-xml",
    srcs: %w[xpath/mkr_xpath_engine_xml.c] },
  { env: "MAKIRI_RUST_XPATH_HTML",          feature: "xpath-html",
    srcs: %w[xpath/mkr_xpath_engine_html.c] },
  { env: "MAKIRI_RUST_XPATH_DRIVER",        feature: "xpath-driver",
    srcs: %w[xpath/mkr_xpath.c] },
  { env: "MAKIRI_RUST_XPATH_SHARED",        feature: "xpath-shared",
    srcs: %w[xpath/mkr_xpath_shared.c] },
  { env: "MAKIRI_RUST_GLUE_SERIALIZE",      feature: "glue-serialize",
    srcs: %w[glue/ruby_html_serialize.c] },
  { env: "MAKIRI_RUST_GLUE_NODE",           feature: "glue-node",
    srcs: %w[glue/ruby_node.c] },
  { env: "MAKIRI_RUST_BRIDGE_STRING",       feature: "bridge-string",
    srcs: %w[bridge/ruby_string.c] },
  { env: "MAKIRI_RUST_GLUE_NODE_SET",       feature: "glue-node-set",
    srcs: %w[glue/ruby_node_set.c] },
  { env: "MAKIRI_RUST_GLUE_CSS",            feature: "glue-css",
    srcs: %w[glue/ruby_html_css.c] },
  { env: "MAKIRI_RUST_GLUE_LEXBOR_CSS",     feature: "glue-lexbor-css",
    srcs: %w[glue/ruby_lexbor_css.c] },
  { env: "MAKIRI_RUST_GLUE_DOC",            feature: "glue-doc",
    srcs: %w[glue/ruby_doc.c] },
  { env: "MAKIRI_RUST_BRIDGE_XML_DECODE",   feature: "bridge-xml-decode",
    srcs: %w[bridge/xml_decode.c] },
  { env: "MAKIRI_RUST_GLUE_XML",            feature: "glue-xml",
    srcs: %w[glue/ruby_xml.c] },
  { env: "MAKIRI_RUST_GLUE_XPATH",          feature: "glue-xpath",
    srcs: %w[glue/ruby_xpath.c] },
  { env: "MAKIRI_RUST_GLUE_XML_NODE_READ",  feature: "glue-xml-node-read",
    srcs: %w[glue/ruby_xml_node_read.c] },
  { env: "MAKIRI_RUST_GLUE_XML_NODE_MUTATE", feature: "glue-xml-node-mutate",
    srcs: %w[glue/ruby_xml_node.c] },
  { env: "MAKIRI_RUST_GLUE_XML_NODE_SERIALIZE", feature: "glue-xml-node-serialize",
    srcs: %w[glue/ruby_xml_node_serialize.c] },
  { env: "MAKIRI_RUST_GLUE_HTML_NODE",      feature: "glue-html-node",
    srcs: %w[glue/ruby_html_node.c] },
  { env: "MAKIRI_RUST_GLUE_HTML_MUTATE",    feature: "glue-html-mutate",
    srcs: %w[glue/ruby_html_mutate.c] },
  # The XPath FRONT END is one feature over three files, and it is also implied
  # by every xpath-* row above (see rust_xpath below).
  { env: "MAKIRI_RUST_XPATH",               feature: "xpath",
    srcs: %w[xpath/mkr_xpath_lex.c xpath/mkr_xpath_number.c xpath/mkr_xpath_parse.c] },
  # The XML reader replaces a whole directory rather than named files.
  { env: "MAKIRI_RUST_XML",                 feature: "xml", srcs: :xml_dir },
].freeze

  # Flags that imply other flags. These are about how the ports RELATE, which is
  # why they are not a column in the table:
  #   - any xpath-* instance needs the front end;
  #   - the XML node readers come with its mutators/serializers/queries (they
  #     share a module);
  #   - the XML decode shares ruby_string.c's strict-text core, so the Rust one
  #     comes with it - otherwise both languages would define mkr_text_check.
  IMPLIES = {
    "MAKIRI_RUST_XPATH" => ->(on) { on.any? { |e| e.start_with?("MAKIRI_RUST_XPATH_") } },
    "MAKIRI_RUST_GLUE_XML_NODE_READ" => lambda { |on|
      on.any? do |e|
        %w[MAKIRI_RUST_GLUE_XML_NODE_MUTATE MAKIRI_RUST_GLUE_XML_NODE_SERIALIZE
           MAKIRI_RUST_GLUE_XPATH].include?(e)
      end
    },
    "MAKIRI_RUST_BRIDGE_STRING" => ->(on) { on.include?("MAKIRI_RUST_BRIDGE_XML_DECODE") },
    # The HTML mutators share the readers' wrap/unwrap and node-type constants.
    "MAKIRI_RUST_GLUE_HTML_NODE" => ->(on) { on.include?("MAKIRI_RUST_GLUE_HTML_MUTATE") },
  }.freeze

  class << self
    # The flags this environment turns on, implications applied.
    #
    # `MAKIRI_RUST=all` means every row. It exists so that nothing outside this
    # file has to enumerate the set: CI and the container scripts used to carry
    # the whole list as a string, which is how two of them ended up two flags
    # behind and the nightly gates ended up running four of nineteen.
    def enabled(env = ENV)
      return ALL.map { |r| r[:env] } if env["MAKIRI_RUST"].to_s.strip == "all"

      on = ALL.map { |r| r[:env] }.select { |e| env[e].to_s.strip == "1" }
      IMPLIES.each { |flag, pred| on |= [flag] if pred.call(on) }
      on
    end

    def features(on = enabled)
      ALL.select { |r| on.include?(r[:env]) }.map { |r| r[:feature] }
    end

    # The C sources those flags replace, relative to ext/makiri.
    def replaced_sources(on = enabled)
      ALL.select { |r| on.include?(r[:env]) }
         .flat_map { |r| r[:srcs] == :xml_dir ? ["xml/"] : r[:srcs] }
    end

    # Every path in the table must name a file that exists.
    #
    # A typo does NOT fail on its own: the file is simply not dropped, so the C
    # and the Rust both define the symbol, `-fvisibility=hidden` makes both
    # local, the linker keeps one, and the build is green with the Rust half
    # dead. Verified - `glue/ruby_doc_TYPO.c` compiled, linked and passed 992
    # examples while the port it was meant to enable did nothing.
    def check_paths!(ext_dir)
      ALL.each do |row|
        next if row[:srcs] == :xml_dir

        row[:srcs].each do |rel|
          next if File.exist?(File.join(ext_dir, rel))

          raise "RustPorts: row #{row[:env]} names #{rel}, which does not exist. " \
                "A wrong path here does not fail the build - it silently leaves " \
                "the C file in and the Rust replacement unused."
        end
      end
    end
  end
end
