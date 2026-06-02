#include "bridge.h"
#include "../makiri.h"

#include <limits.h>
#include <stdio.h>
#include <string.h>

VALUE
mkr_ruby_str_from_slices(const mkr_borrowed_text_t *slices, size_t n, size_t total)
{
    /* Defensive bounds: the caller (text index) guarantees the slice lengths sum
     * to total, but a wrong total would overflow the output buffer. Fail closed
     * rather than memcpy past the allocation. */
    if (total > (size_t)LONG_MAX) {
        rb_raise(mkr_eError, "text too large to assemble");
    }
    VALUE  str = rb_utf8_str_new(NULL, (long)total);
    char  *dst = RSTRING_PTR(str);
    size_t off = 0;
    for (size_t i = 0; i < n; i++) {
        size_t len = slices[i].len;
        if (len == 0) continue;
        if (len > total - off) { /* off <= total holds, so total - off can't underflow */
            rb_raise(mkr_eError, "text slice length inconsistency");
        }
        memcpy(dst + off, slices[i].ptr, len);
        off += len;
    }
    return str;
}

void
mkr_check_text(VALUE str, const char *what)
{
    long        len = RSTRING_LEN(str);
    const char *ptr = RSTRING_PTR(str);

    if (len > 0 && memchr(ptr, '\0', (size_t)len) != NULL) {
        rb_raise(mkr_eError, "%s must not contain a NUL byte", what);
    }

    /* Validate the bytes as UTF-8 regardless of the String's declared encoding. */
    VALUE u = rb_enc_str_new(ptr, len, rb_utf8_encoding());
    if (rb_enc_str_coderange(u) == ENC_CODERANGE_BROKEN) {
        rb_raise(mkr_eError, "%s must be valid UTF-8", what);
    }
    RB_GC_GUARD(str);
}

mkr_ruby_borrowed_text_t
mkr_ruby_checked_text(VALUE in, const char *what)
{
    VALUE s = rb_String(in);
    mkr_check_text(s, what);
    mkr_ruby_borrowed_text_t v;
    v.value = s;
    v.ptr   = RSTRING_PTR(s);
    v.len   = (size_t)RSTRING_LEN(s);
    return v;
}

mkr_ruby_borrowed_bytes_t
mkr_ruby_bytes_view(VALUE in)
{
    VALUE s = rb_String(in);
    mkr_ruby_borrowed_bytes_t v;
    v.value = s;
    v.ptr   = RSTRING_PTR(s);
    v.len   = (size_t)RSTRING_LEN(s);
    return v;
}

int
mkr_ruby_copy_bytes(VALUE in, mkr_owned_bytes_t *out)
{
    mkr_ruby_borrowed_bytes_t v = mkr_ruby_bytes_view(in);
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
mkr_ruby_engine_string_view(VALUE sv, size_t max_bytes, mkr_ruby_borrowed_text_t *out)
{
    long len = RSTRING_LEN(sv);
    if ((size_t)len > max_bytes) {
        return "string exceeds the maximum length";
    }
    const char *ptr = RSTRING_PTR(sv);
    if (memchr(ptr, '\0', (size_t)len) != NULL) {
        return "string contains a NUL byte";
    }
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
