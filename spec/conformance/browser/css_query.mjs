// Run CSS selector queries in real browsers, for
// spec/conformance/css_browser_diff.rb.
//
// Reads a JSON array of documents on stdin - {mime, src, queries: [{sel, ctx,
// mode}]} - and writes {browser: [[answer, ...], ...], browser_version} on
// stdout, one answer per query in order. A document is made by DOMParser with
// its MIME type: "application/xml" for an XML document, "text/html" for an
// HTML one. (WebKit reports a DOMParser HTML document as CSS1Compat whatever
// its doctype, so the cases avoid quirks mode.)
//
// An answer is a list of element paths, or {error: name}. A path is the
// element-only child index of each element from the document down, so a
// text, comment or processing-instruction node the two parsers keep
// differently cannot shift it. Modes:
//   all     - (ctx ? document.querySelector(ctx) : document).querySelectorAll(sel)
//   matches - every element of the document for which element.matches(sel)
//
// Usage: node css_query.mjs chromium,firefox,webkit < cases.json

import { chromium, firefox, webkit } from "playwright-core";

const ENGINES = { chromium, firefox, webkit };

function runAll(docs) {
  const path = (el) => {
    const out = [];
    for (let n = el; n.parentNode && n.nodeType === 1; n = n.parentNode) {
      let i = 0;
      for (let s = n.previousElementSibling; s; s = s.previousElementSibling) i++;
      out.unshift(i);
    }
    return out;
  };
  return docs.map((doc) => {
    const d = new DOMParser().parseFromString(doc.src, doc.mime);
    return doc.queries.map(({ sel, ctx, mode }) => {
      try {
        if (mode === "matches") {
          return Array.from(d.querySelectorAll("*")).filter((e) => e.matches(sel)).map(path);
        }
        const root = ctx ? d.querySelector(ctx) : d;
        if (!root) return { error: "NoContext" };
        return Array.from(root.querySelectorAll(sel), path);
      } catch (e) {
        return { error: e.name };
      }
    });
  });
}

const names = (process.argv[2] || "chromium,firefox,webkit").split(",");
let input = "";
for await (const chunk of process.stdin) input += chunk;
const docs = JSON.parse(input);
const out = {};
for (const name of names) {
  const browser = await ENGINES[name].launch({ headless: true });
  try {
    const page = await browser.newPage();
    out[name] = await page.evaluate(runAll, docs);
    out[`${name}_version`] = browser.version();
  } finally {
    await browser.close();
  }
}
process.stdout.write(JSON.stringify(out));
