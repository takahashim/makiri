# HTML パーサの差分表

本書は、Makiri の HTML パーサと、対象とする仕様およびオラクルとの食い違いを記録する。
セレクタパーサの移植（第 5 段）で生じた差も、同じ分類でここに載せる。
移植計画（`docs/rewrite_parser.ja.md`）の「固定するもの」で定めた表であり、移植の各段はこの表を根拠に「直す差」と「保つ差」を区別する。

## 対象とする版

三つの版は同時に進め、進めたときは表の全項目を見直す。

| 対象 | commit | 日付 | 用途 |
|---|---|---|---|
| HTML Standard（whatwg/html の `source`） | `de953a7cf0523f5873a1904ba2ce010c689dfb66` | 2026-10-16 | 一次の根拠 |
| WPT（web-platform-tests/wpt） | `5ffee20edef3bd97fad8ab03c5e2108c2f48acec` | 2026-10-18 取得 | tree construction（`html/syntax/parsing/resources/*.dat`） |
| html5lib-tests | `c777c408b61078ea2eb4acefc2535f54dbc8b28a` | 2026-10-01 | tokenizer（`tokenizer/*.test`） |
| Lexbor（参照実装） | `05b5d37`（`v3.0.0-66-g05b5d37`） | | 差分オラクル |

html5lib-tests は 2026-06 に tree construction のテストを WPT に移しており、上の html5lib-tests の commit には `tree-construction/` がない。
tree construction は WPT から、tokenizer は html5lib-tests から取る。
`spec/conformance/html5lib_runner.rb` の `DATA_COMMIT` は WPT の pin と一致させる。

## 分類

食い違いは次の四つに分類する。

- **仕様変更**：固定した版の仕様が答えを定めており、テストデータか参照実装が古い。
- **参照実装の未対応**：Lexbor またはブラウザが、固定した版の仕様を実装していない。
- **Makiri の不具合**：直す。移植の初期実装の間は、参照実装との食い違いはすべてこれに当たる。
- **意図した互換差**：Nokogiri や Lexbor に合わせると決めた差。

各項目は、分類、仕様の該当箇所、再現する入力、現在の扱い、移植計画の中で入れる段を持つ。

「監査の」で始まる参照（`in_body F1`、`css_selectors F7` など）は、第 8 段の監査の表の行を指す。表は step ごとの対応を記録した作業記録で、結果をこの表と `compat.rs` に移したあと削除した。git の履歴の `docs/parser_audit/` にある。

## パーサの項目

P で始まる項目は HTML パーサの振る舞いである。
いずれも `spec/conformance/parser_known_divergences.txt` に同じ id で載っており、Ruby の runner と crate の runner は該当テストを失敗とせずに報告し、通るようになったら失敗する。三つ目の欄は、まだ食い違うパーサ（`lexbor`、`port`）を示す。移植版で直した項目は `lexbor` だけに残る。

移植版の直し方は、`compat.rs` の `fixed` を問う分岐として入れる。出荷するビルドは常に仕様どおりに進み、比較のテストと fuzz は Lexbor 互換の実行（`as_lexbor`）で Lexbor との完全一致を保ったうえで、仕様どおりの実行との差を `explains` の述語が説明できる入力でだけ許す。

### P1 暗黙に body を挿入したときの frameset-ok の再設定

- **分類**：参照実装の未対応。移植版で直した（第 7 段）。
- **仕様**：tree construction の「after head」insertion mode の anything else。body 要素を暗黙に挿入するとき、frameset-ok flag を "ok" に戻す。whatwg/html#12908（2026-09-07 merge、issue #3178）で入った。
- **再現**：`<head><template></template></head><p><frameset>`。仕様では frameset 文書になり、Lexbor は `<body><p>` を作る。`<template>` が "in head" で frameset-ok を "not ok" にしたまま、戻す処理がない。
- **テスト**：WPT `template.dat` #44、#45、#46。`spec/html_standard_steps_spec.rb`。
- **移植版**：`html/tree/modes/after_head.rs` の `anything_else` が frameset-ok を "ok" に戻す。差分の許可は `compat.rs` の `explains`（入力に `frameset` と `template` があるとき）。

### P2 form element pointer と「parsing template contents」

- **分類**：参照実装の未対応。移植版で直した（第 7 段）。Lexbor の DOM のビルドは旧来の答えのまま。
- **仕様**：「parsing template contents」は、open elements のスタックに template 要素があるか、fragment の context 要素が template 要素であるときに真になる。"in body" と "in table" の form 開始タグは、この条件で form element pointer を無視し、設定もしない。whatwg/html issue #12257（2026-09-07 close）で入った。
- **再現**：context が template の fragment で `<form><form>`。仕様では入れ子の form が二つでき、Lexbor は二つ目を捨てる。Lexbor は旧来の「スタックに template 要素があるか」だけを見ており、context の template を数えない。"in table" の form 開始タグも旧来の規則のままで、`<template><table><form></table></template>` の form を捨てる。
- **テスト**：WPT `template.dat` #117、#120、#124。`spec/html_standard_steps_spec.rb`。
- **移植版**：`TreeBuilder::form_in_template`（`html/tree/mod.rs`）が「parsing template contents」を判定し、"in body" の form の開始タグと終了タグ、"in table" の form の開始タグがそれを使う。"in table" では、template の中の form を捨てずに、form element pointer なしで挿入する。終了タグも同じ判定に従う（#124 は、template を context にした fragment の `</form>` が form を閉じることを確かめる）。差分の許可は、入力に `form` があり、`template` があるか context が template のとき。

### P3 foreign content の U+FFFD と frameset-ok

- **分類**：参照実装の未対応。移植版で直した（第 7 段）。Lexbor の DOM のビルドは旧来の答えのまま。
- **仕様**：foreign content の「Any other character token」は frameset-ok flag を "not ok" にする。U+0000 を置き換えた U+FFFD は別の規則（「A character token that is U+0000 NULL」）で扱われ、flag を変えない。入力に現れた U+FFFD（文字参照で書かれたものを含む）は「Any other character token」に当たる。
- **再現**：`<!doctype html><svg>&#xFFFD;</svg><frameset><frame>`。仕様では frameset が無視され、Lexbor は frameset 文書を作る。Lexbor の foreign content の文字処理は、NUL の置換で生じた U+FFFD を除外するつもりで、文字列中のすべての U+FFFD を空白と同じく読み飛ばす。"in body" の文字処理にはこの除外がなく、仕様どおりである。
- **テスト**：WPT `tests19.dat` #51、`plain-text-unsafe.dat` #24。`spec/html_standard_steps_spec.rb`。
- **移植版**：`html/tree/modes/foreign_content.rs` の `text` が、U+0000 を置き換える前の文字列で frameset-ok を判定する（U+0000 と空白以外の文字があれば "not ok"）。差分の許可は、入力に `frameset` と `svg` か `math` があり、U+FFFD、`&`（文字参照）、UTF-8 でない byte のどれかがあるとき。

### P4 "in table" の文字 token と template

- **分類**：参照実装の未対応（監査で見つけた）。移植版で直した（第 8 段）。Lexbor の DOM のビルドは旧来の答えのまま。
- **仕様**："in table" の「A character token, if the current node is table, tbody, template, tfoot, thead, or tr element」。
- **再現**：`<template><tbody></tbody><p><b></p> </template>`。仕様では空白は "in table text" から template の内容に入り、Lexbor は "in body" で処理して `<b>` を再構築し、その中に入れる。
- **移植版**：`html/tree/modes/in_table.rs` の `text_open` が template を一覧に含める。差分の許可は入力に `template` があるとき。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P5 "in column group" の DOCTYPE token

- **分類**：参照実装の未対応（監査で見つけた）。移植版で直した。
- **仕様**："in column group" の「A DOCTYPE token: Parse error. Ignore the token.」。
- **再現**：`<table><colgroup><!doctype html><col></table>`。仕様では colgroup は一つで col を含み、Lexbor は DOCTYPE で colgroup を閉じて二つ目を作る。
- **移植版**：`html/tree/modes/in_column_group.rs` に DOCTYPE の分岐を足した。差分の許可は入力に `doctype` があり、`col` があるか context が colgroup のとき。
- **テスト**：WPT `domjs-unsafe.dat` #34 はこの分岐を通るが、次が `</colgroup>` なので差が出ない。`spec/html_standard_steps_spec.rb`。

### P6 "in column group" の空白と、それ以外の文字を含む文字 token

- **分類**：参照実装の未対応（監査で見つけた）。移植版で直した。
- **仕様**：文字 token は一文字ずつで、空白は「Insert the character」、それ以外は「Anything else」（current node が colgroup でなければ parse error として無視）。
- **再現**：`<template><col>a b</template>`、context を colgroup にした fragment の `a b<col>`。仕様では `b` の前の空白が残り、Lexbor は先頭の空白の後ろを token ごと捨てる。current node が colgroup の場合は、残りを "in table" に渡すので仕様と同じになる。
- **移植版**：`html/tree/modes/in_column_group.rs` が、colgroup のないとき残りの空白を挿入する。差分の許可は context が colgroup のときか、入力に `col` と `template` があるとき。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P7 "after after frameset" の空白と、それ以外の文字を含む文字 token

- **分類**：参照実装の未対応（監査で見つけた）。移植版で直した。
- **仕様**：空白の文字は "in body" の規則で処理し（active formatting elements の再構築と挿入）、それ以外は parse error として無視する。
- **再現**：`<frameset></frameset></html> x `。仕様では html の下に空白二つの text ができ、Lexbor は token ごと捨てる（"after frameset" は仕様どおり）。
- **移植版**：`html/tree/modes/after_after_frameset.rs` が空白だけを "in body" の規則で挿入する。差分の許可は入力に `frameset` があるとき。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P8 `[CDATA[` の途中で終わる入力

- **分類**：参照実装の未対応（監査で見つけた）。移植版で直した（第 8 段）。
- **仕様**：markup declaration open state の「Anything else」で空のコメントを作り、bogus comment state が読んだ文字をそのデータにする。
- **再現**：`<![CDA`。仕様ではデータが `[CDA` のコメント、Lexbor は空のコメント。`<![`、`<![cd`、`<svg><![CDA` も同じ。Lexbor の DOCTYPE の側（`<!DOC`）は読んだ文字を残しており、CDATA の側だけが捨てる。
- **移植版**：`html/tokenizer/state.rs` の `markup_declaration_cdata` が、途中まで一致した文字をバッファに残す。差分の許可は、入力の最後の `<!` の後に `>` がないとき。
- **テスト**：html5lib と WPT にない。`spec/html_standard_steps_spec.rb`。

### P9 入力の末尾の `<!`

- **分類**：参照実装の未対応（監査で見つけた）。移植版で直した。
- **仕様**：markup declaration open state で入力が終わると、空のコメントを作る。
- **再現**：`x<!`。仕様では `x` の text と空のコメント、Lexbor はコメントのデータにも `x` を入れる（直前の text のバッファを空にしないため）。
- **移植版**：`html/tokenizer/state.rs` の `markup_declaration_open` が、入力の終わりでバッファを空にする。差分の許可は P8 と同じ。
- **テスト**：html5lib と WPT にない。`spec/html_standard_steps_spec.rb`。

### P10 値のない属性と空の値（active formatting elements の「三つまで」）

- **分類**：参照実装の未対応（監査で三つの範囲から見つかった）。移植版で直した（第 8 段）。
- **仕様**：active formatting elements に要素を積むとき、同じタグ名、名前空間、属性（名前と値）を持つ要素が三つあれば最も古いものを除く。属性を値なしで書いたとき（`<b x>`）の値は空文字列で、`<b x="">` と同じである。
- **再現**：`<p><b x><b x><b x=""><b x=""></p>x`。仕様では最も古い `b` が除かれ、`x` の周りに三つの `b` が再構築される。Lexbor は値がないこと（NULL）と空の値を区別し、四つを再構築する。
- **移植版**：`html/tree/stacks.rs` の `attr_compare` が、値のない属性を空の値として比べる。値がないことの記録（`valueless`）は、直列化などほかの用途のために残す。差分の許可は、入力に引用符で囲んだ空の値（`""` か `''`）があるとき（空の値は引用符なしでは書けない）。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P13 `<input type=hidden>` の大文字小文字

- **分類**：参照実装の未対応（監査の in_body F3）。移植版で直した（第 8 段）。
- **仕様**："in body" の `input` 開始タグで、`type` 属性がないか、その値が "hidden" と ASCII の大文字小文字を区別せずに一致しないときに frameset-ok を "not ok" にする。
- **再現**：`<input type=HIDDEN><frameset>`。仕様では frameset-ok が変わらず、`<frameset>` が body を置き換える。Lexbor は値を正確に比べるので frameset-ok を "not ok" にし、`<frameset>` を無視する。
- **移植版**：`html/tree/modes/in_body.rs` の `input` が値を `eq_ignore_ascii_case` で比べる。差分の許可は、入力に `frameset` と `hidden` があるとき。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P15 foreign content を抜ける `<sup>`

- **分類**：参照実装の未対応（監査の foreign F1）。移植版で直した（第 8 段）。
- **仕様**：foreign content で、`b`、`big`、…、`sub`、`sup`、… の開始タグは parse error で、HTML の要素か MathML text integration point か HTML integration point に達するまで pop し、現在の insertion mode で処理し直す。
- **再現**：`<svg><sup>x</sup></svg>`。仕様では `sup` が `svg` の後ろの兄弟になる。Lexbor の一覧には `sup` がなく、`svg` の中の SVG 要素になる。
- **移植版**：`html/tree/modes/foreign_content.rs` の開始タグの一覧に `sup` を加える。差分の許可は、入力に `sup` と、`svg` か `math`（あるいはその context）があるとき。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P14 table の insertion mode の `<image>`

- **分類**：参照実装の未対応（監査の in_body F5）。移植版で直した（第 8 段）。
- **仕様**："in table" の anything else は foster parenting を有効にして "in body" の規則で処理する。"in body" の `image` 開始タグは tag 名を `img` に変えて再処理するので、`img` はもう一度 "in table" を通って foster parent される。
- **再現**：`<table><image>`。仕様では `<img>` が table の前に置かれる。Lexbor は "in body" の再処理の要求を捨てるので、`<image>` が消える。"in table body"、"in row"、"in column group" からも同じ。
- **移植版**：`html/tree/modes/in_table.rs` の `anything_else` が "in body" の答えを返す。差分の許可は、入力に `image` があるとき。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P12 `<textarea>` の後の "text" insertion mode

- **分類**：参照実装の未対応（監査の in_body F2）。移植版で直した（第 8 段）。
- **仕様**："in body" の `textarea` 開始タグは、次の LF を無視し、original insertion mode を current mode にして "text" insertion mode に切り替える。"text" の文字 token は active formatting elements を再構築せずに挿入する。
- **再現**：`<p><b></p><textarea>x</textarea>y`。仕様では `textarea` の子は文字列 `x` だけ。Lexbor は改行を落とす mode の後で元の mode（"in body" など）に戻るので、`b` を `textarea` の中に再構築する（`<textarea><b>x</b></textarea>`）。
- **移植版**：`html/tree/modes/in_body.rs` の `skip_new_line_textarea` が "text" に切り替える。差分の許可は、入力に `textarea` があるとき。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P11 adoption agency algorithm の bookmark

- **分類**：参照実装の未対応（監査の in_body F1）。移植版で直した（第 8 段）。
- **仕様**：bookmark は active formatting elements の中の「両隣の要素に対する位置」で、inner loop が要素を除いても動かない。step 19 で formatting element を除き、新しい要素を bookmark の位置に入れる。
- **再現**：`<em><s><b><div><strike><tt><nobr><a><div></s></div></div>x`。仕様では `</s>` の後の active formatting elements は em、b、tt、nobr、a で、`x` の周りに `<tt><nobr><a>` が再構築される。Lexbor は bookmark と formatting element の位置を添字で持ち、前の要素を除いても補正しないので、別の要素を除き、stack にない `s` を残す（`<s><nobr><a>x`）。
- **移植版**：`html/tree/actions.rs` の `adoption_agency_algorithm` が、inner loop で除いた要素より後ろにある bookmark と formatting element の添字を一つ戻し、step 19 で formatting element を除いた後の bookmark も補正する。parse5 との乱択の比較（formatting 要素と block を混ぜた 12,000 件）で一致した。差分の許可は、入力に formatting element の終了タグか、`<a>` か `<nobr>` の開始タグ（これも adoption agency algorithm を走らせる）があるとき。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P16 fragment の解析の終わりの pop

- **分類**：参照実装の未対応（監査の foreign F2）。移植版で直した（第 8 段）。
- **仕様**："the end" は stack of open elements のノードをすべて pop する。fragment の解析でも EOF の "stop parsing" から走り、`option` の pop steps が `select` の `selectedcontent` を更新する。
- **再現**：`Makiri::DocumentFragment.parse("<select><button><selectedcontent></selectedcontent></button><option>A")`。仕様では文書として解析したときと同じく `selectedcontent` が `A` を持つ。Lexbor の `lxb_html_parse_fragment_chunk_end` は pop しないので、空のまま残る。
- **移植版**：`html/tree/fragment.rs` の `parse_fragment` が、tokenizer を終えた後で `open_pop_all` を呼ぶ。差分の許可は、fragment の入力に `selectedcontent` があるとき（pop steps を持つのは `option` だけ）。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P17 fragment の解析の quirks mode

- **分類**：参照実装の未対応（監査の foreign F3）。移植版で直した（第 8 段）。
- **仕様**：fragment の解析で作る文書は、context の文書が quirks なら quirks、limited-quirks なら limited-quirks になる。
- **再現**：doctype のない文書（quirks）の要素に `inner_html = "<p><table></table>"`。仕様では quirks mode なので `<table>` が `p` を閉じず、`<p><table></table></p>` になる。Lexbor は fragment の文書を常に no-quirks で作るので `<p></p><table></table>` になる。
- **移植版**：`html/tree/fragment.rs` の `ContextFacts::compat_mode` を fragment の文書が受け取り、`html/fragment.rs` の `FragmentContext::element_into` が context の文書の mode を渡す。要素を context にとる解析はどれもこれを通る（`inner_html=`、`outer_html=`、`Node#parse`、`DocumentFragment.parse` と `Document#fragment` の `context:` に要素を与えたとき。別の文書の要素なら、その要素の文書の mode）。名前で与える context（`DocumentFragment.parse`、`Document#fragment` の `context:` に文字列か nil）には要素も文書もないので、no-quirks のまま（`DocumentFragment.parse` の裏の文書は doctype がなく quirks なので、写すとこの API の答えが変わる）。差分の比較は既定の `ContextFacts`（Lexbor の前提）で解析するので、許可は要らない。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P18 fragment の context の属性（`annotation-xml` の `encoding`）

- **分類**：参照実装の未対応（監査の foreign F4）。移植版で直した（第 8 段）。
- **仕様**：fragment の解析は、context 要素の local name と属性から start tag token を作り、HTML integration point の判定などに使う。`encoding` が `text/html` か `application/xhtml+xml` の MathML `annotation-xml` は HTML integration point で、その中の開始タグは HTML の規則で処理される。
- **再現**：`<annotation-xml encoding="text/html">` に `inner_html = "<foo></foo>"`。仕様では `foo` は HTML 名前空間の要素。Lexbor は context を tag と名前空間だけで作るので integration point にならず、`foo` は MathML の要素になる。文書として解析した `<annotation-xml encoding="text/html"><foo>` は前から HTML だった。
- **移植版**：`html/fragment.rs` の `FragmentContext::element_into`（P17 と同じく、要素を context にとる解析のすべて）が context の `encoding` を読み、`ContextFacts::html_integration_point` として渡す。`html/tree/actions.rs` の `html_integration_point` が、その context を integration point と答える。属性のうち解析が読むのはこれだけなので、属性そのものは写さない。差分の比較は既定の `ContextFacts` で解析するので、許可は要らない。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P20 fragment の form element pointer

- **分類**：参照実装の未対応（監査の foreign F6）。移植版で直した（第 8 段）。
- **仕様**：fragment の解析は、form element pointer を context 要素に最も近い form 要素（context 自身か、祖先をまっすぐたどったもの）に設定する。
- **再現**：`<form><div id=a></div></form>` の `#a` に `inner_html = "<form><input></form>x"`。仕様では pointer が祖先の form を指すので、`<form>` 開始タグと `</form>` は無視され、`<input>x` になる。Lexbor は context 自身が form のときだけ pointer を設定するので、`<form><input></form>x` になる。
- **移植版**：`html/fragment.rs` の `FragmentContext::element_into`（P17 と同じく、要素を context にとる解析のすべて）が context と祖先に HTML の `form` があるかを調べ、`ContextFacts::in_form` として渡す。`html/tree/fragment.rs` の `parse_fragment` が、そのとき fragment の文書に form を作って pointer に置く（context 自身が form のときと同じ）。差分の比較は既定の `ContextFacts` で解析するので、許可は要らない。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P19 独自の名前空間の context（CDATA と adjust foreign attributes）

- **分類**：参照実装の未対応（監査の foreign F5）。移植版で直した（第 8 段）。
- **仕様**：context が HTML 以外の名前空間（名前空間なしを含む）の要素なら、adjusted current node はその要素なので、`<![CDATA[` は CDATA section になる。また foreign content の "any other start tag" は、名前空間によらず adjust foreign attributes を行う。
- **再現**：`urn:x` 名前空間の要素に `inner_html = '<bar xlink:href="a"/><![CDATA[x<y]]>'`。仕様では `xlink:href` は XLink 名前空間の属性で、CDATA は text `x<y` になる。Lexbor は静的でない名前空間を「なし」として渡し、tokenizer は「なし」で CDATA を認めないので comment `<!--[CDATA[x<y]]-->` になる。adjust foreign attributes は MathML と SVG の中でしか行わない。
- **移植版**：`html/fragment.rs` の `FragmentContext::element_into`（P17 と同じく、要素を context にとる解析のすべて）が、名前空間を「なし」として渡す context について `ContextFacts::foreign_namespace` を立て、`html/tree/fragment.rs` が tokenizer の `undef_is_foreign` に写す。`html/tree/modes/foreign_content.rs` の `anything_else` は、MathML と SVG 以外の名前空間でも adjust foreign attributes を行う（`AttrAdjust::Foreign`。文書の解析ではその名前空間の要素は現れない）。差分の比較は既定の `ContextFacts` と、HTML、SVG、MathML の context で解析するので、許可は要らない。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### P21 重複した属性名

- **分類**：参照実装の未対応。移植版で直した。
- **仕様**：attribute name state を出るとき、同じ token にすでに同じ名前の属性があれば duplicate-attribute の parse error で、新しい属性は token から除く。つまり重複は tokenizer で落ち、tree builder には届かない。
- **再現**：`<svg xmlns=a xmlns=b xmlns=c>` は、仕様では `xmlns="a"` が一つ。`<table><input type=text type=hidden>` は、仕様では `type=text` だけを見るので、`input` は table の外（前）に出る。Lexbor の parser は tokenizer に keep-duplicate を設定して重複を tree builder へ渡し、tree builder が名前と名前空間で落とす。そのため名前空間が XMLNS に調整される `xmlns` は落ちずに三つ残り、"in table" の `<input>` は落ちるはずの `type=hidden` を読んで table の中に置く。Chromium 153、Firefox 155、WebKit 26.6 は仕様どおり。
- **移植版**：`html/tree/parse.rs` の `drive` が、tokenizer の `keep_duplicate` を P21 を直すときだけ外す。tokenizer の重複の判定は、属性が多い token では名前の表で引く（`html/tokenizer/mod.rs` の `AttrSeen`）。差分の比較は、tokenizer を単独で走らせて重複の parse error が出る入力を許す（`compat::explains`）。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`、`lexbor::tree_port_tests::p21_drops_repeated_attributes_in_the_tokenizer`。

### R1 void の要素の `inner_html`

- **分類**：参照実装の未対応（監査の foreign F7）。移植版で直した（第 8 段）。
- **仕様**：fragment の直列化の step 1。node が serializes as void なら空文字列を返す。
- **再現**：DOM の操作で `<br>` に text を足し、`inner_html` を読む。仕様では `""`。Lexbor は node の種類を見ずに子を書くので `"x"`。`to_html`（`<br>`）は前から一致していた。
- **移植版**：`html/dom/serialize.rs` の `write` が、子だけを書く形（`deep`）で void の要素に何も書かない。parser はこの形を作らないので、`html_diff` の直列化の比較は Lexbor の答え（`as_lexbor`）で比べる。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`。

### R2 接頭辞のある HTML、SVG、MathML の要素の名前

- **分類**：参照実装の未対応（監査の foreign F8）。移植版で直した（第 8 段）。
- **仕様**：fragment の直列化は、HTML、MathML、SVG の名前空間の要素の tag を local name で書き、ほかの名前空間の要素は qualified name で書く。
- **再現**：`create_element_ns` で作った `s:rect`（SVG）、`h:br`（HTML）、`q:foo`（`urn:x`）。仕様では `<rect></rect><br><q:foo></q:foo>`。Lexbor は常に qualified name を書くので `<s:rect></s:rect><h:br><q:foo></q:foo>`。
- **移植版**：`html/dom/serialize.rs` の `tag_name` が、名前を書いたとおりに持つ（接頭辞か大文字を持つ）要素のうち三つの名前空間のものに DOM の `localName` を返す。parser はこの形を作らない。比較の扱いは R1 と同じ。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`（R1 と同じ例の群）。

### R3 属性の名前の大文字

- **分類**：参照実装の未対応（DOM の操作）。移植版で直した。
- **仕様**：fragment の直列化は、属性の名前を、名前空間が null なら local name で、XML、XMLNS、XLink の名前空間ならそれぞれの接頭辞と local name で書く。local name は DOM の `localName`、つまり設定したときの大文字を保つ。
- **再現**：`create_element_ns(nil, "foo")` に `e["viewBox"] = "1"`。仕様では `<foo viewBox="1"></foo>`。Lexbor は属性を小文字にした名前で持ち、それを書くので `viewbox`。`set_attribute_ns(XLINK, "xlink:Href")` も `xlink:href` になる。
- **移植版**：`html/dom/serialize.rs` の `attribute` が、DOM の `localName` と属性自身の名前空間（D1）で書く。パーサの作る属性は名前を小文字にするか SVG、MathML の表で直すので、パースの結果は変わらない。比較の扱いは R1 と同じ。
- **テスト**：WPT にない。`spec/html_standard_steps_spec.rb`（R1 と同じ例の群）。

### D1 属性の名前空間

- **分類**：参照実装の未対応（DOM の操作）。移植版で直した。
- **仕様**：属性の名前空間は、属性を作ったときに決まり、要素に付けても外しても変わらない。`setAttributeNS(ns, "x")` は、名前空間 `ns` と local name `x` の属性があればその値を書き換える。
- **再現**：`<div>` に `set_attribute_ns(XHTML, "x", "1")` を二度。仕様では属性は一つ。Lexbor は属性の名前空間を持たず、接頭辞があるか要素の名前空間と違うときだけ名前空間ありと読むので、要素と同じ名前空間の属性は「名前空間なし」になり、二度目は見つからずに属性が二つになる。また `set_attribute_ns(SVG, "y")` で作った属性を外すと、要素がなくなるので名前空間が nil に変わる。
- **移植版**：`html/dom/mod.rs` の `AttrData::own_ns` が名前空間を持つ。DOM の操作で作った属性は作るときに、パーサが作った属性は最初に要素に付けたときに Lexbor の規則で決め、以後は変えない。パースの結果は変わらない。Lexbor 互換の実行（`as_lexbor`）では Lexbor の規則を保つ。
- **テスト**：WPT にない。`spec/attribute_dom_algorithms_spec.rb`。

### L1 ソース位置の規則

- **分類**：意図した変更（移植計画の「ソース位置の規則」）。
- **規則**：要素は、それ自身の開始タグの token のために作られたときだけ、その token の開始位置を持つ。`Tok::fake`、adoption agency と再構築の複製（`base` を持つ token）、別のタグの token が暗に作らせた要素（はぐれた `<td>` が作る `<tr>`）は持たない（`html/tree/stamp.rs` の `created_offset`）。`</br>` は `<br>` の開始タグとして扱われて要素を作るので、その要素は `</br>` の位置を持つ。
- **旧来の規則**：Lexbor の token hook の前後で、新しい current node か挿入先の最後の子を探して印を付ける（`source_loc::stamp_created`）。table から foster された void 要素は、どちらの場所にもないので nil だった。
- **変わる答え**：foster された void 要素が自分の行を持つ（`<table>\n<input>` の `input` は 2）。間違った行を返さないことは変わらない。
- **検証**：比較のテストと fuzz は、Lexbor 互換の実行で Lexbor の位置と完全に一致させ、仕様どおりの実行では、印を付けたすべての要素が、その位置で自分の名前の開始タグを指すことを確かめる（`lexbor::tree_diff::offsets_name_their_tags`）。

## 資源の表の項目

`html/res/tree.rs`（tree builder の分類と調整の表）は、固定した版の HTML Standard から生成する（`conformance::res_gen`）。Lexbor をリンクしている間は、`lexbor::res_tests` が Lexbor の表（`tag_res.h`、`tree_res.h`）と照合し、次の差だけを許す。

### T1 HTML の `image` の special

- **分類**：参照実装との差（観測できない）。
- **箇所**：13.2.4.3 の special の一覧は `image` を含まない。仕様のソースは、要素としてスタックに載らないので関係ないとして、コメントで外している。Lexbor の `lxb_html_tag_res_cats` は HTML の `image` を special にしている。
- **移植版**：表を仕様から生成するので、special にしない。
- **影響**：ない。`<image>` の開始タグは `img` 要素になるので HTML の `image` 要素はスタックに載らず、special を問うのはスタック上の要素だけである。

## 実装しないもの

監査で未実装または意図した差とした step のうち、ほかの項目に入らないもの。

### U1 `<template for>` による content patching

- **分類**：未実装。Lexbor にもない。processing-instruction token とともに入った機能で、WPT の pin に入力がない。
- **仕様**：in head の「A start tag whose tag name is "template"」で、shadowrootmode がなく `for` 属性があれば、scope の中から `<?marker name=...?>` か `<?start name=...?>`（と対応する `<?end?>`）を探し（find markers）、prepare content patching で template の insertion target を marker の親にし、start と end の間のノードを除く。成功すれば template 要素そのものは挿入しない。appropriate place for inserting a node はその insertion target を使い、template の終了タグが marker を外す。
- **再現**：`<div><?marker name="x"?></div><template for="x"><p>new</p></template>` は、仕様では `<div>` の子が `<p>new</p>` だけになる。移植版は marker を残し、body に `<template for="x">`（content に `<p>new</p>`）を作る。
- **扱い**：`for` は普通の属性で、template は普通の template として木に入る（`html/tree/modes/in_head.rs`）。

### 入力と資源の制限

- NUL を含む名前や不正な UTF-8 を、API の境界で例外にする（`CLAUDE.md` の text-input contract）。HTML の入力そのものは置き換えて読む。
- stylesheet の at-rule の入れ子が 64 を超えると例外にする（`selectors/stylesheet.rs` の `MAX_DEPTH`）。
- メモリ不足では stylesheet の解析全体を失敗にする（S3 と同じ方針）。

## 安全性のための差

移植計画の「変更の二つの種類」の例外として、参照実装の未定義動作や、失敗を成功として返す箇所を再現しない箇所を記録する。

### S1 selectedness の設定アルゴリズムの step 2

- **分類**：安全性のための変更（参照実装の未定義動作）。
- **箇所**：Lexbor の `lxb_html_select_selectedness_setting_algorithm` は、display size が 1 のときだけ step 1 で数を数え、step 2 ではその数を display size によらず読む。display size が 1 でない（`size` 属性が 1 以外）とき、読む値は未初期化である。
- **移植版**：display size が 1 でないときは step 2 を行わない（`html/tree/select.rs`）。
- **影響**：`size` が 1 以外で、selected な option が二つ以上あり、`<selectedcontent>` を持つ select の、複製される option が変わりうる。Lexbor の答えはスタックの内容次第で決まらない。

### S2 An+B の値が double で 2^63 になる整数

- **分類**：安全性のための変更（参照実装の未定義動作）。セレクタパーサ。
- **箇所**：Lexbor の `lexbor_conv_double_to_long` は、`(double) LONG_MAX` より大きい値を `LONG_MAX` にし、ちょうど 2^63 の値はそのまま C の long への変換に渡す。この変換は未定義動作で、x86-64 では `LONG_MIN` になる。`9223372036854775807` のように、double に読むと 2^63 になる整数がこれに当たる。
- **移植版**：2^63 も `LONG_MAX` に飽和させる（`selectors/parser.rs` の `double_to_long`）。
- **影響**：その整数を a か b に書いた `An+B` だけ。たとえば `:nth-child(-n+9223372036854775807)` は、移植版ではすべての子に当たり、Lexbor（x86-64）ではどれにも当たらない。比較テスト（`lexbor::selector_diff`）は、飽和で生じる値（±9223372036854775807 と -9223372036854775808）を照合から外す。

### S3 セレクタパーサ自身のメモリ不足

- **分類**：安全性のための変更（失敗を成功として返す）。セレクタパーサ。
- **箇所**：Lexbor の構文パーサは、括弧の種類を積むスタックを伸ばせなかったとき、token を読もうとした状態が `tkz->status` を返す。これはトークナイザの状態で OK のままなので、実行は成功として終わり、途中までのセレクタリストが返る。
- **移植版**：パーサ自身の状態を返し、メモリ不足として失敗する（`selectors/parser.rs` の `token`）。
- **影響**：メモリ不足のときだけ。Lexbor では、途中までのセレクタで照合した結果が返りうる。`rake oom` の `xml_css` シナリオがこれを捕まえた。stylesheet パーサの移植でも同じ規則に従う。Lexbor が規則の生成（at-rule、qualified rule、ブロック、宣言リスト）でメモリ不足になると、不正なデータ（`UNEXPECTED_DATA`）として失敗を報告するが、移植版はメモリ不足として報告する。また、prelude や値を読み飛ばす状態は token を読めなかったときにループを抜けて成功を返し、後のコールバックの戻り値（`parser->status = prelude_end(...)`）が失敗を上書きするので、Lexbor では途中までの stylesheet が成功として返る。移植版はメモリ不足をどこで起きても実行の終わりとする（`selectors/parser.rs` の `run`）。`rake oom` の `css_stylesheet` シナリオがこれを捕まえた。

### S4 宣言名の後にコロンがない

- **分類**：安全性のための変更（失敗を成功として返す）。stylesheet パーサ。
- **箇所**：Lexbor の `lxb_css_syntax_parser_declarations_name` は、宣言名の後（空白を飛ばした後）がコロンでないとき、状態を設定せずに NULL を返す。実行はその時点の状態で終わるので、途中までの stylesheet が成功として返る。Lexbor 自身が注記するとおり、この phase に入る前に `lookup_colon` がコロンを確かめているので、到達しない。
- **移植版**：不正なデータとして失敗する（`selectors/parser/syntax.rs` の `phase_declarations_name`）。
- **影響**：ない（到達しない）。

## stylesheet の読み取り

`Makiri::Lexbor::CSS.parse_stylesheet` の名前と値は、Lexbor の型付きのプロパティの正規化をやめ、プロパティによらない規則に置き換えた（`docs/rewrite_parser.ja.md` の「stylesheet パーサ」、2026-10-18 決定）。分類は意図した API の変更である。構造（規則、at-rule の種類と prelude、`:bad_style`、宣言の境界と `!important`）は Lexbor と同じで、`lexbor::stylesheet_diff` が比べる。

- **C1 宣言の値**：Lexbor と同じ offset の範囲を CSS のトークンとして読み直し、空白トークンの連続を空白一つにして前後を削る。ほかのトークン（文字列、url、エスケープ、コメント）は書かれたとおりに残す。Lexbor は型を持つ約 100 のプロパティだけを再直列化した（`color: RED` は `red`）。
- **C2 名前**：宣言名はカスタムプロパティ（`--` で始まる）を除いて ASCII 小文字にし、at-rule の名前も小文字にする。Lexbor は既知のプロパティと型を持つ at-rule だけを正規の名前で書き、ほかは書かれたとおりに返した。型を持つ at-rule を `_undef` に変えたもの（prelude のある `@font-face`、ブロックのある `@namespace`、ブロックのない `@media`）の名前が空になるのは Lexbor のとおりである。
- **C3 値の状態**：Lexbor は宣言の値をプロパティごとの状態で読み、移植版はすべてカスタムプロパティの状態（終わりまで読む）で読む。値の offset と `!important` の判定は構文パーサの phase が決めるので、答えは変わらない。プロパティの状態が先読みした token の数だけは `lookup_important` などの経路に影響しうるが、比較テストと fuzz で食い違いは見つかっていない。

## CSS パーサの項目

CSS Syntax Level 3 と Selectors Level 4（w3c/csswg-drafts の `7b4c69ecaf9289d1bd5455d6bea10eec0b4a4399`）の監査（第 8 段）で見つかった、Lexbor から引き継いだ逸脱を直したもの。HTML の項目と同じく `compat.rs` の `fixed` を問い、比較のテスト（`lexbor::selector_diff`、`lexbor::stylesheet_diff`）は Lexbor 互換の実行で Lexbor と一致させ、仕様どおりの実行との差を `css_explains` の述語が説明できる入力でだけ許す。

### C23〜C26 specificity

- **分類**：参照実装の未対応（監査の css_selectors F7〜F10）。移植版で直した（第 8 段）。
- **仕様**：Selectors 4 の 17 章。A は ID、B は class、属性、擬似クラス、C は type selector と擬似要素の数。`:is()`、`:not()`、`:has()` は引数のうち最も詳細な complex selector、`:nth-child()` と `:nth-last-child()` は擬似クラス一つに `of S` の最も詳細なものを足し、`:where()` は 0。成分が上限を超えるときは丸めてよい。
- **再現**：`p:first-child` は仕様で `[0,1,1]`、Lexbor は `[0,0,1]`（引数のない擬似クラスと `:nth-of-type()` を数えない、C23）。`a:is(.b, .c .d)` は `[0,2,1]`、Lexbor は `[0,1,1]`（関数の中では成分ごとに 1 に丸める、C24）。`foo|div` は `[0,0,1]`、Lexbor は `[0,0,2]`（接頭辞のある type selector を二度数える、C25）。`.a` を 512 個並べると Lexbor は `[1,0,0]`（成分が上に繰り上がる、C26）。
- **移植版**：`selectors/specificity.rs` の `respecify` が、解析の後ですべての complex selector を数え直す。関数の引数の list はそれを持つ list より後に作られるので、後ろから数えれば再帰なしで引数が先に決まる。成分は 9 bit の上限（511）で丸める。差分の許可は、入力に `:` か `|` があるか、1000 byte 以上のとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C21 An+B の後ろの token

- **分類**：参照実装の未対応（監査の css_selectors F4）。移植版で直した（第 8 段）。
- **仕様**：`:nth-child()` と `:nth-last-child()` の引数は `An+B [of S]?`、`:nth-of-type()` などは `An+B` だけ。後ろに別の token が続く引数は文法に合わず、セレクタは無効である。
- **再現**：`:nth-child(2n+1of p)`（`+1of` は dimension token）、`:nth-child(3 n)`、`:nth-of-type(2 of p)`。仕様ではどれも無効。Lexbor は余分な token を読み飛ばして規則を failed にするが、関数の終わりのコールバックがそれを見ないので、`:nth-child(even)`、`:nth-child(3)`、`:nth-of-type(2)` として受け付ける。
- **移植版**：`selectors/parser.rs` の `function_end` が、An+B を持つ関数の規則が failed で終わったとき、An+B が不正なときと同じくセレクタを無効にする。差分の許可は、入力に `nth-` があるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C18 空の `:is()` と `:where()`

- **分類**：参照実装の未対応（監査の css_selectors F1）。移植版で直した（第 8 段）。
- **仕様**：`:is()` と `:where()` の引数は forgiving selector list で、空でもよい。書いたとおり空のときも、無効な項目を除いて空になったときも、何にも一致しない。
- **再現**：`p:is()`、`p:where(:foo)`。仕様では有効で、何にも一致しない（`p:not(:is())` はすべての `p` に一致する）。Lexbor は空の関数を受け付けず、セレクタ全体が構文エラーになる。
- **移植版**：`selectors/parser.rs` の `function_empty` が、`:is()` と `:where()` を引数のないまま残す。HTML の matcher は空の list に何も一致させない。XML の XPath への変換（`css/lower.rs` の `selector_list_selftest`）は、空の list を `false()` にする。差分の許可は、入力に `is(` か `where(` があるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C19 `:has()` の引数は forgiving でない

- **分類**：参照実装の未対応（監査の css_selectors F2）。移植版で直した（第 8 段）。
- **仕様**：`:has()` は `<relative-selector-list>` を取る。固定した版の変更履歴にあるとおり、`:has()` は forgiving でなくなり、forgiving なのは `:is()` と `:where()` だけである。引数に無効な項目が一つでもあれば、セレクタ全体が無効になる。
- **再現**：`:has(p, :foo)`、`:has(p,)`。仕様では無効。Lexbor は `:has()` が forgiving だった頃の仕様に従い、無効な項目を捨てて `:has(p)` として受け付ける。
- **移植版**：`selectors/parser.rs` の `function_error` が、`:has()` の中の誤りを forgiving の経路で扱わない。差分の許可は、入力に `has(` があるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C20 `:has()` の中の `:has()`

- **分類**：参照実装の未対応（監査の css_selectors F3）。移植版で直した（第 8 段）。
- **仕様**：`:has()` は入れ子にできず、`:has()` の中の `:has()` は無効である。論理結合の擬似クラスを通しても同じ。
- **再現**：`:has(:has(span))`、`:has(p:has(span))`、`:has(:not(:has(p)))`。仕様では無効。Lexbor は受け付ける。`:has(:is(:has(p)))` は、`:is()` が forgiving なので内側の `:has()` だけが捨てられ、`:has(:is())`（何にも一致しない）になる。
- **移植版**：`selectors/parser.rs` の `pseudo_class_function` が、外側の関数を `inside_has` でたどり、`:has()` の中の `:has()` を不明な擬似クラスと同じく拒む。matcher は入れ子の `:has()` をヒープ上で照合する仕組みを持ったままで、Lexbor の engine と比べるテスト（`lexbor::tests::css_match`）は `as_lexbor` で解析して入れ子の `:has()` を使い続ける。差分の許可は C19 と同じ。
- **テスト**：`spec/css_standard_steps_spec.rb`。`spec/native_stack_guard_spec.rb` の深い `:has()` は、一つの `:has()` の中に `:is()` を深く入れ子にした形に変えた。

### C4 入力の終わりの `\` で終わる文字列

- **分類**：参照実装の未対応（監査の css_tokenizer F1）。移植版で直した（第 8 段）。
- **仕様**：consume a string token で、U+005C の次が EOF なら何もしない。
- **再現**：`[t="x\`（入力がここで終わる）。仕様では値は `x` で、`t="x"` に一致する。Lexbor は escape として U+FFFD を足し、`t="x�"` に一致する。
- **移植版**：`selectors/tokenizer.rs` の `string_state` が、入力の終わりの `\` を読み飛ばす。トークンを Lexbor と比べるテスト（`selector_port_tests` の `same_tokens`）は `as_lexbor` で比べ、セレクタと stylesheet の比較が差を許す。差分の許可は、入力が `\` で終わるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C5 `-` の後の非 ASCII 文字

- **分類**：参照実装の未対応（監査の css_tokenizer F2、F3）。移植版で直した（第 8 段）。
- **仕様**：would start an ident sequence の U+002D の分岐。二つ目の code point が ident-start code point（非 ASCII なら non-ASCII ident code point）なら真。
- **再現**：`.-א`（U+05D0）は仕様では class selector。Lexbor は UTF-8 の先頭 byte（0xD7）を Latin-1 の code point の表で引くので偽になり、構文エラーになる。逆に `@-¶ x {}`（U+00B6 は名前に使えない）は、先頭 byte 0xC2 が表で真になるので、名前 `-` の at-rule になる。仕様では at-keyword にならず、`bad_style` である。
- **移植版**：`selectors/tokenizer.rs` の `start_ident_sequence` が、`-` の後の 0x80 以上の byte を code point に復号して判定する（`start_ident_utf_8_80`、名前を始める非 ASCII 文字の判定と同じ）。差分の許可は、入力に `-` の直後の 0x80 以上の byte があるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C6 128 桁を超える整数部

- **分類**：参照実装の未対応（監査の css_tokenizer F4）。移植版で直した（第 8 段）。
- **仕様**：consume a number は数字の並び全体を数値に変える。
- **再現**：`li:nth-child(` に 0 を 128 個と `1` を続けたもの。仕様では値は 1 で、最初の子に一致する。Lexbor は数字を 128 byte の buffer に読み、129 桁目から先を捨てて位も補正しないので、値は 0 になり、何にも一致しない。
- **移植版**：`selectors/tokenizer.rs` の `consume_numeric` が、先頭の 0 を buffer に入れず、buffer に入りきらない整数部の桁の数だけ指数を上げる。小数部の 129 桁目から先は、値をほとんど変えないので捨てたままにする。差分の許可は、入力に 129 桁以上の数字の並びがあるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C8 空白だけで終わる `url(`

- **分類**：参照実装の未対応（監査の css_tokenizer F6）。移植版で直した（第 8 段）。
- **仕様**：consume an ident-like token で、`url(` の後の空白を読み、次が引用符でなければ（EOF を含む）consume a url token に進む。EOF なら空の url token になる。
- **再現**：`parse_stylesheet("a{b:url(   ")`。仕様のトークンでは、値の規則（`C1`）から `url(   ` になる。Lexbor は空白だけが続いて入力が終わると function token を作り、空白は値の末尾として削られて `url(` になる。
- **移植版**：`selectors/tokenizer.rs` の ident-like の `url(` の分岐が、空白のまま入力が終わったとき url token を作る。差分の許可は、入力に `url(` があるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C7 hash token の type flag（`#1a`）

- **分類**：参照実装の未対応（監査の css_tokenizer F5、css_selectors F5）。移植版で直した（第 8 段）。
- **仕様**：hash token は、`#` の後が ident sequence を始めるとき type flag が "id" になる。ID selector はその flag を持つ hash token だけで、`#1a` や `#-1` は無効なセレクタである（ブラウザの `querySelector` も SyntaxError にする）。
- **再現**：`#1a`、`#-1`。仕様では無効。Lexbor は type flag を記録せず、どの hash token も ID selector にするので、`id="1a"` の要素に一致する。エスケープで書いた `#\31 a` は仕様でも有効で、同じ要素に一致する。
- **移植版**：`selectors/tokenizer.rs` の `hash_is_id` が token の入力上の位置から flag を求め、`selectors/parser.rs` の `selector_hash` が flag のない hash を不明な構文と同じく拒む。stylesheet の規則は `bad_style` になる。差分の許可は、入力に `#` の後に名前の始まりでない byte が続く箇所があるとき。
- **互換性**：`#1a` を書いていた呼び出し側は `#\31 a` か `[id="1a"]` に書き換える必要がある。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C22 long の範囲を超える `<ndashdigit-*>`

- **分類**：参照実装の未対応（監査の css_selectors F6）。移植版で直した（第 8 段）。
- **仕様**：`<ndashdigit-ident>` などの `n-` の後の数字の数に上限はない。値の範囲の扱いは書かれていない。
- **再現**：`:nth-child(n-99999999999999999999)`。Lexbor は ident の中の数字を long に読み、範囲を超えると読み残して誤りにするので構文エラーになる。同じ値を別の token として書いた `:nth-child(n- 99999999999999999999)` は、数値の token として飽和させて受け付けていた。
- **移植版**：`selectors/parser.rs` の `data_to_long` が、範囲を超える値を long の最大値に飽和させ、残りの数字も読む。S2 で 2^63 を飽和させるのと同じ規則で、書き方によらず同じ答えになる。Lexbor の掛け算は桁あふれで値が回るので、範囲を超える前に確かめる。差分の許可は、入力に `n-` と 19 桁以上の数字の並びがあるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C15 値の正規化と `\` の delim

- **分類**：移植版の値の正規化（C1）から生じた差（監査の css_syntax F7）。直した（第 8 段）。
- **仕様**：直列化は解析と round-trip する（空白の連続を一つにすることだけは許す）。`\` の delim token は、`\` と改行として書く。トークナイザが `\` を delim にするのは、その後が改行のときだけである。
- **再現**：`a{b: c\` 改行 `d}`。C1 の規則は空白 token を空白一つにするので、値は `c\ d` になり、読み直すと `\ ` がエスケープになって ident 一つ（`c d`）になる。値の末尾の `\`（`a{e: x \` 改行 `}`）は、範囲が改行の前で終わるので、読み直すと U+FFFD のエスケープになる。
- **移植版**：`selectors/stylesheet.rs` の `normalize_value` が、`\` の delim の後に改行を書き、続く空白はその改行で表す。値の範囲の末尾にエスケープされていない `\` があり、元の入力でその後が改行なら、改行を足す。比較のテストは規則の構造だけを比べるので、許可は要らない。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C12 `!important` の判定

- **分類**：参照実装の未対応（監査の css_syntax F4）。移植版で直した（第 8 段）。
- **仕様**：consume a declaration の step 6。値の末尾の二つの token（空白を除く。コメントは token にならない）が `!` の delim と、値が "important" に ASCII case-insensitive に一致する ident なら、important を立てて二つを値から除く。
- **再現**：`a { x: y ! important }`、`a{b: c !/**/important}`、`a{b: c !important /**/ }`、`a{b: c !imp\ortant}`。仕様ではどれも値 `c`（`y`）で important。Lexbor は `!` の直後の byte 列を `important` と比べ、その後に空白、`;`、`}`、入力の終わりしか許さないので、どれも important でなく、`!important` が値に残る。
- **移植版**：`selectors/parser/syntax.rs` の `important_by_tokens` が、Lexbor の判定が important を見つけなかった宣言の値を token として読み直し、末尾の二つを確かめる。差分の許可は、入力に `!` があるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C13 括弧の中の `!important`

- **分類**：参照実装の未対応（監査の css_syntax F5）。移植版で直した（第 8 段）。
- **仕様**：step 6 が見るのは値の top-level の token で、simple block や function の中の `!` と `important` はその中身である。括弧の中の `;` も値を終えない。
- **再現**：`a{b: (x !important; y); c:d}`。仕様では `b` の値は `(x !important; y)` で important でない。Lexbor は深さを見ずに `!` の後を調べ、`;` が続くので値を `(x` で切って important にする。
- **移植版**：`selectors/parser/syntax.rs` の値の phase が、括弧の中の `!` で `!important` を探さない。差分の許可は C12 と同じ。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C9 値に `{}` の block とほかの値が並ぶ宣言（`a:hover{...}` が宣言に吸い込まれる）

- **分類**：参照実装の未対応（監査の css_syntax F1）。移植版で直した（第 8 段）。
- **仕様**：consume a declaration の step 8。カスタムプロパティでない宣言の値に top-level の `{}` の simple block と、ほかの空白でない値が並ぶなら、宣言は何も返さない。consume a block's contents は、宣言が返らなければ mark に戻って qualified rule として読み直す。
- **再現**：`@media (max-width:600px){ .x{a:b} a:hover{color:red} .y{c:d} }`。仕様では `@media` の中に style rule `.x`、`a:hover`、`.y` の三つ。Lexbor は block の中で ident の後に `:` があれば宣言と決め、値を `hover{color:red} .y{c:d}` として読むので、規則は `.x` だけになる。型セレクタの直後に擬似クラスを書いた規則（`a:hover`、`li:first-child`、`input:focus`）を `@media` や `@supports` に書くと、その規則と後ろの規則が失われていた。
- **移植版**：block の中で宣言か規則かを決めるとき（`selectors/parser/syntax.rs` の `is_declaration`）、`selectors/tokenizer.rs` の `lookup_declaration` が宣言になるはずの値を先に token として読み、step 8 に当たれば規則として読む。読み直しの mark を持たない Lexbor の制御フローのまま、決める時点で先読みする。差分の許可は、入力に `{` があるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C14 宣言名と `:` の間のコメント

- **分類**：参照実装の未対応（監査の css_syntax F6）。移植版で直した（第 8 段）。
- **仕様**：コメントは token にならない。consume a declaration は名前の後の空白を捨てて `:` を見るので、`c/**/: d` も `c /**/ : d` も宣言である。
- **再現**：`a{x:y; c /**/ : d; e:f}`。仕様の宣言は三つ。Lexbor は名前の後の byte 列を見て、空白以外の最初の byte が `:` かどうかだけを調べるので、`c /**/ : d` を規則として読み、それより後の宣言も別の列に回って出力から消える。
- **移植版**：`lookup_declaration` が `:` も token として探す。コメントで分かれた空白は二つの空白 token になるので、宣言名の phase は空白を続けて読み飛ばす。差分の許可は、入力に `/*` があるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C10 block のない qualified rule

- **分類**：参照実装の未対応（監査の css_syntax F2）。移植版で直した（第 8 段）。
- **仕様**：consume a qualified rule は、block の前に EOF、stop token（block の中では `;`）、入れ子の `}` に達すると何も返さない。consume a block's contents は、何も返らなければ何もしない（宣言の列も分けない）。
- **再現**：`a{b:c} d` は仕様では規則 `a` だけ。Lexbor は style rule `d`（宣言なし）も返す。`@media x { b c; a{b:c} }` も `b c` を返す。`a { c: d; b; e: f }` は仕様の宣言が `c: d` と `e: f`。Lexbor は規則 `b` が宣言の列を分けるので、`e: f` が出力から消える。
- **移植版**：`selectors/parser/syntax.rs` の `phase_qualified_prelude_end`（block のない終わり）が規則に `discard` を付け、`qualified_rule_end` が何も返さない。block の中では、その後の宣言を直前の宣言の列に続ける（`phase_block_declarations`）。差分の許可は、stylesheet のすべての入力（`stylesheet_explains`。どの入力も block のない規則で終わりうる）。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C11 最上位の `--name:` で始まる prelude

- **分類**：参照実装の未対応（監査の css_syntax F3）。移植版で直した（第 8 段）。
- **仕様**：consume a qualified rule の `<{-token>`。prelude の最初の二つの空白でない値が `--` で始まる ident と `:` なら、最上位では block を読んで何も返さない（`--foo:hover { color: blue; }` はどこでも無効）。block の中では宣言として読まれる。
- **再現**：`--foo:hover { color: blue } a{b:c}`。仕様では規則 `a` だけ。Lexbor は `--foo:hover` を型セレクタと擬似クラスとして読み、style rule にする。
- **移植版**：`selectors/parser/syntax.rs` の `phase_qualified_block` が、最上位の prelude を `custom_property_prelude`（`selectors/tokenizer.rs`）で確かめ、当たれば `discard` を付ける。差分の許可は、入力に `--` があるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C16 at-rule の block の中の宣言

- **分類**：出力の形の不足（監査の css_syntax U1）。直した（第 8 段）。
- **仕様**：block を持つ at-rule は、宣言の列と child rules を持つ。`@font-face`（`<declaration-list>`）や `@page`（`<declaration-rule-list>`）の中身は宣言である。
- **再現**：`@font-face { font-family: x }`。パーサは宣言を解析していたが、読み取り層が捨てていたので、出力は `rules: []` だけだった。Lexbor の DOM を使っていた頃の `parse_stylesheet` も同じだった。
- **移植版**：`selectors/stylesheet.rs` の `block_declarations` が block の宣言の列をすべて順に読み、at-rule の Hash に `declarations:` として返す（宣言がなければ `[]`）。既存のキーの中身は変わらない。
- **テスト**：`spec/css_standard_steps_spec.rb`。

### C17 style rule の入れ子の規則（CSS Nesting）

- **分類**：意図した差（監査の css_syntax U2）。対応しない（2026-10-18 決定）。
- **仕様**：qualified rule は child rules を持つ。block の中の最初の宣言の列より後の規則と宣言は、入れ子の規則と nested declarations rule になる。
- **再現**：`a { color: red; & b { c: d } e: f }`。出力は `color: red` だけで、`& b { c: d }` と `e: f` は返らない。
- **理由**：Lexbor も CSS Nesting を扱わず、Lexbor を使っていた頃の `parse_stylesheet` も同じ答えだった。入れ子の規則を返すには、出力の形を足すことに加えて、セレクタパーサに `&` と相対セレクタの扱いを足す必要がある。利用側（dommy）が入れ子の CSS を必要とするまで入れない。

### C28 compound の途中の名前空間つき型セレクタ

- **分類**：参照実装の誤り。移植版で直した。
- **仕様**：compound は `<type-selector>? <subclass-selector>* ...` で、型セレクタ（全称セレクタを含む）は先頭に高々一つ。
- **再現**：`|E|*`、`.x|a`、`[x]|a`、`a|*|b`。仕様ではどれも無効で、Chromium、Firefox、WebKit は SyntaxError。Lexbor は compound の終わりで `|` を見ると、`||` でなければ結合子のないまま compound の続きに戻るので、二つ目の型セレクタを同じ compound に足して受け付ける（`|E|*` は「名前空間のない E」として照合され、旧来の XML の XPath への変換は二つ目で名前を上書きして名前空間のない要素すべてを返していた）。接頭辞のない `a*` や `[x]a` は Lexbor も無効にする。
- **移植版**：`selectors/parser.rs` の `complex_end` が、結合子のない `|` を無効にする。`a |b` や `a>|b` のように結合子のあとの `|` は新しい compound の始まりで、有効のまま。差分の許可は、入力に `|` があるとき。
- **テスト**：`spec/css_standard_steps_spec.rb`、`spec/conformance/css_browser_diff.rb`。

### C27 `[*|a]`（任意の名前空間の属性）

- **分類**：意図した差（監査の css_selectors U1）。対応しない（2026-10-18 決定）。
- **仕様**：属性セレクタの名前空間の接頭辞 `*` は、どの名前空間の属性（名前空間のないものを含む）にも当たる。
- **再現**：`[*|a]` は `Makiri::CSS::SyntaxError` になる。
- **理由**：Lexbor も受け付けず、移植版は同じ答えを保つ。HTML では名前空間を持つ属性は SVG の `xlink:href` などに限られ、ほとんどの用途は `[a]` で足りる。対応には AST の表現、HTML の matcher、XML の XPath への変換の三か所の変更が要る。

## パーサの外の項目

次の差は HTML パーサではなく、DOM の上の層（CSS マッチャ、XPath の名前照合）にある。
パーサの移植では変わらないが、同じ表で管理する。
詳細と根拠は `CLAUDE.md` の CSS の節にある。

- **M1 form state**：`:disabled`、`:enabled`、`:checked` は HTML Standard の定義に従う（fieldset の継承、最初の legend 子要素、option と optgroup）。Lexbor は近似している。分類は意図した互換差ではなく、仕様への準拠である。
- **M2 link**：`:any-link` と `:link` は、`href` 属性を持つ HTML の `a` と `area`、および `href` か XLink の `href` を持つ SVG の `a` に当たる（HTML Standard と SVG 2。履歴がないので二つは同じ）。Lexbor は、`a`、`area` と `map`（`:any-link`）または `link`（`:link`）を名前空間を問わず数え、属性も名前空間を問わず local name で見るので、`<link href>` が `:link` に当たり、`:any-link` と答えが食い違っていた。Chromium、Firefox、WebKit は前者。`Fix::M2`。
- **M3 属性名の大文字小文字**：SVG と MathML の要素では、CSS の属性セレクタの属性名を大文字小文字を区別して照合する。Lexbor はすべての要素で畳む。HTML Standard の規則に従う。
- **M4 type セレクタの大文字小文字**：HTML 以外の要素では、type セレクタを書かれたとおりに localName と照合する（`fegaussianblur` は SVG の `feGaussianBlur` に当たらない）。Lexbor はすべての要素で畳む。HTML Standard の規則に従う。
- **M5 `:nth-child(... of S)`**：S にコンマ区切りのリスト、結合子、擬似クラスがある場合を CSS の定義で数える。Lexbor はこれらを正しく数えない。
- **M6 type セレクタの名前空間の接頭辞**：CSS は接頭辞を `css(selector, ns)` の Hash で解決する。`p|E` は Hash が `p` に束縛した名前空間の E、`|E` は名前空間のない E、`*|E` はどの名前空間の E にも当たり、Hash が束縛しない接頭辞はセレクタを不正にする（`Makiri::CSS::SyntaxError`、照合の前に compile で決まる）。素の E はどの名前空間の E にも当たり、Hash の `"xmlns"` が既定の名前空間を与えると、素の型セレクタと `*` はその名前空間のものになる（CSS の `@namespace` と同じ）。型セレクタのない compound も、`*` が書かれたものとしてその名前空間の要素にだけ当たる（Selectors 4 §5.3）。ただし `:is()`・`:where()`・`:not()` の引数の subject の compound はその例外になる（§4.2-4.4）。`:is()` と `:where()` は forgiving なので、未宣言の接頭辞を含む選択肢はその選択肢だけが捨てられる（三つのブラウザと同じ）。`:not()`・`:has()`・`of S`・最上位では、セレクタ全体が不正になる。XML も同じ規則で、文書の宣言は集めない。Lexbor は接頭辞を無視し、`svg|path`、`|path`、`path` はどれも SVG の `path` に当たっていた。Selectors 4 §6.1 の規則で、宣言の手段がない点を除いて三つのブラウザの querySelector と同じ（`spec/conformance/css_browser_diff.rb`）。`Fix::M6`。Lexbor の旧エンジンとの比較は Lexbor 互換の実行で接頭辞を無視する。
- **M7 属性セレクタの名前空間の接頭辞**：`[p|a]` は Hash が `p` に束縛した名前空間の、local name が `a` の属性に当たる（HTML の要素では ASCII の大文字小文字を区別しない）。接頭辞のない `[a]` と `[|a]` は、DOM の `getAttribute` と同じく修飾名で引く。`[xlink|href]` は、`xlink` を XLink の名前空間に束縛すれば SVG の `xlink:href` に当たる。Lexbor は接頭辞を無視していた。`[*|a]` は C27。M6 と同じ `Fix::M6`。
- **M8 `:nth-child` 系が数える兄弟**：`:nth-child`、`:nth-last-child`、`:first-child`、`:last-child`、`:only-child` は、要素の兄弟だけを数える（Selectors 4）。Lexbor は Text と Comment 以外のすべてを数えるので、`<!doctype html>` があると `html:first-child` が当たらず、HTML の中の処理命令（`<?pi x?>`）も位置を一つ取った。Chromium、Firefox、WebKit は仕様どおり。`css_match/tree.rs` の `counts_toward_child_position`。Lexbor の旧エンジンとの比較は Lexbor 互換の実行（`compat::as_lexbor`）で行い、仕様の答えは `lexbor::tests::css_match::matcher_departures` と `spec/css_standard_steps_spec.rb` が確かめる。
- **M9 `:read-write` と `:read-only`**：HTML Standard の定義に従う。`:read-write` は、readonly 属性が適用される種類（text、search、url、tel、email、password、日付と時刻、number。type がないか不正なら text）の `input` と `textarea` で、readonly 属性がなく無効でないもの、および編集可能な要素（`contenteditable` の `""`、`true`、`plaintext-only` の中。`false` で打ち消し、不正な値は親を継ぐ）に当たり、`:read-only` はそれ以外の要素。Lexbor は readonly 属性のない `input` と `textarea` をすべて書き込み可とするので、チェックボックスやラジオも当たり、編集可能な要素は当たらなかった。Chromium、Firefox、WebKit は前者（`:read-only` は Chromium が文書のほかの要素で異なる）。`Fix::M9`。
- **M10 `:empty`**：子に要素も、長さのあるテキスト（CDATA 区間を含む）もない要素に当たる。コメントと処理命令は数えない。Chromium、Firefox、WebKit と同じ（Selectors 3 の定義。Selectors 4 の空白の扱いは実装していない）。Lexbor は Comment 以外の子をすべて数えるので、処理命令や空の CDATA 区間だけを持つ要素は空ではなかった。`Fix::M10`。

実装していない疑似クラス（`:lang()`、`:dir()`、`:scope` など）、擬似要素、`[*|a]`（C27）は、`spec/conformance/css_browser_diff.rb`（`rake conformance:css_browsers`）が三つのブラウザと比べて一覧にする。XML の CSS は HTML と同じ照合器で、照合の範囲、接頭辞のない型セレクタ、フォーム状態、エラーの種類はブラウザと一致する。各差は理由のタグ付きで `RULES` に載り、タグのない差と、差でなくなった項目が出ると失敗する。M2、M9、M10 は、このブラウザとの比較で見つけて直した。

## オラクル同士の食い違い

ここに載せるのは Makiri の差ではなく、オラクルが固定した仕様と食い違っている箇所である。
オラクルを読むときに同じ調査を繰り返さないために記録する。
ブラウザの版は Chromium 153.0.8010.12、Firefox 155.0、WebKit 26.6（Playwright 1.63.0 の取得物）である。
比較は `spec/conformance/browser_diff.rb` が行う（`rake conformance:browsers`）。

- **B1 RCDATA の context での U+0000**：context が textarea や title の fragment で、三つのブラウザはいずれも U+0000 を捨てる。仕様の RCDATA state は U+0000 を U+FFFD にし、tree builder の "in body" が捨てるのは U+0000 だけなので、U+FFFD が残る。WPT `plain-text-unsafe.dat` #47、#48 の期待値と Makiri は仕様どおりである。
- **B2 processing-instruction token**：2026-10 時点で実装しているのは Chromium だけで、Firefox と WebKit は旧来の bogus comment を作る。WPT `processing-instructions.dat` をはじめ 90 件余りの食い違いの大半がこれである。
- **B3 WebKit の DOMParser と quirks mode**：WebKit は DOMParser で作った文書を doctype によらず `CSS1Compat` と報告する。このため比較では、文書を同一オリジンの URL から scripting を無効にした iframe に読み込む。
