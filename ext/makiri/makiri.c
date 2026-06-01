#include "makiri.h"

VALUE mkr_mMakiri;
VALUE mkr_cNode;
VALUE mkr_cDocument;
VALUE mkr_cElement;
VALUE mkr_cAttribute;
VALUE mkr_cText;
VALUE mkr_cComment;
VALUE mkr_cCData;
VALUE mkr_cProcessingInstruction;
VALUE mkr_cDocumentFragment;
VALUE mkr_cNodeSet;
VALUE mkr_cXPathContext;
VALUE mkr_mXPath;
VALUE mkr_mCSS;
VALUE mkr_eError;
VALUE mkr_eXPathSyntaxError;
VALUE mkr_eXPathLimitExceeded;
VALUE mkr_eCSSSyntaxError;

RUBY_FUNC_EXPORTED void
Init_makiri(void)
{
    mkr_mMakiri        = rb_define_module("Makiri");
    mkr_cNode          = rb_define_class_under(mkr_mMakiri, "Node",         rb_cObject);
    mkr_cDocument      = rb_define_class_under(mkr_mMakiri, "Document",     mkr_cNode);
    mkr_cElement       = rb_define_class_under(mkr_mMakiri, "Element",      mkr_cNode);
    mkr_cAttribute     = rb_define_class_under(mkr_mMakiri, "Attribute",    mkr_cNode);
    mkr_cText          = rb_define_class_under(mkr_mMakiri, "Text",         mkr_cNode);
    mkr_cComment       = rb_define_class_under(mkr_mMakiri, "Comment",      mkr_cNode);
    mkr_cCData         = rb_define_class_under(mkr_mMakiri, "CData",        mkr_cNode);
    mkr_cProcessingInstruction =
        rb_define_class_under(mkr_mMakiri, "ProcessingInstruction", mkr_cNode);
    mkr_cDocumentFragment =
        rb_define_class_under(mkr_mMakiri, "DocumentFragment", mkr_cNode);
    mkr_cNodeSet       = rb_define_class_under(mkr_mMakiri, "NodeSet",      rb_cObject);
    mkr_cXPathContext  = rb_define_class_under(mkr_mMakiri, "XPathContext", rb_cObject);

    mkr_mXPath         = rb_define_module_under(mkr_mMakiri, "XPath");
    mkr_mCSS           = rb_define_module_under(mkr_mMakiri, "CSS");

    mkr_eError              = rb_define_class_under(mkr_mMakiri, "Error", rb_eStandardError);
    mkr_eXPathSyntaxError   = rb_define_class_under(mkr_mXPath, "SyntaxError",   mkr_eError);
    mkr_eXPathLimitExceeded = rb_define_class_under(mkr_mXPath, "LimitExceeded", mkr_eXPathSyntaxError);
    mkr_eCSSSyntaxError     = rb_define_class_under(mkr_mCSS,   "SyntaxError",   mkr_eError);

    /* Instances of these classes are created only from C via
     * TypedData_Make_Struct (document parse / tree traversal / query
     * results), never via Ruby's `.new`. Undefine the inherited object
     * allocator so Ruby does not warn on first allocation and so `.new`
     * fails cleanly. Direct construction arrives with the v0.2 mutation API. */
    rb_undef_alloc_func(mkr_cNode);
    rb_undef_alloc_func(mkr_cDocument);
    rb_undef_alloc_func(mkr_cElement);
    rb_undef_alloc_func(mkr_cAttribute);
    rb_undef_alloc_func(mkr_cText);
    rb_undef_alloc_func(mkr_cComment);
    rb_undef_alloc_func(mkr_cCData);
    rb_undef_alloc_func(mkr_cProcessingInstruction);
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
    mkr_init_serialize();
    mkr_init_mutate();
}
