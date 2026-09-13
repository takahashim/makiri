//! Link probe: can a Ruby-free executable target link the vendored Lexbor?
fn main() {
    let html = b"<html><body><p id='a' class='x y'>hi</p></body></html>";
    unsafe {
        let p = makiri::dom_adapter::post_parse::mkr_parse_html(html.as_ptr(), html.len(), true);
        println!("parsed handle null? {}", p.is_null());
        if !p.is_null() {
            let built = makiri::dom_adapter::dom_index::mkr_parsed_dom_index_build(p);
            println!("index build status: {built}");
            makiri::dom_adapter::post_parse::mkr_parsed_destroy(p);
        }
    }
}
