# frozen_string_literal: true

require_relative "../tools/pe_exports"

desc "Symbol gate: the built extension must export only Init_makiri (and ruby_abi_version)"
task symbols: :compile do
  lib = Dir["lib/makiri/makiri.{bundle,so}"].first or abort "no built extension"
  macos = RbConfig::CONFIG["target_os"] =~ /darwin/
  windows = RbConfig::CONFIG["target_os"] =~ /mingw|mswin/
  # macOS decorates C symbols with a leading underscore; Linux/Windows do not.
  u = macos ? "_" : ""

  # ONLY Init_makiri (and ruby_abi_version, which Ruby reads at require time)
  # may be EXPORTED. The hazard was Lexbor's: another Lexbor-based gem in the
  # same process binding to our differently-versioned copy and segfaulting -
  # see CLAUDE.md. No Lexbor is linked any more, and any name of ours in the
  # dynamic table is the same hazard for whatever binds it.
  #
  # (This gate also refused an UNDEFINED lxb_/lexbor_/mkr_ symbol, which macOS's
  # -undefined dynamic_lookup turns into a NULL call. Nothing so named is linked
  # or declared in the extension now, so that half checked nothing and went.)
  #
  #    This used to assert only "no lxb_ exported", which the flag-based link
  #    made true as a side effect. rustc performs the cdylib link now and passes
  #    its own export list, so the restriction is a post-link trim instead
  #    (extconf.rb) - and a check that asserted less than the claim would have
  #    passed happily while ~220 mkr_* names leaked into the dynamic table.
  #    Assert the claim itself.
  exported = if macos
               `nm -gU #{lib.shellescape}`.lines.grep(/ T /)
             elsif windows
               # nm/objdump on Windows are either the wrong tool, not on PATH,
               # or have output formats that vary by binutils version. Read the
               # PE export directory directly.
               MakiriBuild::PEExports.new(lib).names.map { |name| "#{name}\n" }
             else
               `nm -D --defined-only #{lib.shellescape}`.lines.grep(/ T /)
             end
  allowed = ["#{u}Init_makiri", "#{u}ruby_abi_version"]
  extra = exported.map { |l| l.split.last.to_s }.reject { |s| allowed.include?(s) }
  unless extra.empty?
    abort "#{lib} exports #{extra.size} symbol(s) beyond Init_makiri:\n  " \
          "#{extra.uniq.sort.first(20).join("\n  ")}\n" \
          "The export trim in ext/makiri/rust/extconf.rb did not take effect. " \
          "Do not relax this check - see the note there for the fallback."
  end
  unless exported.any? { |l| l.include?("#{u}Init_makiri") }
    abort "#{lib} does not export Init_makiri - Ruby could not load it."
  end

  puts "symbols: exports limited to #{allowed.join(' + ')} (#{lib})"
end

desc "OOM-injection gate: rebuild with MAKIRI_ALLOC_INJECT=1 and sweep every core " \
     "allocation site, verifying each failure fails closed (clean raise or " \
     "baseline-identical result, never truncated output), then make every " \
     "allocating Ruby C call raise in turn, verifying no document is left borrowed"
task :oom do
  # The hook is compiled in only under MAKIRI_ALLOC_INJECT=1 (zero overhead in
  # a normal build), so this needs its own rebuild; see
  # script/check_alloc_failures.rb for the protocol and the property gated.
  sanitize = ENV["MAKIRI_SANITIZE"].to_s
  build_env = { "MAKIRI_ALLOC_INJECT" => "1" }
  build_env["MAKIRI_SANITIZE"] = sanitize unless sanitize.empty?
  sh(build_env, "#{FileUtils::RUBY} -S rake clean compile")

  run_env = {}
  unless sanitize.empty?
    assert_sanitized!("oom", sanitize)
    run_env = {
      "ASAN_OPTIONS" => ASAN_ENV_OPTIONS,
      "MAKIRI_SANITIZE" => sanitize,
    }.merge(asan_preload_env(sanitize))
  end
  sh(run_env, "#{FileUtils::RUBY} -Ilib script/check_alloc_failures.rb")
  # The same build carries the Ruby-raise hook: every allocating C call of
  # `bridge::raw` raises in turn, and no document may be left borrowed.
  sh(run_env, "#{FileUtils::RUBY} -Ilib script/check_raise_injection.rb")
  puts "(injection build left in place; run `rake clean compile` to restore a normal build)"
end
