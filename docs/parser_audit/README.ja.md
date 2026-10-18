# HTML パーサの仕様監査（第 8 段）

移植計画（`docs/rewrite_parser.ja.md`）の「仕様全体の監査」の成果物である。
固定した版の HTML Standard（`de953a7cf0523f5873a1904ba2ce010c689dfb66`）の構文解析の章を六つに分け、step ごとに、対応するコードの位置、対応の状態、根拠となるテストを表にした。
差分表（`docs/parser_differences.ja.md`）がテストとオラクルの食い違いしか拾わないのに対し、この表はどのオラクルも覆っていない step を拾うためのものである。

## 表

| ファイル | 範囲（HTML Standard の節） | 行数 | 一致 | 意図した差 | 未実装 | 逸脱 |
|---|---|---|---|---|---|---|
| [`tokenizer1.md`](tokenizer1.md) | Preprocessing the input stream、Tokenization の総則から Bogus comment state まで | 199 | 197 | 1 | 0 | 1 |
| [`tokenizer2.md`](tokenizer2.md) | Markup declaration open state から Numeric character reference end state まで（processing instruction の状態を含む）、Named character references | 184 | 182 | 0 | 0 | 2 |
| [`tree_core.md`](tree_core.md) | Parse state、tree construction の dispatcher、Creating and inserting nodes、"initial" から "after head" まで | 152 | 131 | 17 | 3 | 1 |
| [`in_body.md`](in_body.md) | "in body"（adoption agency algorithm を含む） | 111 | 100 | 5 | 0 | 6 |
| [`table_modes.md`](table_modes.md) | "text" から "after after frameset" まで | 149 | 142 | 3 | 0 | 4 |
| [`foreign_fragment_serialize.md`](foreign_fragment_serialize.md) | foreign content、The end、Parsing HTML fragments、Serializing HTML fragments | 84 | 59 | 16 | 0 | 9 |

状態の意味は次のとおり。

- **一致**：step のとおりに動く。
- **意図した差**：差分表に載っているか、Makiri が持たない機能（scripting、browsing context、custom element、sanitizer、shadow root など）にだけ関わる step。
- **未実装**：step に当たるコードがない。
- **逸脱**：step と違う結果になる。

各表の末尾に「逸脱と未実装」と「未カバーの step」がある。
監査は、step ごとにコードを追い、観測できるものは組み込み済みの拡張に入力を与えて確かめた。
in body の監査は、parse5（scripting off）との無作為な比較も使い、食い違いは仕様を追って判定した。

表の各行の「状態」は監査の時点のものである。
見つかった逸脱をその後どう扱ったかは、下の一覧に書く。

## 見つかった逸脱と未実装

すべて Lexbor の振る舞いを移植版が写したもので、WPT の tree construction のデータと html5lib の tokenizer テストのどちらも通らない step にある。
仕様に寄せるものは、第 7 段と同じ手順（`html/tree/compat.rs` の `fixed`、`explains` の述語、最小再現の spec、差分表の項目）で一件ずつ入れる。

| id | 表 | 内容 | 状態 |
|---|---|---|---|
| P4 | table_modes F1 | "in table" の文字 token で、current node が template のときも "in table text" に進む | 直した |
| P5 | table_modes F2 | "in column group" の DOCTYPE を無視する | 直した |
| P6 | table_modes F3 | "in column group" で、他の文字の後ろの空白を捨てる | 直した |
| P7 | table_modes F4 | "after after frameset" で、他の文字を含む token の空白を捨てる | 直した |
| P8 | tokenizer2 D1 | `[CDATA[` の途中で入力が終わると、bogus comment の中身が失われる（`<![CDA`） | 直した |
| P9 | tokenizer2 D2 | 入力末尾の `<!` の直前の文字列が、空のはずのコメントに入る（`x<!`） | 直した |
| P10 | tokenizer1 F1、tree_core 逸脱 1、in_body F4 | 値のない属性（`<b x>`）と空の値（`<b x="">`）を、formatting element の「三つまで」の比較で別の値として扱う | 直した |
| P11 | in_body F1 | adoption agency algorithm の bookmark と formatting element の位置を添字で持ち、要素を除いた後に補正しない | 直した |
| P12 | in_body F2 | `<textarea>` の後に "text" insertion mode に入らず、formatting element を再構築する | 直した |
| P13 | in_body F3 | `<input>` の `type=hidden` の判定が ASCII の大文字小文字を区別する | 直した |
| P14 | in_body F5 | table の insertion mode で `<image>` を `<img>` として処理し直さずに失う | 直した |
| P15 | foreign F1 | foreign content を抜ける開始タグの一覧に `sup` がない | 直した |
| P16 | foreign F2 | fragment の解析の終わりに open element を pop しない（`selectedcontent` が更新されない） | 直した |
| P17 | foreign F3 | fragment の解析の文書が、context の文書の quirks mode を写さない | 未着手 |
| P18 | foreign F4 | fragment の context 要素を属性なしで作る（`annotation-xml` の `encoding` が効かない） | 未着手 |
| P19 | foreign F5 | 独自の名前空間の context 要素を名前空間なしとして渡す | 未着手 |
| P20 | foreign F6 | form element pointer を、祖先の form ではなく context 自身が form のときだけ設定する | 未着手 |
| R1 | foreign F7 | 子を持つ void 要素の `inner_html` が子を返す（仕様は空） | 未着手 |
| R2 | foreign F8 | 接頭辞を持つ HTML、SVG、MathML の要素を修飾名で直列化する（仕様は local name） | 未着手 |
| U1 | tree_core 未実装 2 | `<template for>` による content patching | 未実装（Lexbor にもない） |

parse error の記録に関する差（記録されない error、位置の違い）は、木を変えないので一覧に入れていない。
各表の備考に残してある。
