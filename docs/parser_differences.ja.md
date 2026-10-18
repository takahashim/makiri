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

## パーサの項目

P で始まる項目は HTML パーサの振る舞いである。
いずれも `spec/conformance/parser_known_divergences.txt` に同じ id で載っており、Ruby の runner と crate の runner は該当テストを失敗とせずに報告し、通るようになったら失敗する。三つ目の欄は、まだ食い違うパーサ（`lexbor`、`port`）を示す。移植版で直した項目は `lexbor` だけに残る。

移植版の直し方は、`html/tree/compat.rs` の `fixed` を問う分岐として入れる。出荷するビルドは常に仕様どおりに進み、比較のテストと fuzz は Lexbor 互換の実行（`as_lexbor`）で Lexbor との完全一致を保ったうえで、仕様どおりの実行との差を `explains` の述語が説明できる入力でだけ許す。

### P1 暗黙に body を挿入したときの frameset-ok の再設定

- **分類**：参照実装の未対応。移植版で直した（第 7 段）。Lexbor の DOM のビルド（`MAKIRI_LEXBOR_DOM=1`）は旧来の答えのまま。
- **仕様**：tree construction の「after head」insertion mode の anything else。body 要素を暗黙に挿入するとき、frameset-ok flag を "ok" に戻す。whatwg/html#12908（2026-09-07 merge、issue #3178）で入った。
- **再現**：`<head><template></template></head><p><frameset>`。仕様では frameset 文書になり、Lexbor は `<body><p>` を作る。`<template>` が "in head" で frameset-ok を "not ok" にしたまま、戻す処理がない。
- **テスト**：WPT `template.dat` #44、#45、#46。`spec/html_standard_steps_spec.rb`。
- **移植版**：`html/tree/modes/after_head.rs` の `anything_else` が frameset-ok を "ok" に戻す。差分の許可は `html/tree/compat.rs` の `explains`（入力に `frameset` と `template` があるとき）。

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

### L1 ソース位置の規則

- **分類**：意図した変更（移植計画の「ソース位置の規則」）。Lexbor の DOM のビルドは旧来の答えのまま。
- **規則**：要素は、それ自身の開始タグの token のために作られたときだけ、その token の開始位置を持つ。`Tok::fake`、adoption agency と再構築の複製（`base` を持つ token）、別のタグの token が暗に作らせた要素（はぐれた `<td>` が作る `<tr>`）は持たない（`html/tree/guard.rs` の `created_offset`）。`</br>` は `<br>` の開始タグとして扱われて要素を作るので、その要素は `</br>` の位置を持つ。
- **旧来の規則**：Lexbor の token hook の前後で、新しい current node か挿入先の最後の子を探して印を付ける（`source_loc::stamp_created`）。table から foster された void 要素は、どちらの場所にもないので nil だった。
- **変わる答え**：foster された void 要素が自分の行を持つ（`<table>\n<input>` の `input` は 2）。間違った行を返さないことは変わらない。
- **検証**：比較のテストと fuzz は、Lexbor 互換の実行で Lexbor の位置と完全に一致させ、仕様どおりの実行では、印を付けたすべての要素が、その位置で自分の名前の開始タグを指すことを確かめる（`lexbor::tree_diff::offsets_name_their_tags`）。

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

## パーサの外の項目

次の差は HTML パーサではなく、DOM の上の層（CSS マッチャ、XPath の名前照合）にある。
パーサの移植では変わらないが、同じ表で管理する。
詳細と根拠は `CLAUDE.md` の CSS の節にある。

- **M1 form state**：`:disabled`、`:enabled`、`:checked` は HTML Standard の定義に従う（fieldset の継承、最初の legend 子要素、option と optgroup）。Lexbor は近似している。分類は意図した互換差ではなく、仕様への準拠である。
- **M2 link**：`:any-link` と `:link` は Lexbor の定義に従う（`map` を数える、名前空間を問わない、`xlink:href`）。意図した互換差。
- **M3 属性名の大文字小文字**：SVG と MathML の要素では、CSS の属性セレクタの属性名を大文字小文字を区別して照合する。Lexbor はすべての要素で畳む。HTML Standard の規則に従う。
- **M4 type セレクタの大文字小文字**：HTML 以外の要素では、type セレクタを書かれたとおりに localName と照合する（`fegaussianblur` は SVG の `feGaussianBlur` に当たらない）。Lexbor はすべての要素で畳む。HTML Standard の規則に従う。
- **M5 `:nth-child(... of S)`**：S にコンマ区切りのリスト、結合子、擬似クラスがある場合を CSS の定義で数える。Lexbor はこれらを正しく数えない。

## オラクル同士の食い違い

ここに載せるのは Makiri の差ではなく、オラクルが固定した仕様と食い違っている箇所である。
オラクルを読むときに同じ調査を繰り返さないために記録する。
ブラウザの版は Chromium 153.0.8010.12、Firefox 155.0、WebKit 26.6（Playwright 1.63.0 の取得物）である。
比較は `spec/conformance/browser_diff.rb` が行う（`rake conformance:browsers`）。

- **B1 RCDATA の context での U+0000**：context が textarea や title の fragment で、三つのブラウザはいずれも U+0000 を捨てる。仕様の RCDATA state は U+0000 を U+FFFD にし、tree builder の "in body" が捨てるのは U+0000 だけなので、U+FFFD が残る。WPT `plain-text-unsafe.dat` #47、#48 の期待値と Makiri は仕様どおりである。
- **B2 processing-instruction token**：2026-10 時点で実装しているのは Chromium だけで、Firefox と WebKit は旧来の bogus comment を作る。WPT `processing-instructions.dat` をはじめ 90 件余りの食い違いの大半がこれである。
- **B3 WebKit の DOMParser と quirks mode**：WebKit は DOMParser で作った文書を doctype によらず `CSS1Compat` と報告する。このため比較では、文書を同一オリジンの URL から scripting を無効にした iframe に読み込む。
