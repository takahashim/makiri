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

  it "is safe to mix XPath and mutation on a SHARED document" do
    # XPath evaluation holds the GVL, and so does every mutation, so a walk can
    # never run in parallel with a mutation that detaches/moves nodes on the same
    # document (which would otherwise be a use-after-move). Assert the mix runs to
    # completion with no crash and no stray exception.
    doc = Makiri::HTML(HTML)
    ul  = doc.at_css("ul")
    errors = Queue.new
    threads = []
    (NTHREADS - 2).times do
      threads << Thread.new do
        ITERS.times do
          doc.xpath("//li[@class='item']").length
          doc.at_xpath("//main")&.name
        rescue => e
          errors << e
        end
      end
    end
    2.times do |t|
      threads << Thread.new do
        ITERS.times do |i|
          el = doc.create_element("li")
          el["data-id"] = "x#{t}-#{i}"
          ul.add_child(el)
          ul.children.first&.remove
        rescue => e
          errors << e
        end
      end
    end
    threads.each(&:join)
    expect(errors).to be_empty
  end

  it "is safe to share one XPathContext across threads (no corruption)" do
    # A context's per-evaluate caches / AST memo are not concurrency-safe, but
    # #evaluate holds the GVL, so concurrent evaluate on a shared context is
    # serialised and cannot corrupt memory (previously segfaulted via ms_sort
    # when evaluation released the GVL).
    doc = Makiri::HTML(HTML)
    ctx = Makiri::XPathContext.new(doc)
    want = ctx.evaluate("//li[@class='item']").length
    threads = Array.new(NTHREADS) do
      Thread.new do
        ITERS.times.all? { ctx.evaluate("//li[@class='item']").length == want }
      end
    end
    expect(threads.map(&:value)).to all(be(true))
  end
end
