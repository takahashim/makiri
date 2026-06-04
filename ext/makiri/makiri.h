#ifndef MAKIRI_H
#define MAKIRI_H

#include <ruby.h>
#include <ruby/encoding.h>

#include <lexbor/html/html.h>
#include <lexbor/dom/dom.h>

#include "bridge/bridge.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Ruby module + class refs, populated by Init_makiri. */
extern VALUE mkr_mMakiri;
/* Abstract bases (§12): Node and the per-type bases. Concrete nodes are the
 * per-kind leaves below; `is_a?(Makiri::Element)` etc. stays true for both. */
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

/* Makiri::HTML module + leaves. mkr_mHtmlNodeMethods (Makiri::HTML::NodeMethods)
 * is a behavior module holding the lxb_dom-backed reader/query methods, included
 * into every HTML leaf (so an XML node never inherits an HTML reader).
 * mkr_cHtmlNode (Makiri::HTML::Node) is the concrete generic HTML node — the
 * wrap fallback for an uncommon DOM node type that has no more specific leaf
 * (it carries the readers but is NOT classified as an Element). */
extern VALUE mkr_mHTML;
extern VALUE mkr_mHtmlNodeMethods;
extern VALUE mkr_cHtmlNode;
extern VALUE mkr_cHtmlDocument;
extern VALUE mkr_cHtmlElement;
extern VALUE mkr_cHtmlAttribute;
extern VALUE mkr_cHtmlText;
extern VALUE mkr_cHtmlComment;
extern VALUE mkr_cHtmlCData;
extern VALUE mkr_cHtmlProcessingInstruction;
extern VALUE mkr_cHtmlDocumentType;
extern VALUE mkr_cHtmlDocumentFragment;

/* Makiri::XML leaves + the XML reader behavior module (the custom-node
 * counterpart of mkr_mHtmlNodeMethods). */
extern VALUE mkr_mXmlNodeMethods;
extern VALUE mkr_cXmlNode;
extern VALUE mkr_cXmlDocument;
extern VALUE mkr_cXmlElement;
extern VALUE mkr_cXmlAttribute;
extern VALUE mkr_cXmlText;
extern VALUE mkr_cXmlComment;
extern VALUE mkr_cXmlCData;
extern VALUE mkr_cXmlProcessingInstruction;

void mkr_init_xml_node(void);
extern VALUE mkr_cNodeSet;
extern VALUE mkr_cXPathContext;
extern VALUE mkr_mXPath;
extern VALUE mkr_mCSS;
extern VALUE mkr_mXML;
extern VALUE mkr_eError;
extern VALUE mkr_eXPathSyntaxError;
extern VALUE mkr_eXPathLimitExceeded;
extern VALUE mkr_eCSSSyntaxError;
extern VALUE mkr_eXmlSyntaxError;
extern VALUE mkr_eXmlLimitExceeded;

/* Forward-declared sub-init entry points. Each sub-component owns its
 * own Init_* and registers methods on the class refs above. */
void mkr_init_document(void);
void mkr_init_node(void);
void mkr_init_node_set(void);
void mkr_init_xpath(void);
void mkr_init_css(void);
void mkr_init_serialize(void);
void mkr_init_mutate(void);
void mkr_init_xml(void);

#ifdef __cplusplus
}
#endif

#endif /* MAKIRI_H */
