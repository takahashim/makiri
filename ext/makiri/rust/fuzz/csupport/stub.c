/* HTML-side engine stubs, mirroring verify/stub.c.
 *
 * The XPath dispatcher references both monomorphizations; these fuzz targets
 * link only the XML instance and every one of them pins the context's engine
 * kind to XML, so the HTML entries are unreachable. abort() rather than a
 * silent return, so a wrong engine-kind wiring fails loudly instead of quietly
 * fuzzing nothing. */
#include <stdlib.h>

int mkr_eval_ast_html(void)        { abort(); }
int mkr_try_first_match_html(void) { abort(); }
