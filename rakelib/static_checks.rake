# frozen_string_literal: true

namespace :unsafe do
  desc "Check the Rust safe-module and reviewed-global boundaries"
  task :boundaries do
    sh FileUtils::RUBY, "script/check_unsafe_boundaries.rb"
  end

  # Transcribes the unsafe-island counts and the forbid list into the script,
  # so a hand count cannot disagree with the one the check does. It does not
  # touch the tables that pin a boundary decision (`rb_sys::`, `static mut`,
  # the raising C API) - those still fail until a person raises them.
  desc "Rewrite the recorded unsafe islands and forbid list from what is there"
  task :fix do
    sh FileUtils::RUBY, "script/check_unsafe_boundaries.rb", "--fix"
  end
end

# The extension's dependencies against ext/makiri/rust/deny.toml: RustSec
# advisories and yanked releases, licenses, sources (CI: security.yml, nightly
# too). Needs cargo-deny (`cargo install cargo-deny --locked`).
desc "Check the extension's dependencies with cargo-deny (advisories, licenses, sources)"
task :deny do
  Dir.chdir("ext/makiri/rust") { sh "cargo", "deny", "check" }
end
