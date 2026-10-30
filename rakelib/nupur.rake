# frozen_string_literal: true

# nupur checked on its own: the test extension beside it
# (ext/makiri/rust/nupur/test/ext) - a few methods over the layer and nothing
# of Makiri's - built into tmp/nupur_test, and the specs beside that run
# against it. What Makiri's own suite checks through the extension, this
# checks through the layer alone, for a copy of the layer elsewhere.
NUPUR_TEST_SRC = File.expand_path("ext/makiri/rust/nupur/test", ROOT)
NUPUR_TEST_BUILD = File.expand_path("tmp/nupur_test", ROOT)

namespace :nupur do
  desc "Build nupur's test extension (tmp/nupur_test)"
  task :build do
    FileUtils.mkdir_p(NUPUR_TEST_BUILD)
    Dir.chdir(NUPUR_TEST_BUILD) do
      sh FileUtils::RUBY, File.join(NUPUR_TEST_SRC, "ext", "extconf.rb") unless File.exist?("Makefile")
      sh "make"
    end
  end

  desc "Run nupur's own specs against its test extension"
  task spec: :build do
    sh FileUtils::RUBY, "-S", "rspec", "-I", NUPUR_TEST_BUILD, File.join(NUPUR_TEST_SRC, "spec")
  end
end
