#ifndef MAKIRI_H
#define MAKIRI_H

#include <ruby.h>
#include <ruby/encoding.h>

#include <lexbor/html/html.h>
#include <lexbor/dom/dom.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Ruby module + class refs, populated by Init_makiri. */
extern VALUE mkr_mMakiri;
extern VALUE mkr_cNode;
extern VALUE mkr_cDocument;
extern VALUE mkr_cElement;
extern VALUE mkr_cAttribute;
extern VALUE mkr_cText;
extern VALUE mkr_cComment;
extern VALUE mkr_cCData;
extern VALUE mkr_cProcessingInstruction;
extern VALUE mkr_cDocumentType;
extern VALUE mkr_cDocumentFragment;
extern VALUE mkr_cNodeSet;
extern VALUE mkr_cXPathContext;
extern VALUE mkr_mXPath;
extern VALUE mkr_mCSS;
extern VALUE mkr_eError;
extern VALUE mkr_eXPathSyntaxError;
extern VALUE mkr_eXPathLimitExceeded;
extern VALUE mkr_eCSSSyntaxError;

/* Forward-declared sub-init entry points. Each sub-component owns its
 * own Init_* and registers methods on the class refs above. */
void mkr_init_document(void);
void mkr_init_node(void);
void mkr_init_node_set(void);
void mkr_init_xpath(void);
void mkr_init_css(void);
void mkr_init_serialize(void);
void mkr_init_mutate(void);

/* Enforce Makiri's text-input contract on a programmatic-API String argument:
 * it must be valid UTF-8 (bytes validated regardless of the declared encoding)
 * and must not contain a NUL byte. Raises Makiri::Error otherwise (never
 * truncates/repairs). +str+ must already be a String; +what+ names the input
 * for the message (e.g. "CSS selector"). Used at the XPath/CSS/DOM-mutation
 * boundaries; HTML parsing instead decodes leniently (see mkr_utf8_sanitize). */
void mkr_check_text(VALUE str, const char *what);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_H */
