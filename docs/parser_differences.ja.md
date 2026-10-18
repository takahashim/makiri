# HTML パーサの差分表

本書は、Makiri の HTML パーサと、対象とする仕様およびオラクルとの食い違いを記録する。
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
いずれも `html5lib_runner.rb` の `KNOWN_DIVERGENCES` に同じ id で載っており、runner は該当テストを失敗とせずに報告し、通るようになったら失敗する。

### P1 暗黙に body を挿入したときの frameset-ok の再設定

- **分類**：参照実装の未対応（Makiri は Lexbor の振る舞いを出荷している）。
- **仕様**：tree construction の「after head」insertion mode の anything else。body 要素を暗黙に挿入するとき、frameset-ok flag を "ok" に戻す。whatwg/html#12908（2026-09-07 merge、issue #3178）で入った。
- **再現**：`<head><template></template></head><p><frameset>`。仕様では frameset 文書になり、Lexbor は `<body><p>` を作る。`<template>` が "in head" で frameset-ok を "not ok" にしたまま、戻す処理がない。
- **テスト**：WPT `template.dat` #44、#45、#46。
- **入れる段**：移植計画の 7（仕様適合の変更）。

### P2 form element pointer と「parsing template contents」

- **分類**：参照実装の未対応。
- **仕様**：「parsing template contents」は、open elements のスタックに template 要素があるか、fragment の context 要素が template 要素であるときに真になる。"in body" と "in table" の form 開始タグは、この条件で form element pointer を無視し、設定もしない。whatwg/html issue #12257（2026-09-07 close）で入った。
- **再現**：context が template の fragment で `<form><form>`。仕様では入れ子の form が二つでき、Lexbor は二つ目を捨てる。Lexbor は旧来の「スタックに template 要素があるか」だけを見ており、context の template を数えない。"in table" の form 開始タグも旧来の規則のままで、`<template><table><form></table></template>` の form を捨てる。
- **テスト**：WPT `template.dat` #117、#120、#124。
- **入れる段**：7。

### P3 foreign content の U+FFFD と frameset-ok

- **分類**：参照実装の未対応。
- **仕様**：foreign content の「Any other character token」は frameset-ok flag を "not ok" にする。U+0000 を置き換えた U+FFFD は別の規則（「A character token that is U+0000 NULL」）で扱われ、flag を変えない。入力に現れた U+FFFD（文字参照で書かれたものを含む）は「Any other character token」に当たる。
- **再現**：`<!doctype html><svg>&#xFFFD;</svg><frameset><frame>`。仕様では frameset が無視され、Lexbor は frameset 文書を作る。Lexbor の foreign content の文字処理は、NUL の置換で生じた U+FFFD を除外するつもりで、文字列中のすべての U+FFFD を空白と同じく読み飛ばす。"in body" の文字処理にはこの除外がなく、仕様どおりである。
- **テスト**：WPT `tests19.dat` #51、`plain-text-unsafe.dat` #24。
- **入れる段**：7。移植の初期実装は、この読み飛ばしを Lexbor のとおりに再現する。

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
