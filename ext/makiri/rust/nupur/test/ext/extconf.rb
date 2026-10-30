# frozen_string_literal: true

# nupur's test extension (`rake nupur:spec`): the crate beside this file,
# built as `nupur_test.bundle` / `.so`.
require "mkmf"
require "rb_sys/mkmf"

create_rust_makefile("nupur_test") do |r|
  r.extra_cargo_args = ["--crate-type", "cdylib"]
  r.profile = :dev
end
