# frozen_string_literal: true

# The Ruby API surface of Makiri, as a sorted text manifest.
#
# `spec/api_surface_spec.rb` compares this against the recorded
# `spec/api_surface.txt`, so a method that disappears, gains an argument or
# changes its superclass fails the suite instead of being noticed by a user.
#
# It exists because the rest of the suite only catches what it calls. A method
# no example exercises can be dropped in a refactor and stay green - which is
# exactly the class of regression a port makes likely. This manifest is the
# check that does not depend on anyone having written an example.
#
# What it records, and why each:
#
#   * every module and class under `Makiri`, with a class's superclass and the
#     modules mixed directly into it - the leaf classes are what `#css` and
#     `#xpath` hand back, so their identity is part of the contract;
#   * alias constants (`Makiri::CDATA`), which are API and have no methods of
#     their own to notice their absence;
#   * methods DEFINED ON each module, public and private, with arity - private
#     because `lib/` calls the extension's `_css` / `_at_css` and a rename there
#     is a silent break;
#   * the parameter shape for methods written in Ruby, which is where keyword
#     arguments live (the extension's arity is all a C-defined method reports).
#
# Regenerate with `bundle exec rake api:record`, and review the diff: a change
# here is an API change, whether or not it was meant as one.

module ApiManifest
  FIXTURE = File.expand_path("../spec/api_surface.txt", __dir__)

  HEADER = <<~TEXT
    # Makiri's Ruby API surface, recorded. Regenerate: bundle exec rake api:record
    # A diff here is an API change - review it as one, do not just re-record.
  TEXT

  class << self
    def generate
      HEADER + namespace.map { |name, mod| block(name, mod) }.join("\n")
    end

    # Every module and class whose canonical name is under `Makiri`, sorted.
    # The `name ==` test is what keeps an alias constant from being reported a
    # second time as its target; aliases are recorded in their owner's block.
    def namespace
      found = { "Makiri" => Makiri }
      walk(Makiri, "Makiri", found)
      found.sort_by(&:first)
    end

    private

    def walk(mod, path, found)
      mod.constants(false).sort.each do |c|
        value = begin
          mod.const_get(c)
        rescue StandardError, ScriptError
          next
        end
        next unless value.is_a?(Module)

        name = "#{path}::#{c}"
        next unless value.name == name
        next if found.key?(name)

        found[name] = value
        walk(value, name, found)
      end
    end

    def block(name, mod)
      lines = [mod.is_a?(Class) ? "class #{name} < #{mod.superclass}" : "module #{name}"]
      lines.concat(mixins(mod).map { |m| "  include #{m}" })
      lines.concat(constants(mod))
      lines.concat(methods_of(mod))
      "#{lines.join("\n")}\n"
    end

    # The modules mixed directly into `mod`: its own ancestors down to (but not
    # including) whatever it inherits from.
    def mixins(mod)
      stop = mod.is_a?(Class) ? mod.superclass : nil
      mod.ancestors.take_while { |a| a != stop }.reject { |a| a == mod }.map(&:to_s).sort
    end

    # Non-module constants, plus the aliases: a constant naming a module whose
    # canonical name is elsewhere (`Makiri::CDATA` -> `Makiri::CDATASection`).
    def constants(mod)
      mod.constants(false).sort.filter_map do |c|
        value = begin
          mod.const_get(c)
        rescue StandardError, ScriptError
          next "  const #{c} = <unreadable>"
        end
        if value.is_a?(Module)
          next if value.name == "#{mod}::#{c}"

          "  const #{c} = #{value}"
        else
          "  const #{c} : #{value.class}"
        end
      end
    end

    def methods_of(mod)
      out = []
      singleton = mod.singleton_class
      out.concat(entries(singleton, :public_instance_methods, "  def self.", mod, true))
      out.concat(entries(singleton, :private_instance_methods, "  private def self.", mod, true))
      out.concat(entries(mod, :public_instance_methods, "  def ", mod, false))
      out.concat(entries(mod, :private_instance_methods, "  private def ", mod, false))
      out
    end

    def entries(owner, kind, prefix, mod, on_singleton)
      owner.send(kind, false).sort.filter_map do |name|
        um = begin
          owner.instance_method(name)
        rescue NameError
          next
        end
        # A singleton class inherits Class's own methods; `false` already drops
        # those, but Ruby also reports the attached object's `allocate` and
        # friends on some builds. Keep only what this object actually defines.
        next if on_singleton && um.owner != mod.singleton_class

        "#{prefix}#{name}(#{shape(um)})"
      end
    end

    # Arity always; the parameter list too when the method is written in Ruby,
    # which is the only place a keyword argument is visible.
    def shape(um)
      return um.arity.to_s if um.source_location.nil?

      params = um.parameters.map { |kind, pname| pname ? "#{kind}:#{pname}" : kind.to_s }
      params.empty? ? um.arity.to_s : "#{um.arity} #{params.join(",")}"
    end
  end
end

if $PROGRAM_NAME == __FILE__
  require "makiri"
  if ARGV.include?("--record")
    File.write(ApiManifest::FIXTURE, ApiManifest.generate)
    warn "api: recorded #{ApiManifest::FIXTURE}"
  else
    print ApiManifest.generate
  end
end
