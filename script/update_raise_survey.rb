# frozen_string_literal: true

# Refresh the vendored copy of the C API raise survey
# (https://takahashim.github.io/ruby-c-apis/, source: github.com/takahashim/ruby-c-apis),
# which `script/check_non_raising.rb` holds the NON_RAISING table to:
#
#   ruby script/update_raise_survey.rb [URL-or-path-to-data.json]
#
# Kept small - a function's verdict over every release and config the survey
# ran, not its paths - and in the repository, so `rake dylint` needs no network.
# A change in the snapshot is a change in what the table may claim, and
# reviews as one.

require "json"
require "net/http"
require "uri"

SOURCE = ARGV[0] || "https://takahashim.github.io/ruby-c-apis/data.json"
OUT = File.expand_path("../ext/makiri/rust/lints/makiri_lints/raise_survey.json", __dir__)

raw = if SOURCE.start_with?("http")
        res = Net::HTTP.get_response(URI(SOURCE))
        abort "update_raise_survey: #{SOURCE}: #{res.code}" unless res.is_a?(Net::HTTPSuccess)
        res.body
      else
        File.read(SOURCE)
      end
data = JSON.parse(raw)
abort "update_raise_survey: unknown schema #{data['schema'].inspect}" unless data["schema"] == 1
abort "update_raise_survey: a control failed in the survey" unless data["runs"].all? { |r| r["controls_ok"] }

# overall: no_path_all (no path to a longjmp in any run), raises_any (a path in
# every run), mixed (a path in some). indirect: how calls through a function
# pointer were resolved - "any" is the most conservative, so a path can be
# a false positive and a missing one cannot be.
snapshot = {
  "survey" => {
    "source" => data["repo_url"],
    "commit" => data["commit"],
    "generated_at" => data["generated_at"],
    "releases" => data["releases"],
    "configs" => data["configs"],
    "indirect" => data["runs"].map { |r| r["indirect"] }.uniq
  },
  "functions" => data["functions"].to_h { |f| [f["name"], f["overall"]] }.sort.to_h
}
File.write(OUT, "#{JSON.pretty_generate(snapshot)}\n")
puts "update_raise_survey: #{snapshot['functions'].size} functions, " \
     "releases #{snapshot['survey']['releases'].join(', ')} -> #{OUT.sub("#{Dir.pwd}/", '')}"
