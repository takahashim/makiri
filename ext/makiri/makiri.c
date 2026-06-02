#include "makiri.h"
#include "core/mkr_safe.h"

#include <stdio.h>
#include <string.h>

VALUE mkr_mMakiri;
VALUE mkr_cNode;
VALUE mkr_cDocument;
VALUE mkr_cElement;
VALUE mkr_cAttribute;
VALUE mkr_cText;
VALUE mkr_cComment;
VALUE mkr_cCData;
VALUE mkr_cProcessingInstruction;
VALUE mkr_cDocumentType;
VALUE mkr_cDocumentFragment;
VALUE mkr_cNodeSet;
VALUE mkr_cXPathContext;
VALUE mkr_mXPath;
VALUE mkr_mCSS;
VALUE mkr_eError;
VALUE mkr_eXPathSyntaxError;
VALUE mkr_eXPathLimitExceeded;
VALUE mkr_eCSSSyntaxError;

void
mkr_check_text(VALUE str, const char *what)
{
    long        len = RSTRING_LEN(str);
    const char *ptr = RSTRING_PTR(str);

    if (len > 0 && memchr(ptr, '\0', (size_t)len) != NULL) {
        rb_raise(mkr_eError, "%s must not contain a NUL byte", what);
    }

    /* Validate the bytes as UTF-8 regardless of the String's declared encoding
     * (Makiri accepts UTF-8 text only). rb_enc_str_coderange reports BROKEN for
     * byte sequences that are not well-formed UTF-8. */
    VALUE u = rb_enc_str_new(ptr, len, rb_utf8_encoding());
    if (rb_enc_str_coderange(u) == ENC_CODERANGE_BROKEN) {
        rb_raise(mkr_eError, "%s must be valid UTF-8", what);
    }
    RB_GC_GUARD(str);
}

mkr_ruby_text_view_t
mkr_ruby_checked_text(VALUE in, const char *what)
{
    VALUE s = rb_String(in);
    mkr_check_text(s, what);
    /* s is now a String whose bytes are valid UTF-8 with no NUL, so ptr doubles
     * as a C string. The returned view holds `value`; keeping the view on the C
     * stack keeps the bytes alive (Ruby marks the machine stack). */
    mkr_ruby_text_view_t v;
    v.value = s;
    v.ptr   = RSTRING_PTR(s);
    v.len   = (size_t)RSTRING_LEN(s);
    return v;
}

mkr_ruby_bytes_view_t
mkr_ruby_bytes_view(VALUE in)
{
    VALUE s = rb_String(in);
    mkr_ruby_bytes_view_t v;
    v.value = s;
    v.ptr   = RSTRING_PTR(s);
    v.len   = (size_t)RSTRING_LEN(s);
    return v;
}

int
mkr_ruby_copy_bytes(VALUE in, mkr_owned_bytes_t *out)
{
    mkr_ruby_bytes_view_t v = mkr_ruby_bytes_view(in);
    out->ptr = NULL;
    out->len = 0;

    size_t alloc_len = (v.len > 0) ? v.len : 1;
    char *buf = mkr_reallocarray(NULL, alloc_len, 1);
    if (buf == NULL) {
        RB_GC_GUARD(v.value);
        return -1;
    }
    if (v.len > 0) {
        memcpy(buf, v.ptr, v.len);
    }
    out->ptr = buf;
    out->len = v.len;
    RB_GC_GUARD(v.value);
    return 0;
}

const char *
mkr_ruby_engine_string_view(VALUE sv, size_t max_bytes, mkr_ruby_text_view_t *out)
{
    long len = RSTRING_LEN(sv);
    if ((size_t)len > max_bytes) {
        return "string exceeds the maximum length";
    }
    const char *ptr = RSTRING_PTR(sv);
    if (memchr(ptr, '\0', (size_t)len) != NULL) {
        return "string contains a NUL byte";
    }
    /* Validate the bytes as UTF-8 using Ruby's own validator (GVL is held in
     * the variable-registration and handler paths). */
    VALUE u = rb_utf8_str_new(ptr, len);
    if (!RTEST(rb_funcall(u, rb_intern("valid_encoding?"), 0))) {
        return "string is not valid UTF-8";
    }
    out->value = sv;
    out->ptr   = ptr;
    out->len   = (size_t)len;
    RB_GC_GUARD(u);
    return NULL;
}

static VALUE
mkr_exception_message_thunk(VALUE exc)
{
    return rb_obj_as_string(rb_funcall(exc, rb_intern("message"), 0));
}

void
mkr_ruby_exception_message(VALUE exc, char *buf, size_t len)
{
    if (buf == NULL || len == 0) {
        return;
    }
    int state = 0;
    VALUE msg = rb_protect(mkr_exception_message_thunk, exc, &state);
    if (state != 0) {
        rb_set_errinfo(Qnil);
        snprintf(buf, len, "%s", "error");
        return;
    }
    if (!RB_TYPE_P(msg, T_STRING)) {
        snprintf(buf, len, "%s", "error");
        RB_GC_GUARD(msg);
        return;
    }
    snprintf(buf, len, "%s", RSTRING_PTR(msg));
    RB_GC_GUARD(msg);
}

mkr_valid_text_t
mkr_text_from_view(mkr_ruby_text_view_t v)
{
    /* The one sanctioned mint of mkr_valid_text_t: v has already passed the
     * strict text contract (mkr_ruby_checked_text / mkr_ruby_engine_string_view). */
    return (mkr_valid_text_t){ v.ptr, v.len };
}

/* Makiri.__c_selftest -> true, or raises if the safe-core primitives
 * (mkr_safe.c) fail their internal edge-case checks. Test hook only. */
static VALUE
mkr_c_selftest(VALUE self)
{
    (void)self;
    int rc = mkr_safe_selftest();
    if (rc != 0) {
        rb_raise(mkr_eError, "mkr_safe_selftest failed at check %d", rc);
    }
    return Qtrue;
}

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
    mkr_cDocumentType =
        rb_define_class_under(mkr_mMakiri, "DocumentType", mkr_cNode);
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
    mkr_init_serialize();
    mkr_init_mutate();

    rb_define_singleton_method(mkr_mMakiri, "__c_selftest", mkr_c_selftest, 0);
}
