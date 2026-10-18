# HTML パーサの仕様監査（第 8 段）

移植計画（`docs/rewrite_parser.ja.md`）の「仕様全体の監査」の成果物である。
固定した版の HTML Standard（`de953a7cf0523f5873a1904ba2ce010c689dfb66`）の構文解析の章を六つに分け、CSS Syntax Level 3 と Selectors Level 4（w3c/csswg-drafts の `7b4c69ecaf9289d1bd5455d6bea10eec0b4a4399`）のうち CSS パーサが扱う範囲を三つに分け、step ごとに、対応するコードの位置、対応の状態、根拠となるテストを表にした。
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
| [`css_tokenizer.md`](css_tokenizer.md) | CSS Syntax：入力の前処理、§4 Tokenization | 124 | 118 | 0 | 2 | 4 |
| [`css_syntax.md`](css_syntax.md) | CSS Syntax：§5 Parsing、`parse_stylesheet` の出力 | 97 | 69 | 15 | 2 | 11 |
| [`css_selectors.md`](css_selectors.md) | Selectors Level 4 の文法、An+B、specificity | 98 | 72 | 13 | 1 | 12 |

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
| P17 | foreign F3 | fragment の解析の文書が、context の文書の quirks mode を写さない | 直した |
| P18 | foreign F4 | fragment の context 要素を属性なしで作る（`annotation-xml` の `encoding` が効かない） | 直した |
| P19 | foreign F5 | 独自の名前空間の context 要素を名前空間なしとして渡す | 直した |
| P20 | foreign F6 | form element pointer を、祖先の form ではなく context 自身が form のときだけ設定する | 直した |
| R1 | foreign F7 | 子を持つ void 要素の `inner_html` が子を返す（仕様は空） | 直した |
| R2 | foreign F8 | 接頭辞を持つ HTML、SVG、MathML の要素を修飾名で直列化する（仕様は local name） | 直した |
| U1 | tree_core 未実装 2 | `<template for>` による content patching | 未実装（Lexbor にもない） |

### CSS

CSS の項目も、すべて Lexbor の振る舞いを移植版が写したものである（C15 だけは移植版の値の正規化から生じた）。
移植版の比較テスト（`lexbor::selector_diff`、`lexbor::stylesheet_diff`、`selector_port_tests`）は Lexbor と比べるので、どれも捕まえない。
番号は差分表の stylesheet の項目（C1〜C3）に続ける。

| id | 表 | 内容 | 状態 |
|---|---|---|---|
| C4 | css_tokenizer F1 | 入力の終わりの `\` で終わる文字列に U+FFFD を足す | 未着手 |
| C5 | css_tokenizer F2、F3 | `-` の後の非 ASCII 文字を、UTF-8 の先頭 byte で名前の始まりと判定する | 未着手 |
| C6 | css_tokenizer F4 | 129 桁目以降の整数部を捨てる | 未着手 |
| C7 | css_tokenizer F5、css_selectors F5 | hash token に type flag がなく、`#1a` を ID セレクタとして受け付ける | 未着手 |
| C8 | css_tokenizer F6 | 空白だけが続いて終わる `url(` を function token にする | 未着手 |
| C9 | css_syntax F1 | ブロックの中で `ident:` で始まる規則を宣言として読む | 未着手 |
| C10 | css_syntax F2 | ブロックのないまま終わる qualified rule を規則にする | 未着手 |
| C11 | css_syntax F3 | 最上位の `--foo:hover {}` を style rule にする | 未着手 |
| C12 | css_syntax F4 | `!important` を byte で探す（`! important` を見落とす） | 未着手 |
| C13 | css_syntax F5 | 括弧の中の `!important;` で値を切る | 未着手 |
| C14 | css_syntax F6 | 名前とコロンの間のコメントで宣言でなくなる | 未着手 |
| C15 | css_syntax F7 | 値の正規化が `\` の後の改行を空白に変え、意味を変える | 未着手 |
| C16 | css_syntax U1 | at-rule のブロックの中の宣言を出力しない | 未実装 |
| C17 | css_syntax U2 | style rule の入れ子の規則と、その後の宣言を出力しない | 未実装 |
| C18 | css_selectors F1 | 空の `:is()`、`:where()` を構文エラーにする | 未着手 |
| C19 | css_selectors F2 | `:has()` の中の無効な項目を捨てる（仕様は unforgiving） | 未着手 |
| C20 | css_selectors F3 | `:has()` の中の `:has()` を受け付ける | 未着手 |
| C21 | css_selectors F4 | An+B の後ろの token を捨てる（`:nth-child(2n+1of p)`） | 未着手 |
| C22 | css_selectors F6 | `<ndashdigit-*>` の B が大きすぎると構文エラーにする | 未着手 |
| C23 | css_selectors F7 | 引数のない擬似クラスと `:nth-of-type()` を specificity の B に数えない | 未着手 |
| C24 | css_selectors F8 | `:is()` などの specificity を最も詳細な項目でなく、成分ごとに 1 に丸める | 未着手 |
| C25 | css_selectors F9 | 名前空間の接頭辞のある type selector を C に二度数える | 未着手 |
| C26 | css_selectors F10 | specificity の成分を丸めず、上の成分に繰り上げる | 未着手 |
| C27 | css_selectors U1 | `[*\|a]`（任意の名前空間の属性）を受け付けない | 未実装 |

監査で意図した差とした行のうち、二つの差分文書に書いていないものが三つある。入力の NUL と不正な UTF-8 を例外にする（`CLAUDE.md` の text-input contract）、at-rule の入れ子が 64 を超えると例外にする（`lexbor/stylesheet.rs` の `MAX_DEPTH`）、メモリ不足で stylesheet の解析全体を失敗にする（S3 と同じ方針）である。

parse error の記録に関する差（記録されない error、位置の違い）は、木を変えないので一覧に入れていない。
各表の備考に残してある。
