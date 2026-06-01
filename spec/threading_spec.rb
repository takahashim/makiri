# frozen_string_literal: true

# Concurrency safety. Parsing and (handler-free) XPath evaluation release the
# GVL so multiple threads run the pure-C engine at once. These specs hammer
# that path under GC.stress to flush use-after-move / unlocked-region bugs:
# each thread owns its own Document (no shared mutable state), and we assert the
# results are exactly what the single-threaded path produces.

# Tagged :threading — skipped by default for a fast local run, REQUIRED in CI.
# See spec/spec_helper.rb (THREADING=1 / CI forces it on).
RSpec.describe "concurrency", :threading do
  HTML = (<<~HTML).freeze
    <!doctype html><html><body>
      <main id="main">
        <ul>#{(1..200).map { |i| %(<li class="item" data-id="#{i}">row #{i}</li>) }.join}</ul>
      </main>
    </body></html>
  HTML

  # ASan on macOS mishandles munmap of Ruby's thread stacks (spurious "Failed to
  # munmap" aborts under DYLD_INSERT_LIBRARIES with an uninstrumented Ruby), and
  # it can't detect data races anyway (that's TSan's job). The concurrent parse
  # path is the same C as the single-threaded path, which every other spec
  # already runs under ASan thousands of times — so skip these under the
  # sanitizer build; they carry no extra ASan signal.
  SANITIZING = !ENV["ASAN_OPTIONS"].to_s.empty?
  NTHREADS = 8
  ITERS    = 50

  before { skip "ASan+macOS mishandles Ruby thread-stack munmap" if SANITIZING }

  around do |example|
    GC.stress = true
    example.run
  ensure
    GC.stress = false
  end

  it "parses concurrently with correct, independent results" do
    threads = Array.new(NTHREADS) do
      Thread.new do
        ITERS.times.map do
          doc = Makiri::HTML(HTML)
          doc.css("li").length
        end
      end
    end
    results = threads.flat_map(&:value)
    expect(results).to all(eq(200))
  end

  it "evaluates XPath concurrently, each thread on its own document" do
    threads = Array.new(NTHREADS) do
      Thread.new do
        doc = Makiri::HTML(HTML)
        ITERS.times.map do
          doc.xpath("//li").length +
            doc.xpath("//li[@data-id='100']").length +
            doc.at_xpath("//*[@id='main']").name.length
        end
      end
    end
    # 200 (//li) + 1 (data-id=100) + 4 ("main") = 205, every iteration.
    results = threads.flat_map(&:value)
    expect(results).to all(eq(205))
  end

  it "mixes parse, query, and serialize across threads without crashing" do
    threads = Array.new(NTHREADS) do |t|
      Thread.new do
        doc = Makiri::HTML(HTML)
        ITERS.times do
          case t % 3
          when 0 then doc.css("li.item").map(&:text)
          when 1 then doc.xpath("//li").length
          else doc.to_html
          end
        end
        :ok
      end
    end
    expect(threads.map(&:value)).to all(eq(:ok))
  end
end
