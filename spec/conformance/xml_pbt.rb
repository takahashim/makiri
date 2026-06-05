# frozen_string_literal: true
#
# Property-based testing core for the native XML reader: a generator of
# well-formed XML *by construction* (an explicit model serialized to text), a
# canonical tree dump that works on the model AND on a parsed Makiri/Nokogiri
# document, and a shrinker that minimises a failing model.
#
# It powers two harnesses:
#   * spec/xml_pbt_spec.rb        — Makiri-only properties (round-trip +
#     metamorphic), no oracle, runs in `rake spec` with shrinking.
#   * spec/conformance/xml_pbt_diff.rb — a differential property vs Nokogiri
#     (the parsed trees must be identical), runs in `rake conformance:xml_pbt`.
#
# The generator deliberately stays inside the round-trippable subset (no
# whitespace in attribute values or as standalone text, no adjacent text runs,
# no empty CDATA, only XML-1.0-legal namespace declarations), so that a parsed
# tree is expected to equal the generated model exactly — any difference is a
# real parser/modelling bug, which the shrinker then reduces to a minimal case.
module XmlPbt
  # --- model -----------------------------------------------------------------
  # ns_decls: Hash of (:default | prefix-String) => uri-String  (xmlns / xmlns:p)
  # attrs:    Array of Attr (resolved ns_uri included for the dump)
  Doc  = Struct.new(:misc_before, :root, :misc_after)
  Elem = Struct.new(:prefix, :local, :ns_uri, :ns_decls, :attrs, :children)
  Attr = Struct.new(:prefix, :local, :ns_uri, :value)
  Text = Struct.new(:value)
  CData = Struct.new(:value)
  Comment = Struct.new(:value)
  Pi = Struct.new(:target, :data)

  LOCALS   = %w[a b c item node data x y leaf wrap].freeze
  PREFIXES = %w[p q r].freeze
  URIS     = %w[urn:a urn:b urn:c].freeze
  PI_TARGETS = %w[run style proc do note].freeze
  # text/comment/PI/attr-value content: no whitespace (avoids normalization /
  # ignorable-text ambiguity), no ']' (avoids "]]>"), no '?' or '-' runs handled
  # per-context. '<' '&' '>' are LOGICAL chars the serializer escapes.
  CONTENT  = (("a".."z").to_a + ("0".."9").to_a + %w[< & > _ . ( ) : @ /]).freeze
  ATTRVAL  = (("a".."z").to_a + ("0".."9").to_a + %w[< & > _ . ( ) : @ /]).freeze

  module_function

  def pick(rng, arr)  = arr[rng.rand(arr.length)]
  def chance(rng, p)  = rng.rand < p
  def str(rng, alpha, min, max)
    (min + rng.rand(max - min + 1)).times.map { pick(rng, alpha) }.join
  end

  # --- generation ------------------------------------------------------------

  def gen_document(rng, max_depth: 4, max_children: 4)
    Doc.new(gen_misc(rng, 0, 2), gen_element(rng, max_depth, max_children, {}), gen_misc(rng, 0, 2))
  end

  def gen_misc(rng, min, max)
    (min + rng.rand(max - min + 1)).times.map { chance(rng, 0.5) ? gen_comment(rng) : gen_pi(rng) }
  end

  # scope: Hash prefix-or-nil => uri ("" for an undeclared default).
  def gen_element(rng, depth, max_children, scope)
    decls = {}
    if chance(rng, 0.35) # (re)declare the default namespace (may undeclare with "")
      decls[:default] = chance(rng, 0.2) ? "" : pick(rng, URIS)
    end
    if chance(rng, 0.35) # declare a prefix (never "" — illegal to undeclare in XML 1.0)
      decls[pick(rng, PREFIXES)] = pick(rng, URIS)
    end
    scope2 = apply_decls(scope, decls)

    bound = scope2.keys.select { |k| k && !k.empty? && scope2[k] && !scope2[k].empty? }
    prefix = (!bound.empty? && chance(rng, 0.45)) ? pick(rng, bound) : nil
    ns_uri = prefix ? scope2[prefix] : (scope2[nil] || "")
    elem = Elem.new(prefix, pick(rng, LOCALS), ns_uri, decls,
                    gen_attrs(rng, scope2, bound), nil)
    elem.children =
      depth <= 0 ? maybe_leaf_text(rng) : gen_children(rng, depth, max_children, scope2)
    elem
  end

  def gen_attrs(rng, scope, bound)
    n = rng.rand(4)
    used = []
    (0...n).map do |i|
      pfx = (!bound.empty? && chance(rng, 0.4)) ? pick(rng, bound) : nil
      local = "a#{i}" # unique per element -> no namespace duplicate-attribute WFC
      used << local
      Attr.new(pfx, local, pfx ? scope[pfx] : "", str(rng, ATTRVAL, 0, 6))
    end
  end

  def maybe_leaf_text(rng)
    chance(rng, 0.6) ? [Text.new(str(rng, CONTENT, 1, 8))] : []
  end

  def gen_children(rng, depth, max_children, scope)
    out = []
    (rng.rand(max_children + 1)).times do
      out << case rng.rand(10)
             when 0, 1, 2, 3 then gen_element(rng, depth - 1, max_children, scope)
             when 4, 5       then Text.new(str(rng, CONTENT, 1, 8))
             when 6          then CData.new(str(rng, CONTENT - %w[< & >], 1, 6))
             when 7          then gen_comment(rng)
             else                 gen_pi(rng)
             end
    end
    merge_text(out)
  end

  def gen_comment(rng)
    # no "--" and not ending in "-"
    Comment.new(str(rng, CONTENT - %w[-], 1, 8))
  end

  def gen_pi(rng)
    Pi.new(pick(rng, PI_TARGETS), str(rng, CONTENT - %w[?], 0, 6))
  end

  # The parser coalesces adjacent SAME-TYPE character data into one node and
  # drops empty text, so make the model match: merge consecutive Text->Text and
  # CData->CData (different types stay separate), drop empty Text.
  def merge_text(children)
    out = []
    children.each do |c|
      if c.is_a?(Text) && c.value.empty?
        next
      elsif (c.is_a?(Text) && out.last.is_a?(Text))
        out[-1] = Text.new(out.last.value + c.value)
      elsif (c.is_a?(CData) && out.last.is_a?(CData))
        out[-1] = CData.new(out.last.value + c.value)
      else
        out << c
      end
    end
    out
  end

  def apply_decls(scope, decls)
    s = scope.dup
    decls.each { |k, v| s[k == :default ? nil : k] = v }
    s
  end

  # --- serialization (model -> well-formed XML text) -------------------------

  # No cosmetic whitespace anywhere (decl/misc/elements are concatenated): the
  # generator owns every byte, so there is no prolog/epilog/inter-element
  # whitespace whose text-node treatment could differ between parsers.
  def serialize(doc)
    "<?xml version=\"1.0\"?>" +
      doc.misc_before.map { |m| ser_misc(m) }.join +
      ser_element(doc.root) +
      doc.misc_after.map { |m| ser_misc(m) }.join
  end

  def ser_misc(m)
    m.is_a?(Comment) ? "<!--#{m.value}-->" : "<?#{m.target}#{m.data.empty? ? '' : " #{m.data}"}?>"
  end

  def qname(prefix, local) = prefix ? "#{prefix}:#{local}" : local

  def ser_element(e)
    decls = e.ns_decls.map { |k, v| k == :default ? %( xmlns="#{v}") : %( xmlns:#{k}="#{v}") }.join
    attrs = e.attrs.map { |a| %( #{qname(a.prefix, a.local)}="#{esc_attr(a.value)}") }.join
    open = "<#{qname(e.prefix, e.local)}#{decls}#{attrs}"
    return "#{open}/>" if e.children.empty?

    "#{open}>#{e.children.map { |c| ser_child(c) }.join}</#{qname(e.prefix, e.local)}>"
  end

  def ser_child(c)
    case c
    when Elem    then ser_element(c)
    when Text    then esc_text(c.value)
    when CData   then "<![CDATA[#{c.value}]]>"
    when Comment then "<!--#{c.value}-->"
    when Pi      then ser_misc(c)
    end
  end

  def esc_text(s)  = s.gsub("&", "&amp;").gsub("<", "&lt;")
  def esc_attr(s)  = s.gsub("&", "&amp;").gsub("<", "&lt;").gsub('"', "&quot;")

  # --- canonical dump (model / Makiri / Nokogiri all map to the same string) --

  def dump_model(doc)
    "D[" + (doc.misc_before.map { |m| dm(m) } +
            [dm(doc.root)] +
            doc.misc_after.map { |m| dm(m) }).join + "]"
  end

  def dm(n)
    case n
    when Elem
      a = n.attrs.map { |x| "#{x.ns_uri}|#{x.local}=#{x.value}" }.sort.join(",")
      "E{#{n.ns_uri}|#{n.local}}(#{a})[" + n.children.map { |c| dm(c) }.join + "]"
    when Text    then "T:#{n.value};"
    when CData   then "C:#{n.value};"
    when Comment then "M:#{n.value};"
    when Pi      then "P:#{n.target}:#{n.data};"
    end
  end

  # A parsed document (Makiri or Nokogiri) -> the same canonical string. The two
  # libraries' node APIs are nearly identical; the small differences (namespace
  # nil vs "", node-type predicate) are normalised by the +adapter+.
  def dump_parsed(doc, a)
    "D[" + a.children(doc).map { |c| dp(c, a) }.join + "]"
  end

  def dp(n, a)
    case a.kind(n)
    when :element
      attrs = a.attrs(n).map { |x| "#{a.attr_ns(x)}|#{a.attr_local(x)}=#{a.attr_value(x)}" }.sort.join(",")
      "E{#{a.ns(n)}|#{a.local(n)}}(#{attrs})[" + a.children(n).map { |c| dp(c, a) }.join + "]"
    when :text    then "T:#{a.value(n)};"
    when :cdata   then "C:#{a.value(n)};"
    when :comment then "M:#{a.value(n)};"
    when :pi      then "P:#{a.pi_target(n)}:#{a.pi_data(n)};"
    else "?:#{a.kind(n)};"
    end
  end

  # --- shrinking -------------------------------------------------------------
  #
  # Greedily reduce a failing document while the predicate (true = still fails)
  # holds: drop misc/children/attrs/ns-decls, then shrink leaf values. Returns
  # the minimal still-failing document.

  def shrink(doc, &fails)
    cur = doc
    loop do
      smaller = candidates(cur).find { |c| fails.call(c) }
      break unless smaller

      cur = smaller
    end
    cur
  end

  def candidates(doc)
    out = []
    out << Doc.new([], doc.root, doc.misc_after) unless doc.misc_before.empty?
    out << Doc.new(doc.misc_before, doc.root, []) unless doc.misc_after.empty?
    elem_candidates(doc.root).each { |r| out << Doc.new(doc.misc_before, r, doc.misc_after) }
    out
  end

  def elem_candidates(e)
    out = []
    out << dup_elem(e, children: []) unless e.children.empty?
    out << dup_elem(e, attrs: []) unless e.attrs.empty?
    out << dup_elem(e, ns_decls: {}) unless e.ns_decls.empty?
    e.children.each_index do |i| # drop child i
      out << dup_elem(e, children: e.children[0...i] + e.children[(i + 1)..])
    end
    e.children.each_index do |i| # shrink child i (recurse into elements)
      child = e.children[i]
      next unless child.is_a?(Elem)

      elem_candidates(child).each do |sc|
        out << dup_elem(e, children: e.children[0...i] + [sc] + e.children[(i + 1)..])
      end
    end
    out
  end

  def dup_elem(e, **over)
    Elem.new(over.fetch(:prefix, e.prefix), over.fetch(:local, e.local),
             over.fetch(:ns_uri, e.ns_uri), over.fetch(:ns_decls, e.ns_decls),
             over.fetch(:attrs, e.attrs), over.fetch(:children, e.children))
  end

  # --- node adapters: map a parsed node to the dump interface ----------------
  # Both libraries expose a near-identical Node API; only namespace nil-vs-"",
  # the node-type test, and the xmlns-on-attribute-axis convention differ.

  # Makiri::XML nodes.
  module Makiri
    module_function
    KINDS = { 1 => :element, 3 => :text, 4 => :cdata, 7 => :pi, 8 => :comment, 9 => :document }.freeze
    def children(n)  = n.children.to_a
    def kind(n)      = KINDS[n.node_type] || :other
    def ns(n)        = n.namespace_uri.to_s
    def local(n)     = n.local_name
    def attrs(n)     = n.attribute_nodes.to_a.reject { |a| a.name == "xmlns" || a.name.start_with?("xmlns:") }
    def attr_ns(x)   = x.namespace_uri.to_s
    def attr_local(x) = x.local_name
    def attr_value(x) = x.value
    def value(n)     = n.value
    def pi_target(n) = n.name
    def pi_data(n)   = n.value
  end

  # Nokogiri nodes (only referenced when Nokogiri is loaded).
  module Nokogiri
    module_function
    def children(n)  = n.children.to_a
    def kind(n)
      return :element if n.element?
      return :cdata   if n.cdata?      # cdata? before text? (CDATA is also "text-ish")
      return :text    if n.text?
      return :comment if n.comment?
      return :pi      if n.processing_instruction?
      return :document if n.document?

      :other
    end
    def ns(n)        = n.namespace&.href.to_s
    def local(n)     = n.name.split(":").last
    def attrs(n)     = n.attribute_nodes # Nokogiri excludes xmlns declarations already
    def attr_ns(x)   = x.namespace&.href.to_s
    def attr_local(x) = x.name.split(":").last
    def attr_value(x) = x.value
    def value(n)     = n.content
    def pi_target(n) = n.name
    def pi_data(n)   = n.content
  end

  # --- properties / helpers --------------------------------------------------

  # Parse a generated model with Makiri and check parse == model (Property A).
  def roundtrips?(doc)
    parsed = ::Makiri::XML(serialize(doc))
    dump_model(doc) == dump_parsed(parsed, Makiri)
  rescue StandardError
    false
  end

  # A human-readable counterexample report for a (minimal) failing model.
  def report(doc, oracle: nil)
    xml = serialize(doc)
    lines = ["counterexample (#{xml.bytesize} bytes):", xml, "want (model):  #{dump_model(doc)}"]
    if oracle
      begin
        lines << "makiri:        #{dump_parsed(::Makiri::XML(xml), Makiri)}"
      rescue StandardError => e
        lines << "makiri:        RAISED #{e.class}: #{e.message}"
      end
      lines << "nokogiri:      #{dump_parsed(oracle.call(xml), Nokogiri)}" if oracle.respond_to?(:call)
    end
    lines.join("\n")
  end
end
