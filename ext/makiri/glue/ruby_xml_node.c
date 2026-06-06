/* ruby_xml_node.c — Ruby read API for custom XML nodes (Phase 1, read-only).
 *
 * The XML counterpart of ruby_node.c: it wraps a mkr_xml_node_t into the right
 * Makiri::XML::* leaf and defines the reader/query methods on the
 * Makiri::XML::NodeMethods behavior module (included into every XML leaf), each
 * reading the custom node's fields directly. XML nodes never inherit the lxb_dom
 * HTML readers (those live on Makiri::HTML::NodeMethods), so the read-only
 * guarantee is structural. The shared node TypedData (mkr_node_type) stores the
 * node pointer + a keepalive Document VALUE; for XML the pointer is a
 * mkr_xml_node_t* (the document arena outlives the wrapper via the Document).
 */
#include "glue.h"
#include "../xml/mkr_xml_node.h"
#include "../core/mkr_core.h"   /* mkr_buf */

#include <ruby/encoding.h>     /* rb_to_encoding / rb_str_encode (output encoding) */
#include <stdlib.h>            /* qsort / free (C14N attribute sort) */
#include <string.h>

/* ---- wrap / unwrap ---- */

VALUE
mkr_wrap_xml_node(mkr_xml_node_t *node, VALUE document)
{
    if (node == NULL) {
        return Qnil;
    }
    if (node->type == MKR_XML_NODE_TYPE_DOCUMENT) {
        return document;   /* the document node maps back onto the Ruby Document */
    }
    VALUE klass;
    switch (node->type) {
    case MKR_XML_NODE_TYPE_ELEMENT:   klass = mkr_cXmlElement;   break;
    case MKR_XML_NODE_TYPE_ATTRIBUTE: klass = mkr_cXmlAttribute; break;
    case MKR_XML_NODE_TYPE_TEXT:      klass = mkr_cXmlText;      break;
    case MKR_XML_NODE_TYPE_CDATA_SECTION:     klass = mkr_cXmlCData;     break;
    case MKR_XML_NODE_TYPE_COMMENT:   klass = mkr_cXmlComment;   break;
    case MKR_XML_NODE_TYPE_PI:        klass = mkr_cXmlProcessingInstruction; break;
    case MKR_XML_NODE_TYPE_DOCUMENT_TYPE: klass = mkr_cXmlDTD;       break;
    default:               klass = mkr_cXmlNode;      break;
    }
    mkr_node_data_t *nd;
    VALUE obj = TypedData_Make_Struct(klass, mkr_node_data_t, &mkr_node_type, nd);
    nd->node     = (lxb_dom_node_t *)node;   /* a mkr_xml_node_t*; XML readers cast back */
    nd->document = document;
    return obj;
}

static mkr_xml_node_t *
mkr_xml_node_unwrap(VALUE self)
{
    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        return mkr_parsed_xml_doc(mkr_doc_parsed(self))->doc_node;
    }
    mkr_node_data_t *nd;
    TypedData_Get_Struct(self, mkr_node_data_t, &mkr_node_type, nd);
    return (mkr_xml_node_t *)nd->node;
}

static VALUE
mkr_xml_node_document(VALUE self)
{
    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        return self;
    }
    mkr_node_data_t *nd;
    TypedData_Get_Struct(self, mkr_node_data_t, &mkr_node_type, nd);
    return nd->document;
}

/* ---- name / namespace ---- */

static VALUE
mkr_xml_node_name(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    switch (n->type) {
    case MKR_XML_NODE_TYPE_ELEMENT:
    case MKR_XML_NODE_TYPE_ATTRIBUTE: return rb_utf8_str_new(n->qname, (long)n->qname_len);
    case MKR_XML_NODE_TYPE_PI:        return rb_utf8_str_new(n->local, (long)n->local_len); /* target */
    case MKR_XML_NODE_TYPE_TEXT:      return rb_utf8_str_new_cstr("text");
    case MKR_XML_NODE_TYPE_CDATA_SECTION:     return rb_utf8_str_new_cstr("#cdata-section");
    case MKR_XML_NODE_TYPE_COMMENT:   return rb_utf8_str_new_cstr("comment");
    case MKR_XML_NODE_TYPE_DOCUMENT_TYPE: return rb_utf8_str_new(n->local, (long)n->local_len); /* the DOCTYPE name */
    default:               return rb_utf8_str_new_cstr("document");
    }
}

/* ---- DTD (DOCUMENT_TYPE) identifiers ----
 *
 * The off-tree doctype node repurposes fields: local/qname = the DOCTYPE name
 * (Node#name), prefix = the PUBLIC/external id, value = the SYSTEM id. A field
 * left NULL means that id was absent (-> nil); an empty literal (e.g. PUBLIC "")
 * is a non-NULL 0-length slice (-> ""). Mirrors Nokogiri::XML::DTD#external_id /
 * #system_id; #public_id is a WHATWG-DOM-style alias of #external_id. The DTD
 * itself is NOT parsed (no entities/elements), so there is nothing else to read. */
static VALUE
mkr_xml_dtd_external_id(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    return n->prefix == NULL ? Qnil : rb_utf8_str_new(n->prefix, (long)n->prefix_len);
}

static VALUE
mkr_xml_dtd_system_id(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    return n->value == NULL ? Qnil : rb_utf8_str_new(n->value, (long)n->value_len);
}

static VALUE
mkr_xml_node_local_name(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if (n->type == MKR_XML_NODE_TYPE_ELEMENT || n->type == MKR_XML_NODE_TYPE_ATTRIBUTE) {
        return rb_utf8_str_new(n->local, (long)n->local_len);
    }
    return Qnil;
}

static VALUE
mkr_xml_node_prefix(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    return n->prefix_len ? rb_utf8_str_new(n->prefix, (long)n->prefix_len) : Qnil;
}

static VALUE
mkr_xml_node_namespace_uri(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    return n->ns_uri_len ? rb_utf8_str_new(n->ns_uri, (long)n->ns_uri_len) : Qnil;
}

/* ---- namespace introspection (Nokogiri-compatible) ----
 *
 * Makiri::XML::Namespace is a small (prefix, href) value object. xmlns
 * declarations are stored as ordinary attribute nodes (qname "xmlns" /
 * "xmlns:PREFIX"), so the four queries below just read the tree:
 *   #namespace               -> the node's own resolved namespace, or nil
 *   #namespace_definitions   -> the xmlns declarations ON this element
 *   #namespaces              -> all xmlns declarations IN SCOPE here (a Hash)
 *   #collect_namespaces      -> every xmlns declaration in the document (a Hash) */
static VALUE mkr_cXmlNamespace;

static VALUE
mkr_ns_new(VALUE prefix, VALUE href)
{
    VALUE ns = rb_obj_alloc(mkr_cXmlNamespace);
    rb_ivar_set(ns, rb_intern("@prefix"), prefix);
    rb_ivar_set(ns, rb_intern("@href"), href);
    return ns;
}

static VALUE mkr_ns_prefix(VALUE self) { return rb_ivar_get(self, rb_intern("@prefix")); }
static VALUE mkr_ns_href(VALUE self)   { return rb_ivar_get(self, rb_intern("@href")); }

static VALUE
mkr_ns_equal(VALUE self, VALUE other)
{
    if (!rb_obj_is_kind_of(other, mkr_cXmlNamespace)) return Qfalse;
    return (rb_equal(mkr_ns_prefix(self), mkr_ns_prefix(other)) &&
            rb_equal(mkr_ns_href(self), mkr_ns_href(other))) ? Qtrue : Qfalse;
}

static VALUE
mkr_ns_hash(VALUE self)
{
    return rb_funcall(rb_ary_new3(2, mkr_ns_prefix(self), mkr_ns_href(self)), rb_intern("hash"), 0);
}

static VALUE
mkr_ns_inspect(VALUE self)
{
    return rb_sprintf("#<Makiri::XML::Namespace prefix=%" PRIsVALUE " href=%" PRIsVALUE ">",
                      rb_inspect(mkr_ns_prefix(self)), rb_inspect(mkr_ns_href(self)));
}

/* If +a+ is an xmlns declaration, set the declared namespace prefix (NULL,
 * plen 0, for the default xmlns) and its URI as arena slices, and return 1; else
 * 0. Pure C: callers mint Ruby objects only where the API actually needs them
 * (shared by the namespace introspection and the C14N walk). */
static int
mkr_xml_xmlns_decl(const mkr_xml_node_t *a, const char **prefix, uint32_t *plen,
                   const char **uri, uint32_t *ulen)
{
    if (a->qname_len == 5 && memcmp(a->qname, "xmlns", 5) == 0) {
        *prefix = ""; *plen = 0;
    } else if (a->qname_len > 6 && memcmp(a->qname, "xmlns:", 6) == 0) {
        *prefix = a->local; *plen = a->local_len;
    } else {
        return 0;
    }
    *uri = a->value ? a->value : ""; *ulen = a->value_len;
    return 1;
}

static VALUE
mkr_xml_node_namespace(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if ((n->type == MKR_XML_NODE_TYPE_ELEMENT || n->type == MKR_XML_NODE_TYPE_ATTRIBUTE)
        && n->ns_uri_len > 0) {
        VALUE prefix = n->prefix_len ? rb_utf8_str_new(n->prefix, (long)n->prefix_len) : Qnil;
        return mkr_ns_new(prefix, rb_utf8_str_new(n->ns_uri, (long)n->ns_uri_len));
    }
    return Qnil;
}

static VALUE
mkr_xml_node_namespace_definitions(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    VALUE arr = rb_ary_new();
    if (n->type == MKR_XML_NODE_TYPE_ELEMENT) {
        for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
            const char *p, *u; uint32_t pl, ul;
            if (mkr_xml_xmlns_decl(a, &p, &pl, &u, &ul)) {
                VALUE prefix = pl ? rb_utf8_str_new(p, (long)pl) : Qnil;
                rb_ary_push(arr, mkr_ns_new(prefix, rb_utf8_str_new(u, (long)ul)));
            }
        }
    }
    return arr;
}

static VALUE
mkr_xml_node_namespaces(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    VALUE h = rb_hash_new();
    for (mkr_xml_node_t *e = n; e != NULL; e = e->parent) {   /* inner scope wins */
        if (e->type != MKR_XML_NODE_TYPE_ELEMENT) continue;
        for (mkr_xml_node_t *a = e->attrs; a != NULL; a = a->next) {
            const char *p, *u; uint32_t pl, ul;
            if (!mkr_xml_xmlns_decl(a, &p, &pl, &u, &ul)) continue;
            VALUE key = rb_utf8_str_new(a->qname, (long)a->qname_len); /* "xmlns" / "xmlns:p" */
            if (rb_hash_lookup2(h, key, Qundef) == Qundef) {
                rb_hash_aset(h, key, rb_utf8_str_new(u, (long)ul));
            }
        }
    }
    return h;
}

static VALUE
mkr_xml_node_collect_namespaces(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    mkr_xml_node_t *root = n;
    while (root->parent != NULL) root = root->parent;   /* the DOCUMENT node */
    VALUE h = rb_hash_new();
    /* iterative pre-order over the whole tree (parent-pointer walk, no recursion) */
    for (mkr_xml_node_t *cur = root; cur != NULL;) {
        if (cur->type == MKR_XML_NODE_TYPE_ELEMENT) {
            for (mkr_xml_node_t *a = cur->attrs; a != NULL; a = a->next) {
                const char *p, *u; uint32_t pl, ul;
                if (!mkr_xml_xmlns_decl(a, &p, &pl, &u, &ul)) continue;
                rb_hash_aset(h, rb_utf8_str_new(a->qname, (long)a->qname_len),
                             rb_utf8_str_new(u, (long)ul));
            }
        }
        if (cur->first_child != NULL) { cur = cur->first_child; continue; }
        while (cur != root && cur->next == NULL) cur = cur->parent;
        if (cur == root) break;
        cur = cur->next;
    }
    return h;
}

static VALUE
mkr_xml_node_node_type(VALUE self)
{
    return INT2NUM((int)mkr_xml_node_unwrap(self)->type);
}

/* ---- content / text ---- */

static VALUE
mkr_xml_node_content(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    /* leaf data nodes return their own value verbatim. */
    if (n->type == MKR_XML_NODE_TYPE_TEXT || n->type == MKR_XML_NODE_TYPE_CDATA_SECTION
        || n->type == MKR_XML_NODE_TYPE_COMMENT || n->type == MKR_XML_NODE_TYPE_ATTRIBUTE
        || n->type == MKR_XML_NODE_TYPE_PI) {
        return rb_utf8_str_new(n->value ? n->value : "", (long)n->value_len);
    }
    /* element / document: concatenate every TEXT/CDATA descendant in document
     * order. Iterative pre-order (parent-pointer) walk — no C recursion, so a
     * deep tree cannot overflow the stack. */
    VALUE str = rb_utf8_str_new("", 0);
    mkr_xml_node_t *cur = n->first_child;
    while (cur != NULL) {
        if ((cur->type == MKR_XML_NODE_TYPE_TEXT || cur->type == MKR_XML_NODE_TYPE_CDATA_SECTION) && cur->value_len) {
            rb_str_cat(str, cur->value, (long)cur->value_len);
        }
        if (cur->first_child != NULL) { cur = cur->first_child; continue; }
        while (cur != NULL && cur != n && cur->next == NULL) cur = cur->parent;
        if (cur == NULL || cur == n) break;
        cur = cur->next;
    }
    return str;
}

static VALUE
mkr_xml_node_value(VALUE self)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    return rb_utf8_str_new(n->value ? n->value : "", (long)n->value_len);
}

/* ---- navigation ---- */

static VALUE
mkr_xml_wrap_rel(VALUE self, mkr_xml_node_t *rel)
{
    return mkr_wrap_xml_node(rel, mkr_xml_node_document(self));
}

static VALUE mkr_xml_node_parent(VALUE self)   { return mkr_xml_wrap_rel(self, mkr_xml_node_unwrap(self)->parent); }
static VALUE mkr_xml_node_next(VALUE self)     { return mkr_xml_wrap_rel(self, mkr_xml_node_unwrap(self)->next); }
static VALUE mkr_xml_node_previous(VALUE self) { return mkr_xml_wrap_rel(self, mkr_xml_node_unwrap(self)->prev); }
static VALUE mkr_xml_node_first_child(VALUE self) { return mkr_xml_wrap_rel(self, mkr_xml_node_unwrap(self)->first_child); }
static VALUE mkr_xml_node_last_child(VALUE self)  { return mkr_xml_wrap_rel(self, mkr_xml_node_unwrap(self)->last_child); }

static VALUE
mkr_xml_node_children(VALUE self)
{
    VALUE doc = mkr_xml_node_document(self);
    VALUE set = mkr_node_set_new(doc);
    for (mkr_xml_node_t *c = mkr_xml_node_unwrap(self)->first_child; c != NULL; c = c->next) {
        mkr_node_set_push(set, (lxb_dom_node_t *)c);
    }
    return set;
}

/* ---- attributes ---- */

static VALUE
mkr_xml_node_aref(VALUE self, VALUE rb_name)
{
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if (n->type != MKR_XML_NODE_TYPE_ELEMENT) return Qnil;
    mkr_ruby_borrowed_text_t nv = mkr_ruby_verified_text(rb_name, "attribute name");
    VALUE out = Qnil;
    for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
        if (a->qname_len == nv.len && memcmp(a->qname, nv.ptr, nv.len) == 0) {
            out = rb_utf8_str_new(a->value ? a->value : "", (long)a->value_len);
            break;
        }
    }
    RB_GC_GUARD(nv.value);
    return out;
}

static VALUE
mkr_xml_node_attribute_nodes(VALUE self)
{
    VALUE doc = mkr_xml_node_document(self);
    VALUE set = mkr_node_set_new(doc);
    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    if (n->type == MKR_XML_NODE_TYPE_ELEMENT) {
        for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
            mkr_node_set_push(set, (lxb_dom_node_t *)a);
        }
    }
    return set;
}

static VALUE
mkr_xml_node_get_document(VALUE self)
{
    return mkr_xml_node_document(self);
}

/* ---- XML serialization (#to_xml / #to_s) ---------------------------------
 *
 * Re-emit the parsed tree as XML 1.0 text (UTF-8). The default preserves the
 * content as parsed (no pretty-printing); `pretty: true` indents element-only
 * content (an element with any text/CDATA child stays inline, so character data
 * is never altered). Output is always well-formed and re-parses to the same
 * tree. xmlns declarations ride along as ordinary attribute nodes, so namespaces
 * round-trip without special handling. The tree depth is bounded at parse time
 * (MKR_XML_MAX_DEPTH) and the tree is read-only, so the recursive walk cannot
 * exceed that depth. */

#define MKR_XSER_APPEND(b, p, n) \
    do { if (mkr_buf_append((b), (p), (n)) != MKR_OK) return -1; } while (0)
#define MKR_XSER_LIT(b, lit) MKR_XSER_APPEND((b), (lit), sizeof(lit) - 1)

/* Append [s, s+n), escaping &, <, > (and, in an attribute value, " and the
 * whitespace TAB/LF that attribute-value normalization would otherwise fold).
 * CR is always escaped so line-ending normalization cannot alter it on reparse. */
static int
mkr_xser_escaped(mkr_buf_t *b, const char *s, uint32_t n, int attr)
{
    uint32_t start = 0;
    for (uint32_t i = 0; i < n; i++) {
        const char *rep = NULL;
        size_t replen = 0;
        switch (s[i]) {
        case '&': rep = "&amp;"; replen = 5; break;
        case '<': rep = "&lt;";  replen = 4; break;
        case '>': rep = "&gt;";  replen = 4; break;
        case '"':  if (attr) { rep = "&quot;"; replen = 6; } break;
        case '\t': if (attr) { rep = "&#9;";   replen = 4; } break;
        case '\n': if (attr) { rep = "&#10;";  replen = 5; } break;
        case '\r': rep = "&#13;"; replen = 5; break;
        default: break;
        }
        if (rep != NULL) {
            if (i > start) MKR_XSER_APPEND(b, s + start, i - start);
            MKR_XSER_APPEND(b, rep, replen);
            start = i + 1;
        }
    }
    if (n > start) MKR_XSER_APPEND(b, s + start, n - start);
    return 0;
}

static int
mkr_xser_indent(mkr_buf_t *b, int level, int width)
{
    MKR_XSER_LIT(b, "\n");
    for (int i = 0; i < level * width; i++) MKR_XSER_LIT(b, " ");
    return 0;
}

/* True if any child is character data (TEXT/CDATA): such an element is kept
 * inline even in pretty mode, so its text content is preserved exactly. */
static int
mkr_xser_has_chardata(const mkr_xml_node_t *e)
{
    for (const mkr_xml_node_t *c = e->first_child; c != NULL; c = c->next) {
        if (c->type == MKR_XML_NODE_TYPE_TEXT || c->type == MKR_XML_NODE_TYPE_CDATA_SECTION) {
            return 1;
        }
    }
    return 0;
}

static int mkr_xser_doctype(mkr_buf_t *b, const mkr_xml_node_t *dt);

static int
mkr_xser_node(mkr_buf_t *b, const mkr_xml_node_t *n, int level, int width)
{
    switch (n->type) {
    case MKR_XML_NODE_TYPE_DOCUMENT_TYPE:
        return mkr_xser_doctype(b, n);
    case MKR_XML_NODE_TYPE_ELEMENT: {
        MKR_XSER_LIT(b, "<");
        MKR_XSER_APPEND(b, n->qname, n->qname_len);
        for (const mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
            MKR_XSER_LIT(b, " ");
            MKR_XSER_APPEND(b, a->qname, a->qname_len);
            MKR_XSER_LIT(b, "=\"");
            if (mkr_xser_escaped(b, a->value ? a->value : "", a->value_len, 1) != 0) return -1;
            MKR_XSER_LIT(b, "\"");
        }
        if (n->first_child == NULL) { MKR_XSER_LIT(b, "/>"); return 0; }
        MKR_XSER_LIT(b, ">");
        int block = width > 0 && !mkr_xser_has_chardata(n);
        for (const mkr_xml_node_t *c = n->first_child; c != NULL; c = c->next) {
            if (block && mkr_xser_indent(b, level + 1, width) != 0) return -1;
            if (mkr_xser_node(b, c, level + 1, width) != 0) return -1;
        }
        if (block && mkr_xser_indent(b, level, width) != 0) return -1;
        MKR_XSER_LIT(b, "</");
        MKR_XSER_APPEND(b, n->qname, n->qname_len);
        MKR_XSER_LIT(b, ">");
        return 0;
    }
    case MKR_XML_NODE_TYPE_TEXT:
        return mkr_xser_escaped(b, n->value ? n->value : "", n->value_len, 0);
    case MKR_XML_NODE_TYPE_CDATA_SECTION:
        MKR_XSER_LIT(b, "<![CDATA[");
        MKR_XSER_APPEND(b, n->value ? n->value : "", n->value_len);
        MKR_XSER_LIT(b, "]]>");
        return 0;
    case MKR_XML_NODE_TYPE_COMMENT:
        MKR_XSER_LIT(b, "<!--");
        MKR_XSER_APPEND(b, n->value ? n->value : "", n->value_len);
        MKR_XSER_LIT(b, "-->");
        return 0;
    case MKR_XML_NODE_TYPE_PI:
        MKR_XSER_LIT(b, "<?");
        MKR_XSER_APPEND(b, n->local, n->local_len);
        if (n->value_len) { MKR_XSER_LIT(b, " "); MKR_XSER_APPEND(b, n->value, n->value_len); }
        MKR_XSER_LIT(b, "?>");
        return 0;
    default:
        return 0;
    }
}

/* <!DOCTYPE name [PUBLIC "pub" "sys" | SYSTEM "sys"]> from the off-tree node. */
static int
mkr_xser_doctype(mkr_buf_t *b, const mkr_xml_node_t *dt)
{
    MKR_XSER_LIT(b, "<!DOCTYPE ");
    MKR_XSER_APPEND(b, dt->local, dt->local_len);
    if (dt->prefix != NULL) {                 /* PUBLIC id present -> "PUBLIC pub sys" */
        MKR_XSER_LIT(b, " PUBLIC \"");
        MKR_XSER_APPEND(b, dt->prefix, dt->prefix_len);
        MKR_XSER_LIT(b, "\" \"");
        MKR_XSER_APPEND(b, dt->value ? dt->value : "", dt->value_len);
        MKR_XSER_LIT(b, "\"");
    } else if (dt->value != NULL) {           /* SYSTEM id only */
        MKR_XSER_LIT(b, " SYSTEM \"");
        MKR_XSER_APPEND(b, dt->value, dt->value_len);
        MKR_XSER_LIT(b, "\"");
    }
    MKR_XSER_LIT(b, ">");
    return 0;
}

/* call-seq: node.to_xml(pretty: false, indent: 2, encoding: "UTF-8") -> String
 * A Document also emits the XML declaration and its DOCTYPE; any other node
 * serializes just its own subtree. +encoding+ (a String or Encoding) transcodes
 * the output — a character the target cannot represent becomes a hexadecimal
 * character reference — and is named in a Document's declaration. */
static VALUE
mkr_xml_node_to_xml(int argc, VALUE *argv, VALUE self)
{
    VALUE opts;
    rb_scan_args(argc, argv, "0:", &opts);
    int width = 0;
    VALUE enc_opt = Qnil;
    if (!NIL_P(opts)) {
        if (RTEST(rb_hash_aref(opts, ID2SYM(rb_intern("pretty"))))) width = 2;
        VALUE iv = rb_hash_aref(opts, ID2SYM(rb_intern("indent")));
        if (!NIL_P(iv)) width = NUM2INT(iv) < 0 ? 0 : NUM2INT(iv);
        enc_opt = rb_hash_aref(opts, ID2SYM(rb_intern("encoding")));
    }
    /* resolve the target encoding (raises on an unknown name) + its declared name */
    rb_encoding *to_enc = NIL_P(enc_opt) ? NULL : rb_to_encoding(enc_opt);
    VALUE enc_name = NIL_P(enc_opt) ? Qnil : rb_obj_as_string(enc_opt);

    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    mkr_buf_t buf;
    mkr_buf_init(&buf, 0);
    int rc = 0;

    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        static const char decl_a[] = "<?xml version=\"1.0\" encoding=\"";
        static const char decl_b[] = "\"?>\n";
        VALUE name = NIL_P(enc_name) ? rb_utf8_str_new_cstr("UTF-8") : enc_name;
        mkr_ruby_borrowed_bytes_t nv = mkr_ruby_bytes_view(name);
        mkr_xml_doc_t *xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(self));
        rc = (mkr_buf_append(&buf, decl_a, sizeof(decl_a) - 1) == MKR_OK) ? 0 : -1;
        if (rc == 0) rc = (mkr_buf_append(&buf, nv.ptr, nv.len) == MKR_OK) ? 0 : -1;
        if (rc == 0) rc = (mkr_buf_append(&buf, decl_b, sizeof(decl_b) - 1) == MKR_OK) ? 0 : -1;
        RB_GC_GUARD(nv.value);
        if (rc == 0 && xdoc != NULL && xdoc->doctype != NULL) {
            rc = mkr_xser_doctype(&buf, xdoc->doctype);
            if (rc == 0) rc = (mkr_buf_append(&buf, "\n", 1) == MKR_OK) ? 0 : -1;
        }
        for (mkr_xml_node_t *c = n->first_child; rc == 0 && c != NULL; c = c->next) {
            rc = mkr_xser_node(&buf, c, 0, width);
            if (rc == 0) rc = (mkr_buf_append(&buf, "\n", 1) == MKR_OK) ? 0 : -1;
        }
    } else {
        rc = mkr_xser_node(&buf, n, 0, width);
    }

    if (rc != 0) {
        mkr_buf_free(&buf);
        rb_raise(mkr_eError, "out of memory serializing XML");
    }
    VALUE str = rb_utf8_str_new(buf.len ? buf.data : "", (long)buf.len);
    mkr_buf_free(&buf);

    /* Transcode to the requested encoding; an unrepresentable character becomes a
     * &#xNN; reference (ECONV_UNDEF_HEX_CHARREF) rather than raising or dropping. */
    if (to_enc != NULL && to_enc != rb_utf8_encoding() && to_enc != rb_usascii_encoding()) {
        str = rb_str_encode(str, rb_enc_from_encoding(to_enc), ECONV_UNDEF_HEX_CHARREF, Qnil);
    }
    return str;
}

/* ---- Canonical XML 1.0 (#canonicalize) -----------------------------------
 *
 * Inclusive Canonical XML 1.0 (https://www.w3.org/TR/xml-c14n), the form used
 * for XML signatures: UTF-8 output, explicit start/end tags (no `<a/>`),
 * attributes sorted by (namespace-uri, local-name), namespace declarations
 * sorted by prefix with superfluous ones removed, CDATA emitted as escaped text,
 * comments omitted unless requested. Exclusive C14N is not implemented. */

/* C14N escaping — text: & < > #xD ; attribute value: & < " #x9 #xA #xD. */
static int
mkr_c14n_escaped(mkr_buf_t *b, const char *s, uint32_t n, int attr)
{
    uint32_t start = 0;
    for (uint32_t i = 0; i < n; i++) {
        const char *rep = NULL;
        size_t replen = 0;
        switch ((unsigned char)s[i]) {
        case '&':  rep = "&amp;"; replen = 5; break;
        case '<':  rep = "&lt;";  replen = 4; break;
        case '>':  if (!attr) { rep = "&gt;";  replen = 4; } break;
        case '"':  if (attr)  { rep = "&quot;"; replen = 6; } break;
        case 0x09: if (attr)  { rep = "&#x9;";  replen = 5; } break;
        case 0x0A: if (attr)  { rep = "&#xA;";  replen = 5; } break;
        case 0x0D: rep = "&#xD;"; replen = 5; break;
        default: break;
        }
        if (rep != NULL) {
            if (i > start) MKR_XSER_APPEND(b, s + start, i - start);
            MKR_XSER_APPEND(b, rep, replen);
            start = i + 1;
        }
    }
    if (n > start) MKR_XSER_APPEND(b, s + start, n - start);
    return 0;
}

/* slice compare (lexicographic; shorter sorts first on a shared prefix). */
static int
mkr_slice_cmp(const char *a, uint32_t al, const char *bb, uint32_t bl)
{
    uint32_t m = al < bl ? al : bl;
    int c = m ? memcmp(a ? a : "", bb ? bb : "", m) : 0;
    if (c != 0) return c;
    return al == bl ? 0 : (al < bl ? -1 : 1);
}

static int
mkr_c14n_attr_cmp(const void *pa, const void *pb)   /* by (namespace-uri, local) */
{
    const mkr_xml_node_t *a = *(const mkr_xml_node_t *const *)pa;
    const mkr_xml_node_t *b = *(const mkr_xml_node_t *const *)pb;
    int c = mkr_slice_cmp(a->ns_uri, a->ns_uri_len, b->ns_uri, b->ns_uri_len);
    return c != 0 ? c : mkr_slice_cmp(a->local, a->local_len, b->local, b->local_len);
}

/* A renderable namespace binding. All pointers are arena slices (stable for the
 * document's lifetime, GC-irrelevant): the C14N walk never builds Ruby objects. */
typedef struct { const char *prefix; uint32_t plen; const char *uri; uint32_t ulen; } mkr_c14n_ns_t;

static int
mkr_c14n_ns_cmp(const void *pa, const void *pb)     /* by prefix (default "" first) */
{
    const mkr_c14n_ns_t *a = pa, *b = pb;
    return mkr_slice_cmp(a->prefix, a->plen, b->prefix, b->plen);
}


/* Nearest in-scope binding for +prefix+ at or above +node+ (walks the real tree;
 * no scope dictionary is threaded). */
static int
mkr_c14n_nearest(const mkr_xml_node_t *node, const char *prefix, uint32_t plen,
                 const char **uri, uint32_t *ulen)
{
    for (const mkr_xml_node_t *e = node; e != NULL; e = e->parent) {
        if (e->type != MKR_XML_NODE_TYPE_ELEMENT) continue;
        for (mkr_xml_node_t *a = e->attrs; a != NULL; a = a->next) {
            const char *p, *u; uint32_t pl, ul;
            if (mkr_xml_xmlns_decl(a, &p, &pl, &u, &ul) && pl == plen
                && (pl == 0 || memcmp(p, prefix, pl) == 0)) {
                *uri = u; *ulen = ul; return 1;
            }
        }
    }
    return 0;
}

static int
mkr_c14n_has_prefix(const mkr_c14n_ns_t *arr, size_t n, const char *p, uint32_t pl)
{
    for (size_t i = 0; i < n; i++)
        if (arr[i].plen == pl && (pl == 0 || memcmp(arr[i].prefix, p, pl) == 0)) return 1;
    return 0;
}

/* The namespace declarations to render at +n+ (Inclusive C14N 1.0). The apex
 * renders every in-scope namespace (walking ancestors, nearest binding winning);
 * a descendant renders only its OWN xmlns declarations that change the inherited
 * binding. Writes a heap array (the caller frees) sorted by prefix; returns the
 * count, or SIZE_MAX on OOM. */
static size_t
mkr_c14n_namespaces(const mkr_xml_node_t *n, int is_apex, mkr_c14n_ns_t **out)
{
    *out = NULL;
    size_t cap = 0;
    for (const mkr_xml_node_t *e = n; e != NULL; e = e->parent) {
        if (e->type == MKR_XML_NODE_TYPE_ELEMENT) {
            for (mkr_xml_node_t *a = e->attrs; a != NULL; a = a->next) {
                const char *p, *u; uint32_t pl, ul;
                if (mkr_xml_xmlns_decl(a, &p, &pl, &u, &ul)) cap++;
            }
        }
        if (!is_apex) break;   /* a descendant considers only its own declarations */
    }
    if (cap == 0) return 0;
    mkr_c14n_ns_t *arr = mkr_reallocarray(NULL, cap, sizeof(*arr));
    if (arr == NULL) return SIZE_MAX;
    size_t cnt = 0;
    int default_seen = 0;
    for (const mkr_xml_node_t *e = n; e != NULL; e = e->parent) {
        if (e->type == MKR_XML_NODE_TYPE_ELEMENT) {
            for (mkr_xml_node_t *a = e->attrs; a != NULL; a = a->next) {
                const char *p, *u; uint32_t pl, ul;
                if (!mkr_xml_xmlns_decl(a, &p, &pl, &u, &ul)) continue;
                if (pl == 3 && memcmp(p, "xml", 3) == 0) continue;             /* implicit xml: */
                if (is_apex) {
                    if (pl == 0) {                /* default: nearest decl wins */
                        if (default_seen) continue;
                        default_seen = 1;
                        if (ul == 0) continue;    /* nearest default is xmlns="" -> not rendered */
                    } else if (mkr_c14n_has_prefix(arr, cnt, p, pl)) {
                        continue;
                    }
                } else {                          /* descendant: only changes to the binding */
                    const char *au; uint32_t aul;
                    int above = mkr_c14n_nearest(e->parent, p, pl, &au, &aul);
                    if (above && aul == ul && (ul == 0 || memcmp(au, u, ul) == 0)) continue; /* superfluous */
                    if (pl == 0 && ul == 0 && !(above && aul > 0)) continue; /* xmlns="" only undeclares */
                }
                arr[cnt].prefix = p; arr[cnt].plen = pl;
                arr[cnt].uri = u;    arr[cnt].ulen = ul;
                cnt++;
            }
        }
        if (!is_apex) break;
    }
    qsort(arr, cnt, sizeof(*arr), mkr_c14n_ns_cmp);
    *out = arr;
    return cnt;
}

static int
mkr_c14n_node(mkr_buf_t *b, const mkr_xml_node_t *n, int is_apex, int comments)
{
    switch (n->type) {
    case MKR_XML_NODE_TYPE_ELEMENT: {
        MKR_XSER_LIT(b, "<");
        MKR_XSER_APPEND(b, n->qname, n->qname_len);

        /* namespace declarations, prefix-sorted (a heap array of arena slices;
         * freed before returning, so no buffer macro may early-return past it). */
        mkr_c14n_ns_t *ns = NULL;
        size_t nn = mkr_c14n_namespaces(n, is_apex, &ns);
        if (nn == (size_t)-1) return -1;
        for (size_t i = 0; i < nn; i++) {
            int ok;
            if (ns[i].plen == 0) {
                ok = mkr_buf_append(b, " xmlns=\"", 8) == MKR_OK;
            } else {
                ok = mkr_buf_append(b, " xmlns:", 7) == MKR_OK
                     && mkr_buf_append(b, ns[i].prefix, ns[i].plen) == MKR_OK
                     && mkr_buf_append(b, "=\"", 2) == MKR_OK;
            }
            if (ok) ok = mkr_c14n_escaped(b, ns[i].uri, ns[i].ulen, 1) == 0;
            if (ok) ok = mkr_buf_append(b, "\"", 1) == MKR_OK;
            if (!ok) { free(ns); return -1; }
        }
        free(ns);

        /* attributes (non-xmlns) sorted by (namespace-uri, local-name) */
        size_t na = 0;
        for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
            const char *p, *u; uint32_t pl, ul;
            if (!mkr_xml_xmlns_decl(a, &p, &pl, &u, &ul)) na++;
        }
        if (na > 0) {
            const mkr_xml_node_t **av = mkr_reallocarray(NULL, na, sizeof(*av));
            if (av == NULL) return -1;
            size_t k = 0;
            for (mkr_xml_node_t *a = n->attrs; a != NULL; a = a->next) {
                const char *p, *u; uint32_t pl, ul;
                if (!mkr_xml_xmlns_decl(a, &p, &pl, &u, &ul)) av[k++] = a;
            }
            qsort(av, na, sizeof(*av), mkr_c14n_attr_cmp);
            for (size_t i = 0; i < na; i++) {
                int ok = mkr_buf_append(b, " ", 1) == MKR_OK
                         && mkr_buf_append(b, av[i]->qname, av[i]->qname_len) == MKR_OK
                         && mkr_buf_append(b, "=\"", 2) == MKR_OK;
                if (ok) ok = mkr_c14n_escaped(b, av[i]->value ? av[i]->value : "", av[i]->value_len, 1) == 0;
                if (ok) ok = mkr_buf_append(b, "\"", 1) == MKR_OK;
                if (!ok) { free(av); return -1; }
            }
            free(av);
        }

        MKR_XSER_LIT(b, ">");
        for (mkr_xml_node_t *c = n->first_child; c != NULL; c = c->next) {
            if (mkr_c14n_node(b, c, 0, comments) != 0) return -1;  /* children are not the apex */
        }
        MKR_XSER_LIT(b, "</");
        MKR_XSER_APPEND(b, n->qname, n->qname_len);
        MKR_XSER_LIT(b, ">");
        return 0;
    }
    case MKR_XML_NODE_TYPE_TEXT:
    case MKR_XML_NODE_TYPE_CDATA_SECTION:    /* CDATA canonicalizes to escaped text */
        return mkr_c14n_escaped(b, n->value ? n->value : "", n->value_len, 0);
    case MKR_XML_NODE_TYPE_COMMENT:
        if (comments) {
            MKR_XSER_LIT(b, "<!--");
            MKR_XSER_APPEND(b, n->value ? n->value : "", n->value_len);
            MKR_XSER_LIT(b, "-->");
        }
        return 0;
    case MKR_XML_NODE_TYPE_PI:
        MKR_XSER_LIT(b, "<?");
        MKR_XSER_APPEND(b, n->local, n->local_len);
        if (n->value_len) { MKR_XSER_LIT(b, " "); MKR_XSER_APPEND(b, n->value, n->value_len); }
        MKR_XSER_LIT(b, "?>");
        return 0;
    default:
        return 0;
    }
}

/* call-seq: node.canonicalize(comments: false) -> String
 * Inclusive Canonical XML 1.0 (UTF-8). A Document canonicalizes its element +
 * top-level PIs (and comments when requested); any other node canonicalizes its
 * subtree, with the apex inheriting the ancestors' in-scope namespaces. */
static VALUE
mkr_xml_node_canonicalize(int argc, VALUE *argv, VALUE self)
{
    VALUE opts;
    rb_scan_args(argc, argv, "0:", &opts);
    int comments = !NIL_P(opts) && RTEST(rb_hash_aref(opts, ID2SYM(rb_intern("comments"))));

    mkr_xml_node_t *n = mkr_xml_node_unwrap(self);
    mkr_buf_t buf;
    mkr_buf_init(&buf, 0);
    int rc = 0;

    if (rb_obj_is_kind_of(self, mkr_cXmlDocument)) {
        /* §2.4: a PI/comment before the document element is followed by #xA, one
         * after is preceded by #xA; the element itself has no surrounding line. */
        int seen_root = 0;
        for (mkr_xml_node_t *c = n->first_child; rc == 0 && c != NULL; c = c->next) {
            if (c->type == MKR_XML_NODE_TYPE_ELEMENT) {
                rc = mkr_c14n_node(&buf, c, 1, comments);   /* the root element is the apex */
                seen_root = 1;
            } else if (c->type == MKR_XML_NODE_TYPE_PI ||
                       (c->type == MKR_XML_NODE_TYPE_COMMENT && comments)) {
                if (seen_root && rc == 0) rc = (mkr_buf_append(&buf, "\n", 1) == MKR_OK) ? 0 : -1;
                if (rc == 0) rc = mkr_c14n_node(&buf, c, 0, comments);
                if (!seen_root && rc == 0) rc = (mkr_buf_append(&buf, "\n", 1) == MKR_OK) ? 0 : -1;
            }
        }
    } else {
        rc = mkr_c14n_node(&buf, n, 1, comments);  /* the node is the apex (inherits ancestors' ns) */
    }

    if (rc != 0) {
        mkr_buf_free(&buf);
        rb_raise(mkr_eError, "out of memory canonicalizing XML");
    }
    VALUE str = rb_utf8_str_new(buf.len ? buf.data : "", (long)buf.len);
    mkr_buf_free(&buf);
    return str;
}

/* ---- fail-closed guards for the unsupported query / serialization surface ----
 *
 * Makiri::XML is a strict, read-only reader (§7.5 / §12). Two HTML-node
 * conveniences would silently misbehave on XML and are turned into explicit
 * NotImplementedError instead of a wrong result:
 *   - CSS selectors: Lexbor's lxb_selectors lower-cases names, which destroys
 *     XML's case- and namespace-sensitive matching — use #xpath.
 *   - HTML serialization (to_html/inner_html/outer_html): emitting
 *     HTML-serialized markup for an XML document would be wrong (escaping /
 *     CDATA / void elements differ). Use #to_xml. */
static VALUE
mkr_xml_node_no_css(int argc, VALUE *argv, VALUE self)
{
    (void)argc; (void)argv; (void)self;
    rb_raise(rb_eNotImpError,
             "Makiri::XML does not support CSS selectors; use #xpath. "
             "(CSS cannot preserve XML element-name case or namespaces.) "
             "Default-namespace documents (RSS/Atom) need a registered prefix, "
             "e.g. doc.xpath(\"//a:entry\", \"a\" => \"http://www.w3.org/2005/Atom\").");
}

static VALUE
mkr_xml_node_no_serialize(int argc, VALUE *argv, VALUE self)
{
    (void)argc; (void)argv; (void)self;
    rb_raise(rb_eNotImpError,
             "Makiri::XML does not HTML-serialize (to_html / inner_html / "
             "outer_html); use #to_xml for XML output.");
}

/* ---- node identity (== / eql? / hash / pointer_id) ----
 *
 * XML nodes share the mkr_node_data_t typed-data with HTML nodes, so the
 * underlying node pointer (mkr_node_unwrap, representation-agnostic) IS the
 * identity. Without these, two wrappers for the same XML node compared unequal
 * (Object identity), which broke #path, NodeSet/Set dedup, and Hash keys.
 * Same contract as the HTML nodes (ruby_node.c): a.pointer_id == b.pointer_id
 * iff a.eql?(b). */
static VALUE
mkr_xml_node_pointer_id(VALUE self)
{
    return ULL2NUM((unsigned long long)(uintptr_t)mkr_node_unwrap(self));
}

static VALUE
mkr_xml_node_equals(VALUE self, VALUE other)
{
    if (!rb_obj_is_kind_of(other, mkr_cNode)) return Qfalse;
    return mkr_node_unwrap(self) == mkr_node_unwrap(other) ? Qtrue : Qfalse;
}

void
mkr_init_xml_node(void)
{
    rb_define_method(mkr_mXmlNodeMethods, "name",          mkr_xml_node_name, 0);
    rb_define_method(mkr_mXmlNodeMethods, "local_name",    mkr_xml_node_local_name, 0);
    rb_define_method(mkr_mXmlNodeMethods, "prefix",        mkr_xml_node_prefix, 0);
    rb_define_method(mkr_mXmlNodeMethods, "namespace_uri", mkr_xml_node_namespace_uri, 0);
    rb_define_method(mkr_mXmlNodeMethods, "node_type",     mkr_xml_node_node_type, 0);

    /* namespace introspection (Nokogiri-compatible) + the Namespace value object */
    mkr_cXmlNamespace = rb_define_class_under(mkr_mXML, "Namespace", rb_cObject);
    rb_define_method(mkr_cXmlNamespace, "prefix",  mkr_ns_prefix, 0);
    rb_define_method(mkr_cXmlNamespace, "href",    mkr_ns_href, 0);
    rb_define_method(mkr_cXmlNamespace, "to_s",    mkr_ns_href, 0);
    rb_define_method(mkr_cXmlNamespace, "==",      mkr_ns_equal, 1);
    rb_define_method(mkr_cXmlNamespace, "eql?",    mkr_ns_equal, 1);
    rb_define_method(mkr_cXmlNamespace, "hash",    mkr_ns_hash, 0);
    rb_define_method(mkr_cXmlNamespace, "inspect", mkr_ns_inspect, 0);
    rb_define_method(mkr_mXmlNodeMethods, "namespace",     mkr_xml_node_namespace, 0);
    rb_define_method(mkr_mXmlNodeMethods, "namespace_definitions", mkr_xml_node_namespace_definitions, 0);
    rb_define_method(mkr_mXmlNodeMethods, "namespaces",    mkr_xml_node_namespaces, 0);
    rb_define_method(mkr_mXmlNodeMethods, "collect_namespaces", mkr_xml_node_collect_namespaces, 0);
    rb_define_method(mkr_mXmlNodeMethods, "content",       mkr_xml_node_content, 0);
    rb_define_method(mkr_mXmlNodeMethods, "text",          mkr_xml_node_content, 0);
    rb_define_method(mkr_mXmlNodeMethods, "inner_text",    mkr_xml_node_content, 0);
    rb_define_method(mkr_mXmlNodeMethods, "value",         mkr_xml_node_value, 0);
    rb_define_method(mkr_mXmlNodeMethods, "document",      mkr_xml_node_get_document, 0);
    rb_define_method(mkr_mXmlNodeMethods, "parent",        mkr_xml_node_parent, 0);
    rb_define_method(mkr_mXmlNodeMethods, "next",          mkr_xml_node_next, 0);
    rb_define_method(mkr_mXmlNodeMethods, "next_sibling",  mkr_xml_node_next, 0);
    rb_define_method(mkr_mXmlNodeMethods, "previous",      mkr_xml_node_previous, 0);
    rb_define_method(mkr_mXmlNodeMethods, "previous_sibling", mkr_xml_node_previous, 0);
    rb_define_method(mkr_mXmlNodeMethods, "child",         mkr_xml_node_first_child, 0);
    rb_define_method(mkr_mXmlNodeMethods, "last_element_child", mkr_xml_node_last_child, 0);
    rb_define_method(mkr_mXmlNodeMethods, "children",      mkr_xml_node_children, 0);
    rb_define_method(mkr_mXmlNodeMethods, "[]",            mkr_xml_node_aref, 1);
    rb_define_method(mkr_mXmlNodeMethods, "attribute_nodes", mkr_xml_node_attribute_nodes, 0);

    /* Node identity by underlying pointer, so #path / NodeSet dedup / Set / Hash
     * work (the same contract HTML nodes have). */
    rb_define_method(mkr_mXmlNodeMethods, "==",         mkr_xml_node_equals, 1);
    rb_define_method(mkr_mXmlNodeMethods, "eql?",       mkr_xml_node_equals, 1);
    rb_define_method(mkr_mXmlNodeMethods, "hash",       mkr_xml_node_pointer_id, 0);
    rb_define_method(mkr_mXmlNodeMethods, "pointer_id", mkr_xml_node_pointer_id, 0);

    /* XML serialization: #to_xml / #to_s re-emit the subtree (a Document also
     * emits the declaration + DOCTYPE). Also defined on XML::Document below. */
    rb_define_method(mkr_mXmlNodeMethods, "to_xml",     mkr_xml_node_to_xml, -1);
    rb_define_method(mkr_mXmlNodeMethods, "to_s",       mkr_xml_node_to_xml, -1);
    rb_define_method(mkr_mXmlNodeMethods, "canonicalize", mkr_xml_node_canonicalize, -1);

    /* Fail-closed: CSS + HTML serialization are unsupported on XML; raise rather
     * than return a wrong result (these shadow nothing on XML nodes otherwise). */
    rb_define_method(mkr_mXmlNodeMethods, "css",        mkr_xml_node_no_css, -1);
    rb_define_method(mkr_mXmlNodeMethods, "at_css",     mkr_xml_node_no_css, -1);
    rb_define_method(mkr_mXmlNodeMethods, "to_html",    mkr_xml_node_no_serialize, -1);
    rb_define_method(mkr_mXmlNodeMethods, "inner_html", mkr_xml_node_no_serialize, -1);
    rb_define_method(mkr_mXmlNodeMethods, "outer_html", mkr_xml_node_no_serialize, -1);

    /* Makiri::XML::DTD identifiers (Nokogiri-compatible; #public_id is the
     * WHATWG-DOM alias of #external_id). #name comes from the shared reader. */
    rb_define_method(mkr_cXmlDTD, "external_id", mkr_xml_dtd_external_id, 0);
    rb_define_method(mkr_cXmlDTD, "public_id",   mkr_xml_dtd_external_id, 0);
    rb_define_method(mkr_cXmlDTD, "system_id",   mkr_xml_dtd_system_id, 0);
}
