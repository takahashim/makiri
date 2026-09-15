//! Baseline: what does the CURRENT DomIndex build cost, in Rust, per document size?
use std::time::Instant;

fn html(articles: usize) -> Vec<u8> {
    let mut h =
        String::from("<html><body><div id='main' class='container'><section class='content'>");
    for i in 0..articles {
        let cls = if i % 2 == 0 { "post featured" } else { "post" };
        h.push_str(&format!(
            "<article id='post-{i}' class='{cls}' data-author='a{}'>\
             <h2 class='title'>T{i}</h2><p class='body'>B{i}</p>\
             <a href='/p/{i}'>more</a></article>",
            i % 7
        ));
    }
    h.push_str("</section></div></body></html>");
    h.into_bytes()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    for articles in [400usize, 2000] {
        let src = html(articles);
        // parse cost
        let mut parse_ms = Vec::new();
        for _ in 0..20 {
            let t = Instant::now();
            let p = unsafe {
                makiri::dom_adapter::post_parse::mkr_parse_html(src.as_ptr(), src.len(), true)
            };
            parse_ms.push(t.elapsed().as_secs_f64() * 1000.0);
            unsafe { makiri::dom_adapter::post_parse::mkr_parsed_destroy(p) };
        }
        // index build cost, on a freshly parsed doc each time (build is cached per handle)
        let mut build_ms = Vec::new();
        for _ in 0..20 {
            let p = unsafe {
                makiri::dom_adapter::post_parse::mkr_parse_html(src.as_ptr(), src.len(), true)
            };
            let t = Instant::now();
            let rc = unsafe { makiri::dom_adapter::dom_index::mkr_parsed_dom_index_build(p) };
            build_ms.push(t.elapsed().as_secs_f64() * 1000.0);
            assert!(rc);
            unsafe { makiri::dom_adapter::post_parse::mkr_parsed_destroy(p) };
        }
        println!("{articles} articles ({} bytes)", src.len());
        println!("  parse            {:8.3} ms", median(parse_ms));
        println!(
            "  DomIndex build   {:8.3} ms  (tag CSR + attr->owner, current)",
            median(build_ms)
        );
    }
}
