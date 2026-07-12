# frozen_string_literal: true
#
# XML mutation fuzzer / property test (target :mutate).
#
# Where the :xml target fuzzes the PARSER, this fuzzes the MUTATION surface: it
# applies a random, seeded SEQUENCE of tree edits - factories + add_child/<<,
# before/after, add_previous/next_sibling, replace, []=/delete, content=/name=,
# remove, clone_node (deep/shallow), cross-document import and in-tree moves,
# plus deliberate cycle /
# cross-representation / frozen attempts - to a fresh document, then checks the
# structural INVARIANTS every edit must preserve:
#
#   (a) no cycle         - the connected tree is walkable in bounded steps. The
#                          walk uses #child/#next one step at a time (never the C
#                          #children iterator), so a corrupt sibling ring is
#                          caught as a finding instead of hanging the walker.
#   (b) link consistency - every child's #parent is its container.
#   (c) serialization    - #to_xml terminates (the buffer cap turns any residual
#                          runaway into a raise).
#   (d) fixed point      - the canonical serialization is stable under re-parse:
#                          let a = Makiri::XML(doc.to_xml); a.to_xml must equal
#                          Makiri::XML(a.to_xml).to_xml.
#
# Note (d) is a fixed point AFTER one normalization, not raw idempotence of the
# constructed tree: the mutation API can build shapes a parser never emits -
# adjacent CDATA/text (which coalesce on parse), all-whitespace PI data (which
# normalizes), or, since the API permits it, a non-well-formed document (top-level
# text, no single element root). The first re-parse canonicalizes those benignly;
# the canonical form must then be a stable fixed point. A re-parse failure is only
# a finding when the document is STRUCTURALLY well-formed (exactly one element root
# and no top-level character data) - otherwise the fuzzer simply built a tree that
# is not a well-formed XML document, which is allowed.
#
# The oracle is SELF-CONSISTENCY, not a Nokogiri differential: Makiri resolves
# namespaces at the insertion point (with deferral for a still-detached subtree),
# which deliberately differs from libxml2, so only Makiri's own round-trip is a
# sound reference here.
#
# A run is fully determined by its integer seed, so a finding replays with:
#   ruby -Ilib -r makiri -r ./spec/fuzz/mutate -e 'p MutateFuzz.run(SEED)'

module MutateFuzz
  module_function

  # Starting points: with/without namespaces, default ns, text-only, mixed
  # comment/PI content, and a deep chain (so moves reshuffle real structure).
  SEED_DOCS = [
    %(<r xmlns:p="urn:p" xmlns="urn:d"><a/><b><c>text</c></b></r>),
    %(<root><x/><y a="1"/><z>hi</z></root>),
    %(<feed xmlns="urn:a" xmlns:dc="urn:dc"><entry dc:id="1"><title>t</title></entry></feed>),
    %(<a><b><c><d/></c></b></a>),
    %(<doc>plain text only</doc>),
    %(<m><!-- c --><?pi x?><n>y</n></m>),
  ].freeze

  # A mix of valid and invalid names/values/targets - the invalid ones must be
  # rejected (a CLEAN exception), never crash or corrupt the tree.
  ELEM_NAMES = ["x", "item", "p:item", "p:wrap", "plain", "dc:id", "café",
                "tag-name", "1bad", "a b", "ns:", ":bad", "", "a:b:c", "zzz:orphan"].freeze
  TEXTS      = ["hello", "a < b & c", "", "x\ny z", "  ", "日本語", "]]>", "--", "\x01x", "&ent;"].freeze
  ATTR_NAMES = ["id", "class", "p:role", "xmlns:q", "1bad", "data-x", "a b", "", "dc:id"].freeze
  ATTR_VALS  = ["v", %(a"b), "x<y", "", "p q", "\x00", "&amp;"].freeze
  PI_TARGETS = ["xml-stylesheet", "php", "xml", "1bad", "ok-target"].freeze
  # DOCTYPE factory inputs: valid + invalid names, and ids with a '"' / NUL that
  # must be rejected cleanly. Inserting the resulting node exercises the doctype
  # placement guards (document-only, pre-root, at most one) - most positions are
  # rejected, a before-root insert is accepted.
  DOCTYPE_NAMES = ["r", "root", "svg", "p:doc", "1bad", "a b", "", "SVG"].freeze
  DOCTYPE_IDS   = [nil, "", "-//W3C//DTD", "sys.dtd", %(a"b), "\x00", "urn:x"].freeze
  # HTML element names for the cross-representation import op: a mix of valid
  # QNames (strict, serializable) and DOM-lenient names that are not well-formed
  # XML QNames, which import_node must take verbatim as DOM-loose names.
  HTML_IMPORT_NAMES = ["div", "span", ":good:times:", "x<", "0:a", "a b", "f}oo", "xmlns:foo"].freeze

  # A documented, fail-closed rejection of a bad edit (invalid name/char, cycle,
  # second root, cross-document/representation, frozen receiver, bad index): the
  # fuzzer expects these and keeps going. Anything else is a finding.
  CLEAN = [Makiri::Error, ArgumentError, TypeError, FrozenError, RangeError, IndexError].freeze

  CycleError     = Class.new(StandardError)
  InvariantError = Class.new(StandardError)

  WALK_CAP = 200_000

  def next_seed(rng)
    rng.rand(2**62)
  end

  # Build, mutate and verify a document deterministically from +seed+.
  # Returns [category, detail] in run.rb's vocabulary.
  def run(seed)
    rng   = Random.new(seed)
    doc   = Makiri::XML(SEED_DOCS.sample(random: rng))
    other = Makiri::XML(SEED_DOCS.sample(random: rng))
    src   = collect(other) # cross-document import sources (deep-copied on insert)
    (8 + rng.rand(40)).times do
      nodes = collect(doc) # bounded; CycleError on a runaway
      begin
        apply(doc, nodes, src, rng)
      rescue *CLEAN
        # documented rejection of an invalid edit - expected, keep mutating
      end
    end
    verify(doc)
    [:ok, nil]
  rescue CycleError, InvariantError => e
    [:unexpected, "#{e.class.name.split('::').last}: #{e.message}"]
  rescue StandardError => e
    [:unexpected, "#{e.class}: #{e.message}".slice(0, 2000)]
  end

  # ---- operations ---------------------------------------------------------

  def apply(doc, nodes, src, rng)
    t = nodes.sample(random: rng)
    case rng.rand(22)
    when 0  then t.add_child(make(doc, rng))
    when 1  then t << make(doc, rng)
    when 2  then t.before(make(doc, rng))
    when 3  then t.after(make(doc, rng))
    when 4  then t.add_previous_sibling(make(doc, rng))
    when 5  then t.add_next_sibling(make(doc, rng))
    when 6  then t.replace(make(doc, rng))
    when 7  then t[ATTR_NAMES.sample(random: rng)] = ATTR_VALS.sample(random: rng)
    when 8  then t.delete(ATTR_NAMES.sample(random: rng))
    when 9  then t.content = TEXTS.sample(random: rng)
    when 10 then t.name = ELEM_NAMES.sample(random: rng)
    when 11 then t.remove
    when 12 then t.add_child(src.sample(random: rng))              # cross-document import (deep copy)
    when 13 then t.add_child(nodes.sample(random: rng))            # in-tree move (an ancestor -> cycle, rejected)
    when 14 then t.before(nodes.sample(random: rng))              # in-tree move as a sibling
    when 15 then t.replace(nodes.sample(random: rng))            # in-tree move via replace
    when 16 then t.add_child(t)                                   # self -> rejected / no-op
    when 17 then t.add_child(Makiri::HTML("<x/>").at_xpath("//x")) # cross-representation -> TypeError
    when 18 then t.freeze                                         # later edits on it -> FrozenError
    when 19 then t.add_child(doc.create_cdata(TEXTS.sample(random: rng)))
    when 20 then t.add_child(t.clone_node(rng.rand(2).zero?)) # clone (deep/shallow) reinserted as an independent copy
    when 21 then import_html(doc, rng)                        # HTML->XML import_node incl DOM-loose element names
    end
  end

  # Exercise cross-representation import_node (HTML -> XML): build a small HTML
  # subtree whose element names mix valid QNames and DOM-lenient names, and import
  # it (deep/shallow). A lenient name is taken verbatim as a non-serializable
  # DOM-loose element rather than rejected. The detached result is DISCARDED (not
  # linked into +doc+), so the serialization invariant in verify never sees a
  # loose name - the arena allocation + deep translation still run under ASan.
  def import_html(doc, rng)
    h  = Makiri::HTML("<div></div>")
    el = h.create_element(HTML_IMPORT_NAMES.sample(random: rng))
    (rng.rand(3)).times do
      child = h.create_element(HTML_IMPORT_NAMES.sample(random: rng))
      child[ATTR_NAMES.sample(random: rng)] = ATTR_VALS.sample(random: rng) if rng.rand(2).zero?
      el.add_child(child)
    end
    doc.import_node(el, rng.rand(2).zero?)
  end

  def make(doc, rng)
    case rng.rand(9)
    when 0, 1 then doc.create_element(ELEM_NAMES.sample(random: rng))
    when 2    then doc.create_element(ELEM_NAMES.sample(random: rng), TEXTS.sample(random: rng))
    when 3    then doc.create_text_node(TEXTS.sample(random: rng))
    when 4    then doc.create_comment(TEXTS.sample(random: rng))
    when 5    then doc.create_cdata(TEXTS.sample(random: rng))
    when 6    then doc.create_processing_instruction(PI_TARGETS.sample(random: rng), TEXTS.sample(random: rng))
    when 7    then make_fragment(doc, rng)  # inserting a fragment splices its children
    when 8    then doc.create_document_type(DOCTYPE_NAMES.sample(random: rng),
                                             DOCTYPE_IDS.sample(random: rng),
                                             DOCTYPE_IDS.sample(random: rng))
    end
  end

  # A fragment of 1-3 top-level items, parsed either bound to the document (names
  # resolve against its namespaces) or standalone (then imported on insert). Much
  # of the generated source is invalid (bad names, unbound prefixes, "]]>" / "--")
  # and is rejected cleanly; the valid remainder exercises the splice/import paths.
  def make_fragment(doc, rng)
    src = Array.new(1 + rng.rand(3)) { fragment_item(rng) }.join
    rng.rand(2).zero? ? doc.fragment(src) : Makiri::XML::DocumentFragment.parse(src)
  end

  def fragment_item(rng)
    name = ELEM_NAMES.sample(random: rng)
    case rng.rand(3)
    when 0 then "<#{name}/>"
    when 1 then "<#{name}>#{TEXTS.sample(random: rng)}</#{name}>"
    else        TEXTS.sample(random: rng)  # bare top-level character data
    end
  end

  # ---- invariants ---------------------------------------------------------

  # Depth-first node list of the connected tree, walked one step at a time
  # (#child + #next) so a corrupt sibling ring trips the bound instead of hanging
  # inside the C #children iterator. Raises CycleError past WALK_CAP.
  def collect(doc)
    out   = []
    stack = [doc]
    until stack.empty?
      n = stack.pop
      out << n
      raise CycleError, "node count exceeded #{WALK_CAP}" if out.size > WALK_CAP

      c = child_of(n)
      steps = 0
      while c
        stack.push(c)
        steps += 1
        raise CycleError, "sibling chain exceeded #{WALK_CAP}" if steps > WALK_CAP

        c = c.next
      end
    end
    out
  end

  def child_of(node)
    node.child
  rescue NoMethodError
    nil # leaf representations may not expose #child
  end

  def verify(doc)
    # (a) the connected tree is finite + walkable
    nodes = collect(doc)

    # (b) every child points back to its container
    nodes.each do |n|
      c = child_of(n)
      while c
        raise InvariantError, "child.parent mismatch under <#{(n.name rescue '?')}>" unless c.parent == n

        c = c.next
      end
    end

    # (c) serialization terminates
    xml1 = doc.to_xml

    # The constructed tree may not be a well-formed XML document (the API allows
    # top-level character data / no single root); then its serialization need not
    # re-parse, and that is not a finding.
    well_formed = structurally_well_formed?(doc)
    begin
      canonical = Makiri::XML(xml1).to_xml
    rescue Makiri::Error => e
      raise InvariantError, "well-formed document serialized to unparseable XML: #{e.message}" if well_formed

      return # not well-formed by construction - nothing further to check
    end

    # (d) the canonical serialization is a stable fixed point under re-parse.
    raise InvariantError, "serialization not a fixed point" unless canonical == Makiri::XML(canonical).to_xml
  rescue Makiri::Error => e
    raise InvariantError, "serialize/re-parse failed: #{e.class}: #{e.message}"
  end

  # A document is structurally well-formed when exactly one of its top-level
  # children is an element and none is character data (comments / PIs are allowed).
  def structurally_well_formed?(doc)
    elements = 0
    c = child_of(doc)
    while c
      return false if c.is_a?(Makiri::XML::Text) || c.is_a?(Makiri::XML::CDATASection)

      elements += 1 if c.is_a?(Makiri::XML::Element)
      c = c.next
    end
    elements == 1
  end
end
