/* HTML-side engine stubs for the XML carve-out (docs/formal_verification.ja.md §3).
 *
 * The XPath dispatcher (mkr_xpath.c) references both monomorphizations; the
 * verification builds link only the XML instance, and every harness pins the
 * context's engine kind to XML, so the HTML entries are unreachable. abort()
 * (not a silent return) so a wrong engine-kind wiring fails loudly. */
#include <stdlib.h>

int mkr_eval_ast_html(void)        { abort(); }
int mkr_try_first_match_html(void) { abort(); }
