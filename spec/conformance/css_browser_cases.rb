# frozen_string_literal: true
#
# The documents and selectors spec/conformance/css_browser_diff.rb runs in
# Makiri and in the browsers. Every selector of SELECTORS runs against every
# document, in both modes (querySelectorAll from the document, and
# element.matches over every element); a document's own `selectors` run the
# same way, and its `contexts` run querySelectorAll from the element the key
# selects, which is where the scope rule shows.
#
# Every HTML document has a doctype: WebKit's DOMParser does not report quirks
# mode, so quirks cases would not compare.

module CssBrowserCases
  XHTML = "http://www.w3.org/1999/xhtml"
  SVG = "http://www.w3.org/2000/svg"
  XLINK = "http://www.w3.org/1999/xlink"

  SELECTORS = [
    # types, universal, case
    "*", "p", "P", "li", "span", "svg", "path", "PATH", "foreignObject", "foreignobject",
    # combinators
    "div p", "div > p", "p + p", "p ~ span", "ul li:last-child", "b || d",
    # the :nth-child family
    ":first-child", ":last-child", ":only-child", "li:nth-child(2)", "li:nth-last-child(1)",
    "p:first-of-type", "span:last-of-type", ":nth-child(2n+1 of .x)", ":nth-last-child(1 of p)",
    # tree state
    ":root", ":empty", ":not(:empty)",
    # logical
    ":is(p, li)", ":where(span)", ":not(p)", "div:has(> p)", ":is()", "div:has(span, li)",
    # attributes
    "[title]", "[TITLE]", "[title='Hello World']", "[title='hello world']", "[title='hello world' i]",
    "[title='Hello World' s]", "[title~=World]", "[data-n^='1']", "[data-n$='2']", "[data-n*='2']",
    "[lang|=en]", "[type=text]", "[type=TEXT]", "[viewBox]", "[viewbox]",
    # id and class
    "#p1", "#P1", ".x", ".X", ".A", ".a",
    # namespaces
    "*|p", "|p", "*|*", "|*", "s|p", "svg|path", "[*|href]", "[|href]", "[xlink|href]",
    # links and user action
    ":link", ":any-link", ":visited", ":hover", ":focus", ":active", ":target",
    # forms
    ":checked", ":disabled", ":enabled", ":required", ":optional", ":read-only", ":read-write",
    ":placeholder-shown", ":default", ":indeterminate", "input:read-write", "b:read-write",
    "p:empty", "svg a:link",
    # language, scope and the rest
    ":lang(en)", ":lang(fr)", ":dir(ltr)", ":scope", ":scope > *", ":defined",
    "::before", "p::before", ":current", ":foo", "foo(", "p:nth-child(", ""
  ].freeze

  DOCUMENTS = [
    {
      name: "html-basic", mime: "text/html",
      src: <<~HTML,
        <!DOCTYPE html><html lang="en"><body><div id="d" class="A b"><?pi x?><p id="p1" class="x" title="Hello World" data-n="12">t</p><!--c--><p id="p2" lang="fr" class="x"></p><span></span><span> </span><ul><li>1</li><li class="x">2</li><li>3</li></ul></div><p></p></body></html>
      HTML
      contexts: { "#d" => ["body p", "html div p", ":scope > p", "div p", "p"], "ul" => ["div li", "li"] }
    },
    {
      name: "html-form", mime: "text/html",
      src: <<~HTML,
        <!DOCTYPE html><html><body><form>
        <input id="i1" type="checkbox" checked><input id="i2" type="TEXT" required placeholder="p"><input id="i3" readonly>
        <input id="i4" type="radio"><textarea id="t1"></textarea><select id="s1"><option id="o1">a</option><option id="o2" selected>b</option></select>
        <fieldset disabled><legend><input id="i5"></legend><input id="i6"></fieldset><button id="b1" disabled>x</button>
        </form><a id="a1" href="x">a</a><a id="a2">b</a><area id="ar" href="x"><map id="m1"></map><link id="l1" href="x">
        <svg><a id="sa" href="x"><path/></a><a id="sx" xlink:href="x"><path/></a></svg>
        <div contenteditable id="ce"><p id="cp">x</p><span contenteditable="false"><b id="cf">y</b></span><i contenteditable="bogus">z</i></div>
        <div contenteditable="plaintext-only" id="pt"><b>w</b></div><div contenteditable="FALSE"><b>v</b></div>
        <input id="h" type="hidden"><input id="rg" type="range"><input id="nu" type="NUMBER"><input id="em" type="email" readonly><input id="un" type="bogus">
        <p id="e1"></p><p id="e2"><!--c--></p><p id="e3"> </p></body></html>
      HTML
    },
    {
      name: "html-foreign", mime: "text/html",
      src: <<~HTML,
        <!DOCTYPE html><html><body><p>x</p><svg viewBox="0 0 1 1"><path id="pa"/><foreignObject><p id="fp">y</p></foreignObject><a xlink:href="u"></a></svg><math><mi>x</mi></math></body></html>
      HTML
    },
    {
      name: "xml-plain", mime: "application/xml",
      src: <<~XML,
        <r lang="en"><?pi x?><div id="d" class="A b"><p id="p1" class="x" title="Hello World" data-n="12" type="TEXT">t</p><!--c--><p id="p2" xml:lang="fr" class="x"/><span/><span> </span><span><![CDATA[]]></span><span><?pi y?></span><ul><li>1</li><li class="x">2</li><li>3</li></ul></div><P/><p/></r>
      XML
      contexts: { "div" => ["r div p", "r p", "div p", ":scope > p", "p"], "ul" => ["div li", "li"] }
    },
    {
      name: "xml-ns", mime: "application/xml",
      src: <<~XML,
        <r xmlns="urn:a" xmlns:s="urn:s" xmlns:xlink="#{XLINK}"><p id="p1"/><s:p id="p2" xlink:href="h"/><q xmlns="" id="q1"><p/></q></r>
      XML
      contexts: { "q" => ["r p", "p"] }
    },
    {
      name: "xml-xhtml", mime: "application/xml",
      src: <<~XML,
        <html xmlns="#{XHTML}" xmlns:svg="#{SVG}" xmlns:xlink="#{XLINK}"><body><form><input id="i1" type="checkbox" checked=""/><input id="i2" type="TEXT" required="" placeholder="p"/><input id="i3" disabled=""/><fieldset disabled=""><input id="i4"/></fieldset><select><option selected="">a</option></select></form><a id="a1" href="x">a</a><a id="a2">b</a><DIV id="D"/><P title="Hello World"/><svg:svg><svg:a xlink:href="x"><svg:path/></svg:a><svg:a href="y"/></svg:svg></body></html>
      XML
    }
  ].freeze
end
