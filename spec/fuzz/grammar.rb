# frozen_string_literal: true
#
# XPath grammar-aware generator. Produces queries closer to the real XPath
# surface than pure-random byte strings would, while still covering enough of
# the grammar (axes, name tests, predicates, functions, unions, operators) to
# exercise the evaluator.
#
# Mutation: simple byte-level edits over an existing query, to stir the corpus
# and produce malformed input the parser must reject gracefully.

module Grammar
  AXES = %w[
    child:: descendant:: descendant-or-self::
    parent:: ancestor:: ancestor-or-self::
    following:: preceding::
    following-sibling:: preceding-sibling::
    self:: attribute::
  ].freeze

  NAME_TESTS = %w[
    div span a p ul li h1 h2 h3
    section article header footer main
    img input br
    svg circle rect path
    body html head title
    * text() node() comment() processing-instruction()
  ].freeze

  ATTRS    = %w[id class href src type name lang data-id data-rank data-ved].freeze
  STRINGS  = %w["x" "main" "item" "hidden" "first" "second" "g" "" "abc"].freeze
  NUMBERS  = %w[0 1 2 3 -1 0.5 1.5 last() position() position()-1 last()-1].freeze
  FN_BOOL  = %w[true() false()].freeze
  FN_NUM_0 = %w[last() position()].freeze
  FN_NUM_1 = %w[count string-length number floor ceiling round sum].freeze
  FN_STR_0 = %w[name() local-name() namespace-uri()].freeze
  FN_STR_1 = %w[string normalize-space name local-name].freeze
  OPS      = %w[+ - * div mod = != < <= > >=].freeze
  BOOL_OPS = %w[and or].freeze

  module_function

  def gen_xpath(depth, rng)
    if rng.rand(20) == 0
      gen_expr(depth, rng)
    else
      gen_path(depth, rng)
    end
  end

  def gen_path(d, rng)
    prefix = case rng.rand(4)
             when 0 then "//"
             when 1 then "/"
             when 2 then ".//"
             else        ""
             end
    n_steps = 1 + rng.rand([d, 1].max)
    steps = (1..n_steps).map { gen_step(d - 1, rng) }
    expr = prefix + steps.join("/")

    expr += " | #{gen_path(d - 1, rng)}" if d > 1 && rng.rand(8) == 0
    expr
  end

  def gen_step(d, rng)
    axis = rng.rand(3) == 0 ? AXES.sample(random: rng) : ""
    test = if axis == "attribute::"
             rng.rand(2) == 0 ? "*" : ATTRS.sample(random: rng)
           else
             NAME_TESTS.sample(random: rng)
           end
    n_preds = d > 0 ? rng.rand(3) : 0
    preds = (1..n_preds).map { gen_predicate(d - 1, rng) }.join
    "#{axis}#{test}#{preds}"
  end

  def gen_predicate(d, rng)
    case rng.rand(11)
    when 0 then "[@#{ATTRS.sample(random: rng)}]"
    when 1 then "[@#{ATTRS.sample(random: rng)}=#{STRINGS.sample(random: rng)}]"
    when 2 then "[#{NUMBERS.sample(random: rng)}]"
    when 3 then "[#{FN_BOOL.sample(random: rng)}]"
    when 4 then "[#{FN_NUM_1.sample(random: rng)}(.) #{OPS.sample(random: rng)} #{NUMBERS.sample(random: rng)}]"
    when 5 then "[contains(@#{ATTRS.sample(random: rng)}, #{STRINGS.sample(random: rng)})]"
    when 6 then "[starts-with(@#{ATTRS.sample(random: rng)}, #{STRINGS.sample(random: rng)})]"
    when 7 then "[not(@#{ATTRS.sample(random: rng)})]"
    when 8 then "[@#{ATTRS.sample(random: rng)} #{BOOL_OPS.sample(random: rng)} @#{ATTRS.sample(random: rng)}]"
    when 9
      d > 0 ? "[#{gen_path(d, rng)}]" : "[@#{ATTRS.sample(random: rng)}]"
    else "[#{FN_NUM_0.sample(random: rng)} #{OPS.sample(random: rng)} #{NUMBERS.sample(random: rng)}]"
    end
  end

  def gen_expr(d, rng)
    case rng.rand(8)
    when 0 then "#{NUMBERS.sample(random: rng)} #{OPS.sample(random: rng)} #{NUMBERS.sample(random: rng)}"
    when 1 then "concat(#{STRINGS.sample(random: rng)}, #{STRINGS.sample(random: rng)})"
    when 2 then "string-length(#{STRINGS.sample(random: rng)})"
    when 3 then "contains(#{STRINGS.sample(random: rng)}, #{STRINGS.sample(random: rng)})"
    when 4 then "count(#{gen_path([d - 1, 1].max, rng)})"
    when 5 then "not(#{STRINGS.sample(random: rng)})"
    when 6 then "number(#{STRINGS.sample(random: rng)})"
    else gen_path(d, rng)
    end
  end

  # Byte-level mutation: delete, insert, substitute, or duplicate a small
  # range. The output is not guaranteed to be valid - that's the point; the
  # parser must handle garbage gracefully (fail closed, never crash).
  def mutate(s, rng)
    return "" if s.nil?
    return gen_xpath(3, rng) if s.empty?

    out = s.b.dup
    n_edits = 1 + rng.rand(3)
    n_edits.times do
      next if out.empty?

      case rng.rand(5)
      when 0
        out[rng.rand(out.bytesize)] = ""
      when 1
        out.insert(rng.rand(out.bytesize + 1), rng.rand(0x20..0x7e).chr)
      when 2
        out[rng.rand(out.bytesize)] = rng.rand(0x20..0x7e).chr
      when 3
        i = rng.rand(out.bytesize)
        out.insert(i, out[i, rng.rand(1..5)] || "")
      when 4
        out.insert(rng.rand(out.bytesize + 1), %w([ ] ( ) / @ * | = " ').sample(random: rng))
      end
    end
    out.force_encoding("UTF-8")
    out.valid_encoding? ? out : s
  end
end
