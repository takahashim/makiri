#include "makiri.h"
#include "core/mkr_core.h"
#include "bridge/bridge.h"
#include "xml/mkr_xml.h"
#include "xml/mkr_xml_mutate.h"

VALUE mkr_mMakiri;
VALUE mkr_cNode;
VALUE mkr_cDocument;
VALUE mkr_cElement;
VALUE mkr_cAttr;
VALUE mkr_cText;
VALUE mkr_cComment;
VALUE mkr_cCDATASection;
VALUE mkr_cProcessingInstruction;
VALUE mkr_cDocumentType;
VALUE mkr_cDocumentFragment;
VALUE mkr_mHTML;
VALUE mkr_mHtmlNodeMethods;
VALUE mkr_cHtmlNode;
VALUE mkr_cHtmlDocument;
VALUE mkr_cHtmlElement;
VALUE mkr_cHtmlAttr;
VALUE mkr_cHtmlText;
VALUE mkr_cHtmlComment;
VALUE mkr_cHtmlCDATASection;
VALUE mkr_cHtmlProcessingInstruction;
VALUE mkr_cHtmlDocumentType;
VALUE mkr_cHtmlDocumentFragment;
VALUE mkr_mXmlNodeMethods;
VALUE mkr_cXmlNode;
VALUE mkr_cXmlDocument;
VALUE mkr_cXmlElement;
VALUE mkr_cXmlAttr;
VALUE mkr_cXmlText;
VALUE mkr_cXmlComment;
VALUE mkr_cXmlCDATASection;
VALUE mkr_cXmlProcessingInstruction;
VALUE mkr_cXmlDocumentType;
VALUE mkr_cXmlDocumentFragment;
VALUE mkr_cNodeSet;
VALUE mkr_cXPathContext;
VALUE mkr_mXPath;
VALUE mkr_mCSS;
VALUE mkr_mXML;
VALUE mkr_mLexbor;
VALUE mkr_eError;
VALUE mkr_eXPathSyntaxError;
VALUE mkr_eXPathLimitExceeded;
VALUE mkr_eCSSSyntaxError;
VALUE mkr_eXmlSyntaxError;
VALUE mkr_eXmlLimitExceeded;

/* Makiri.__c_selftest -> true, or raises if the safe-core primitives
 * (mkr_core.c) fail their internal edge-case checks. Test hook only. */
static VALUE
mkr_c_selftest(VALUE self)
{
    (void)self;
    int rc = mkr_core_selftest();
    if (rc != 0) {
        rb_raise(mkr_eError, "mkr_core_selftest failed at check %d", rc);
    }
    int xc = mkr_xml_node_selftest();
    if (xc != 0) {
        rb_raise(mkr_eError, "mkr_xml_node_selftest failed at check %d", xc);
    }
    int pc = mkr_xml_parse_selftest();
    if (pc != 0) {
        rb_raise(mkr_eError, "mkr_xml_parse_selftest failed at check %d", pc);
    }
    int qc = mkr_xml_xpath_selftest();
    if (qc != 0) {
        rb_raise(mkr_eError, "mkr_xml_xpath_selftest failed at check %d", qc);
    }
    int mc = mkr_xml_mutate_selftest();
    if (mc != 0) {
        rb_raise(mkr_eError, "mkr_xml_mutate_selftest failed at check %d", mc);
    }
    return Qtrue;
}

/* Makiri.__alloc_inject(n) / __alloc_inject_calls / __alloc_inject? - the OOM
 * sweep harness's controls (script/check_alloc_failures.rb, `rake oom`): arm
 * "the nth core allocation fails once", and read how many core allocations a
 * workload attempted. Compiled to a real hook only under MKR_ALLOC_INJECT
 * (extconf: MAKIRI_ALLOC_INJECT=1); in a normal build __alloc_inject? is
 * false and the others raise, so the harness fails loudly on the wrong build
 * instead of sweeping nothing. Test hooks only. */
static VALUE
mkr_s_alloc_inject_p(VALUE self)
{
    (void)self;
#ifdef MKR_ALLOC_INJECT
    return Qtrue;
#else
    return Qfalse;
#endif
}

static VALUE
mkr_s_alloc_inject(VALUE self, VALUE nth)
{
    (void)self;
#ifdef MKR_ALLOC_INJECT
    mkr_alloc_inject_arm((long long)NUM2LL(nth));
    return Qnil;
#else
    (void)nth;
    rb_raise(rb_eNotImpError, "rebuild with MAKIRI_ALLOC_INJECT=1 (rake oom does this)");
#endif
}

static VALUE
mkr_s_alloc_inject_calls(VALUE self)
{
    (void)self;
#ifdef MKR_ALLOC_INJECT
    return ULL2NUM(mkr_alloc_inject_calls());
#else
    rb_raise(rb_eNotImpError, "rebuild with MAKIRI_ALLOC_INJECT=1 (rake oom does this)");
#endif
}

/* Makiri::XML.__decode(str) -> validated, UTF-8-tagged String, or raises
 * Makiri::XML::SyntaxError. Internal test hook exercising the strict input
 * decode (§2.1) on its own, until the full Makiri::XML(...) parse pipeline
 * (tokenizer + tree builder) lands and subsumes it. */
static VALUE
mkr_xml_s_decode(VALUE self, VALUE str)
{
    (void)self;
    return mkr_xml_decode_input(rb_String(str), 0);  /* decode-only: no arena, no budget */
}

RUBY_FUNC_EXPORTED void
Init_makiri(void)
{
    mkr_mMakiri        = rb_define_module("Makiri");
    mkr_cNode          = rb_define_class_under(mkr_mMakiri, "Node",         rb_cObject);
    mkr_cDocument      = rb_define_class_under(mkr_mMakiri, "Document",     mkr_cNode);
    mkr_cElement       = rb_define_class_under(mkr_mMakiri, "Element",      mkr_cNode);
    mkr_cAttr     = rb_define_class_under(mkr_mMakiri, "Attr",    mkr_cNode);
    mkr_cText          = rb_define_class_under(mkr_mMakiri, "Text",         mkr_cNode);
    mkr_cComment       = rb_define_class_under(mkr_mMakiri, "Comment",      mkr_cNode);
    mkr_cCDATASection         = rb_define_class_under(mkr_mMakiri, "CDATASection",        mkr_cNode);
    mkr_cProcessingInstruction =
        rb_define_class_under(mkr_mMakiri, "ProcessingInstruction", mkr_cNode);
    mkr_cDocumentType =
        rb_define_class_under(mkr_mMakiri, "DocumentType", mkr_cNode);
    mkr_cDocumentFragment =
        rb_define_class_under(mkr_mMakiri, "DocumentFragment", mkr_cNode);
    mkr_cNodeSet       = rb_define_class_under(mkr_mMakiri, "NodeSet",      rb_cObject);
    mkr_cXPathContext  = rb_define_class_under(mkr_mMakiri, "XPathContext", rb_cObject);

    mkr_mXPath         = rb_define_module_under(mkr_mMakiri, "XPath");
    mkr_mCSS           = rb_define_module_under(mkr_mMakiri, "CSS");
    mkr_mXML           = rb_define_module_under(mkr_mMakiri, "XML");
    mkr_mLexbor        = rb_define_module_under(mkr_mMakiri, "Lexbor");

    /* Per-kind hierarchy (§12): the classes above are abstract bases; concrete
     * HTML nodes are Makiri::HTML::* leaves. The reader/query methods (which read
     * lxb_dom) live on the mkr_mHtmlNodeMethods behavior module and are included into
     * every leaf, so XML nodes (added in step 2) never inherit an HTML reader. */
    mkr_mHTML          = rb_define_module_under(mkr_mMakiri, "HTML");
    mkr_mHtmlNodeMethods = rb_define_module_under(mkr_mHTML, "NodeMethods");
    /* The concrete generic HTML node - the wrap fallback for an uncommon DOM node
     * type with no more specific leaf. Carries the readers but is not an Element. */
    mkr_cHtmlNode      = rb_define_class_under(mkr_mHTML, "Node",         mkr_cNode);
    mkr_cHtmlDocument  = rb_define_class_under(mkr_mHTML, "Document",     mkr_cDocument);
    mkr_cHtmlElement   = rb_define_class_under(mkr_mHTML, "Element",      mkr_cElement);
    mkr_cHtmlAttr = rb_define_class_under(mkr_mHTML, "Attr",    mkr_cAttr);
    mkr_cHtmlText      = rb_define_class_under(mkr_mHTML, "Text",         mkr_cText);
    mkr_cHtmlComment   = rb_define_class_under(mkr_mHTML, "Comment",      mkr_cComment);
    mkr_cHtmlCDATASection     = rb_define_class_under(mkr_mHTML, "CDATASection",        mkr_cCDATASection);
    mkr_cHtmlProcessingInstruction =
        rb_define_class_under(mkr_mHTML, "ProcessingInstruction", mkr_cProcessingInstruction);
    mkr_cHtmlDocumentType =
        rb_define_class_under(mkr_mHTML, "DocumentType", mkr_cDocumentType);
    mkr_cHtmlDocumentFragment =
        rb_define_class_under(mkr_mHTML, "DocumentFragment", mkr_cDocumentFragment);

    /* Every HTML leaf - the generic node and the typed leaves - gets the shared
     * lxb_dom reader/query methods. */
    VALUE html_leaves[] = {
        mkr_cHtmlNode, mkr_cHtmlDocument, mkr_cHtmlElement, mkr_cHtmlAttr,
        mkr_cHtmlText, mkr_cHtmlComment, mkr_cHtmlCDATASection,
        mkr_cHtmlProcessingInstruction, mkr_cHtmlDocumentType,
        mkr_cHtmlDocumentFragment,
    };
    for (size_t i = 0; i < sizeof(html_leaves) / sizeof(html_leaves[0]); i++) {
        rb_include_module(html_leaves[i], mkr_mHtmlNodeMethods);
        rb_undef_alloc_func(html_leaves[i]); /* created only from C, never .new */
    }

    /* Makiri::XML leaves (the custom-node counterparts). XML::Document itself is
     * defined in mkr_init_xml (it backs a mkr_parsed_t, not a node). */
    mkr_mXmlNodeMethods = rb_define_module_under(mkr_mXML, "NodeMethods");
    mkr_cXmlNode      = rb_define_class_under(mkr_mXML, "Node",      mkr_cNode);
    mkr_cXmlElement   = rb_define_class_under(mkr_mXML, "Element",   mkr_cElement);
    mkr_cXmlAttr = rb_define_class_under(mkr_mXML, "Attr", mkr_cAttr);
    mkr_cXmlText      = rb_define_class_under(mkr_mXML, "Text",      mkr_cText);
    mkr_cXmlComment   = rb_define_class_under(mkr_mXML, "Comment",   mkr_cComment);
    mkr_cXmlCDATASection     = rb_define_class_under(mkr_mXML, "CDATASection",     mkr_cCDATASection);
    mkr_cXmlProcessingInstruction =
        rb_define_class_under(mkr_mXML, "ProcessingInstruction", mkr_cProcessingInstruction);
    /* Makiri::XML::DocumentType - the off-tree DOCTYPE metadata node
     * (doc->doctype), reachable only via Document#internal_subset (XPath has no
     * doctype node, as in Nokogiri/libxml2). The canonical name is the WHATWG DOM
     * interface name DocumentType (with a Makiri::XML::DTD alias for Nokogiri
     * compatibility, defined in Ruby); it descends from the shared
     * Makiri::DocumentType base (like HTML::DocumentType) - not Makiri::XML::Node -
     * so is_a?(Makiri::DocumentType) holds for both HTML and XML doctypes. It is
     * still an XML node leaf (the reader/identity methods come from the included
     * mkr_mXmlNodeMethods below), with its own name/external_id/system_id readers. */
    mkr_cXmlDocumentType       = rb_define_class_under(mkr_mXML, "DocumentType", mkr_cDocumentType);
    /* Makiri::XML::DocumentFragment - a detached group of sibling nodes built by
     * XML::DocumentFragment.parse / XML::Document#fragment (an XML node leaf, like
     * its HTML counterpart). */
    mkr_cXmlDocumentFragment =
        rb_define_class_under(mkr_mXML, "DocumentFragment", mkr_cDocumentFragment);
    VALUE xml_leaves[] = {
        mkr_cXmlNode, mkr_cXmlElement, mkr_cXmlAttr, mkr_cXmlText,
        mkr_cXmlComment, mkr_cXmlCDATASection, mkr_cXmlProcessingInstruction, mkr_cXmlDocumentType,
        mkr_cXmlDocumentFragment,
    };
    for (size_t i = 0; i < sizeof(xml_leaves) / sizeof(xml_leaves[0]); i++) {
        rb_include_module(xml_leaves[i], mkr_mXmlNodeMethods);
        rb_undef_alloc_func(xml_leaves[i]);
    }

    mkr_eError              = rb_define_class_under(mkr_mMakiri, "Error", rb_eStandardError);
    mkr_eXPathSyntaxError   = rb_define_class_under(mkr_mXPath, "SyntaxError",   mkr_eError);
    mkr_eXPathLimitExceeded = rb_define_class_under(mkr_mXPath, "LimitExceeded", mkr_eXPathSyntaxError);
    mkr_eCSSSyntaxError     = rb_define_class_under(mkr_mCSS,   "SyntaxError",   mkr_eError);
    mkr_eXmlSyntaxError     = rb_define_class_under(mkr_mXML,   "SyntaxError",   mkr_eError);
    mkr_eXmlLimitExceeded   = rb_define_class_under(mkr_mXML,   "LimitExceeded", mkr_eError);

    /* Instances of these classes are created only from C via
     * TypedData_Make_Struct (document parse / tree traversal / query
     * results), never via Ruby's `.new`. Undefine the inherited object
     * allocator so Ruby does not warn on first allocation and so `.new`
     * fails cleanly. Direct construction arrives with the v0.2 mutation API. */
    rb_undef_alloc_func(mkr_cNode);
    rb_undef_alloc_func(mkr_cDocument);
    rb_undef_alloc_func(mkr_cElement);
    rb_undef_alloc_func(mkr_cAttr);
    rb_undef_alloc_func(mkr_cText);
    rb_undef_alloc_func(mkr_cComment);
    rb_undef_alloc_func(mkr_cCDATASection);
    rb_undef_alloc_func(mkr_cProcessingInstruction);
    rb_undef_alloc_func(mkr_cDocumentType);
    rb_undef_alloc_func(mkr_cDocumentFragment);
    rb_undef_alloc_func(mkr_cNodeSet);
    /* XPathContext is also created only from C (via XPathContext.new, which
     * wraps a native context with TypedData_Make_Struct). */
    rb_undef_alloc_func(mkr_cXPathContext);

    mkr_init_node();
    mkr_init_document();
    mkr_init_node_set();
    mkr_init_xpath();
    mkr_init_css();
    mkr_init_lexbor_css();
    mkr_init_serialize();
    mkr_init_mutate();
    mkr_init_xml();
    mkr_init_xml_node();

    rb_define_singleton_method(mkr_mMakiri, "__c_selftest", mkr_c_selftest, 0);
    rb_define_singleton_method(mkr_mMakiri, "__alloc_inject?", mkr_s_alloc_inject_p, 0);
    rb_define_singleton_method(mkr_mMakiri, "__alloc_inject", mkr_s_alloc_inject, 1);
    rb_define_singleton_method(mkr_mMakiri, "__alloc_inject_calls", mkr_s_alloc_inject_calls, 0);
    rb_define_singleton_method(mkr_mXML, "__decode", mkr_xml_s_decode, 1);
}
