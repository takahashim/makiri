// Parse HTML cases in real browsers and print each resulting DOM as a tree.
//
// Reads a JSON array of cases on stdin - {kind: "document" | "fragment",
// data, context} - and writes {browser: [tree, ...]} on stdout, one tree per
// case in order. Trees have the shape spec/conformance/browser_diff.rb builds
// from Makiri, so the two compare with ==.
//
// Every parse runs with scripting DISABLED, as Makiri parses. A document is
// loaded the way a browser loads one - navigated to and served as
// text/html;charset=utf-8 from a same-origin URL that Playwright answers - into
// an iframe sandboxed with allow-same-origin and without allow-scripts, so
// scripting is off and the page can read the result. (A blob URL crashed
// Chromium's renderer in a sandboxed frame.) (DOMParser was the first
// choice and is wrong for this: WebKit reports a DOMParser document as
// CSS1Compat whatever its doctype.) A fragment goes through innerHTML on a
// context element of a DOMParser document: the fragment parsing algorithm,
// the context's document having no browsing context. Quirks mode is
// document.compatMode, which does not tell limited-quirks from no-quirks.
//
// Usage: node dump.mjs chromium,firefox,webkit < cases.json

import { chromium, firefox, webkit } from "playwright-core";

const ENGINES = { chromium, firefox, webkit };
const ORIGIN = "http://makiri-parser.test";

function parseAll(cases) {
  const HTML = "http://www.w3.org/1999/xhtml";
  const NS = { svg: "http://www.w3.org/2000/svg", math: "http://www.w3.org/1998/Math/MathML" };
  const node = (n) => {
    switch (n.nodeType) {
      case 1: {
        const attrs = Array.from(n.attributes, (a) => [a.namespaceURI, a.prefix, a.localName, a.value]);
        const isTemplate = n.namespaceURI === HTML && n.localName === "template";
        return ["el", n.namespaceURI, n.prefix, n.localName, attrs,
          isTemplate ? [] : Array.from(n.childNodes, node),
          isTemplate ? Array.from(n.content.childNodes, node) : null];
      }
      case 3: return ["text", n.data];
      case 4: return ["text", n.data];
      case 7: return ["pi", n.target, n.data];
      case 8: return ["comment", n.data];
      case 10: return ["doctype", n.name, n.publicId, n.systemId];
      default: return ["other", n.nodeType];
    }
  };
  const load = (index) => new Promise((resolve, reject) => {
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "allow-same-origin");
    frame.onload = () => {
      try {
        const d = frame.contentDocument;
        resolve({ mode: d.compatMode, children: Array.from(d.childNodes, node) });
      } catch (e) {
        reject(e);
      } finally {
        frame.remove();
      }
    };
    frame.src = `/case/${index}`;
    document.body.appendChild(frame);
  });
  const one = async (c) => {
    try {
      if (c.kind === "document") {
        return await load(c.index);
      }
      const host = new DOMParser().parseFromString("<!DOCTYPE html>", "text/html");
      const [ns, local] = c.context.includes(" ") ? c.context.split(" ", 2) : [null, c.context];
      const el = ns ? host.createElementNS(NS[ns], local) : host.createElement(local);
      el.innerHTML = c.data;
      const kids = el.namespaceURI === HTML && el.localName === "template" ? el.content.childNodes : el.childNodes;
      return { mode: null, children: Array.from(kids, node) };
    } catch (e) {
      return { error: String(e) };
    }
  };
  return (async () => {
    const out = [];
    for (const c of cases) out.push(await one(c));
    return out;
  })();
}

const names = (process.argv[2] || "chromium,firefox,webkit").split(",");
let input = "";
for await (const chunk of process.stdin) input += chunk;
const cases = JSON.parse(input).map((c, index) => ({ ...c, index }));
const out = {};
for (const name of names) {
  const browser = await ENGINES[name].launch({ headless: true });
  try {
    const page = await browser.newPage();
    await page.route(`${ORIGIN}/**`, (route) => {
      const path = new URL(route.request().url()).pathname;
      const m = path.match(/^\/case\/(\d+)$/);
      const body = m ? cases[Number(m[1])].data : "<!DOCTYPE html><body></body>";
      return route.fulfill({ status: 200, contentType: "text/html; charset=utf-8", body });
    });
    await page.goto(`${ORIGIN}/`);
    const trees = [];
    for (let i = 0; i < cases.length; i += 50) {
      trees.push(...(await page.evaluate(parseAll, cases.slice(i, i + 50))));
    }
    out[name] = trees;
    out[`${name}_version`] = browser.version();
  } finally {
    await browser.close();
  }
}
process.stdout.write(JSON.stringify(out));
