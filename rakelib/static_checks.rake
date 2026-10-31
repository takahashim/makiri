# frozen_string_literal: true

namespace :unsafe do
  desc "Check each file's unsafe count, the forbid files and static mut against the recorded tables"
  task :boundaries do
    sh FileUtils::RUBY, "script/check_unsafe_boundaries.rb"
  end

  # Transcribes the unsafe-island counts and the forbid list into the script,
  # so a hand count cannot disagree with the one the check does. It does not
  # touch `static mut`'s table, which pins a boundary decision - that still
  # fails until a person raises it.
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
