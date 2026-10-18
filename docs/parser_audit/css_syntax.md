# CSS 構文パーサ監査：CSS Syntax Level 3 の §5 Parsing と、stylesheet パーサの出力

- 対象の仕様：CSS Syntax Level 3 `css-syntax-3/Overview.bs`（w3c/csswg-drafts commit `7b4c69ecaf9289d1bd5455d6bea10eec0b4a4399`）。行番号はこのファイルの行である。
  - §5 Parsing（1839–3308）：railroad diagram、Parsing Results、Token Streams、Parser Entry Points のうち parse a stylesheet と、セレクタの解析が使う入口、Parser Algorithms のすべて（入れ子の規則と nested declarations rule を含む）
  - 入力の前処理（filter code points、441–470）は、入口の normalize が呼ぶ範囲だけ
  - §8 CSS stylesheets（3704–3834。style rule、at-rule、`@charset`）と §9 Serialization（3846–3967）は、`parse_stylesheet` の出力の文字列（セレクタの文字列、prelude、値、名前）に関わる範囲だけ
  - 範囲外：トークナイザ（§4）、An+B（§6）、セレクタの文法そのもの（別の監査）
- 対象の実装：Makiri の CSS 構文パーサ（Lexbor の `css/syntax/parser.c`、`css/state.c`、`css/at_rule/state.c` の移植）、規則のアリーナ、規則を所有値に読み取る層、Ruby への出力
- 意図した差として扱うのは、`docs/rewrite_parser.ja.md` の「stylesheet パーサ」（2026-10-18 決定）と `docs/parser_differences.ja.md` の S3、S4、「stylesheet の読み取り」（C1–C3）に記録された差である。新しい差としては数えていない。

## 読み方

### 略号

| 略号 | パス（リポジトリ相対） |
|---|---|
| `Y` | `ext/makiri/rust/src/selectors/parser/syntax.rs`（stylesheet の phase と callback） |
| `P` | `ext/makiri/rust/src/selectors/parser.rs`（規則のスタック、状態のスタック、`run`、汎用の phase） |
| `K` | `ext/makiri/rust/src/selectors/tokenizer.rs`（`lookup_*` の先読み） |
| `R` | `ext/makiri/rust/src/selectors/rules.rs`（規則のアリーナ） |
| `L` | `ext/makiri/rust/src/lexbor/stylesheet.rs`（規則の読み取りと値の正規化） |
| `G` | `ext/makiri/rust/src/glue/stylesheet.rs`（Ruby への出力） |
| `X` | `vendor/lexbor/source/lexbor/css/syntax/parser.c`（Lexbor、参照のみ） |
| `XT` | `vendor/lexbor/source/lexbor/css/syntax/tokenizer.c`（Lexbor、参照のみ） |

### テストの欄

- `R:` は `spec/lexbor_css_spec.rb` の example（`it` の説明を書く）。期待値を人が書いたテストで、この監査では仕様の答えと照らした。
- `IC:` は `spec/input_cost_spec.rb`、`NS:` は `spec/native_stack_guard_spec.rb` の example。
- `T:` は `ext/makiri/rust/src/lexbor/tests.rs` のテスト（モジュール名と関数名）。
- `D:` は `ext/makiri/rust/src/lexbor/selector_port_tests.rs` の `the_stylesheet_parser_builds_lexbors_rules`（797–948）の手書きの入力。`lexbor::stylesheet_diff::check` で、移植版の規則の木を Lexbor の木と比べるだけで、仕様とは比べない。仕様から逸れていても Lexbor と同じなら通る。`D:` だけが当たる行は、経路を通すテストはあるが答えを確かめるテストはない、という意味である。同じファイルの `..._on_random_inputs` と `..._on_token_soup` も Lexbor との比較で、表では個別に挙げない。
- `src/selectors/` には stylesheet のテストモジュールがない（`strtod.rs` のテストだけ）。`spec/conformance/css_*` はセレクタの照合の比較で、stylesheet の構文は通らない。
- 「手動確認」は、この監査で組み込み済みの Ruby 拡張に入力を与えて確かめたことを示す。「Lexbor で確認」は、`vendor/lexbor/dist/lib/liblexbor_static.a` に監査用の小さな C プログラム（リポジトリには含めない）をリンクし、`lxb_css_stylesheet_parse` の規則の木を出力して確かめたことを示す。

### 状態の欄

一致、意図した差、未実装、逸脱の四つを使う。parse error の報告は、出力が変わらない限り状態を変えない。`parse_stylesheet` は parse error を返さず、移植版は Lexbor の log も持たない（`P:20`）ので、どの分岐についても parse error は観測できない。

「valid in the current context」（各 consume の最後の判定）は、Syntax の外（各規則の文法）が定める判定である。`parse_stylesheet` はこれを行わず、`:bad_style` や Lexbor の型による at-rule として呼び出し側に渡す設計なので、その判定を含む行は意図した差とした（11 章）。

### 実装の構造（前提）

- **Lexbor の制御フローのまま。** 規則（rule list、at-rule、qualified rule、block、declaration list）ごとに phase と state を持ち、phase が token を見て規則の終わりの token を `_END` に変え、state が token を消費する（`P:7-25`、`P:448-536`）。再帰はせず、規則を heap のスタックに積む。
- **宣言か規則かは先読みで決める。** 仕様の consume a block's contents は、まず宣言として読み（mark）、宣言にならなければ巻き戻して（restore）規則として読み直す。実装は mark と restore を持たず、ident の後に（空白を飛ばして）`:` があるかを入力のバイト列で調べ（`K:164-184` の `lookup_colon`）、あれば宣言、なければ qualified rule と最初に決める（`Y:420`、`Y:667`、Lexbor の `X:1180`）。宣言として読み始めたものが仕様では宣言にならない場合を扱えない。後述の **F1**、**F6** の原因である。
- **値は component value の木にしない。** 括弧の対応だけを、閉じ括弧の種類のスタック（`P:565-571`、`Y:231-251`）で数え、宣言の値は source の offset の範囲（`value_begin`、`value_end`）として持つ（`Y:718-735`、`R:64-69`）。at-rule と qualified rule の prelude も offset の範囲で持つ。
- **`!important` の判定は先読み。** 値の中の `!`（または値の終わりの直前の空白）で、その後ろを入力のバイト列で見て判定する（`K:188-261`）。仕様は値を最後まで読んでから末尾の二つの token を見る。後述の **F4**、**F5** の原因である。
- **block の結果は rule のリスト。** block の中の宣言の連続は `DeclarationList` という項目一つになり、規則と並んで block のリストに入る（`Y:431-452`）。qualified rule は、block のリストの最初の項目が `DeclarationList` ならそれを declarations にし、残りを child に置く（`Y:881-927`）。仕様の「残りの宣言のリストを nested declarations rule にする」は、`DeclarationList` のまま child に残ることに当たる。
- **qualified rule の prelude は、セレクタの状態がそのまま token 列から読む**（`Y:824-842` が `St::ComplexList` に切り替える）。phase が `{` を `_END` に変えるので、セレクタの解析はそこで終わる。セレクタが失敗すれば prelude を読み飛ばし（`St::SkipToEnd`）、規則は `BadStyle` になる（`Y:846-877`）。
- **コメントはパーサに届かない。** トークナイザが捨てる（`K:286-288`、`with_comment` は false）。ただし offset はコメントを含めて進むので、source の範囲として取り出した prelude と値にはコメントが残る。
- **出力は読み取り層が作る**（`L:259-328`）。style rule は selectors と declarations、`:bad_style` は prelude の文字列と declarations、at-rule は name と prelude と block の中の規則を返す。style rule の child と、at-rule の block の中の `DeclarationList` は出力しない（`L:266-287`、`L:320-321`）。後述の **U1**、**U2** である。
- **仕様の文面の欠落について。** consume a block's contents の `<EOF-token>` と `<}-token>` の分岐（2939–2942）は「Return |rules|」だけで、溜めた |decls| を |rules| に足す step がない。文面どおりなら `a{color:red}` は宣言を一つも持たないことになり、qualified rule の step（2867–2869）とも合わない。この監査は、返す前に空でない |decls| を |rules| に足す、という意図で読んだ。

---

## 1. CSS Parsing Results と Token Streams（1994–2186）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| stylesheet は rule のリストを持つ（2001–2002） | `Y:29-59`（根の `Item::List`）、`R:28-32` | 一致 | R: "returns style rules with selectors and declarations in source order" | |
| rule は at-rule か qualified rule（2004–2006） | `R:35-56`（`Style`、`BadStyle`、`At`） | 一致 | R: "surfaces an unknown at-rule (as :at_rule) and keeps a following good rule" | |
| at-rule：name、prelude、block at-rule は declarations と child rules を持つ（2008–2019） | `R:50-56`（block は `Item::List`。その中に規則と `DeclarationList` が並ぶ）、`Y:799-821` | 一致 | R: "surfaces @media uniformly with name, prelude (condition), and nested rules" | パーサが作る構造は一致する。出力に宣言が出ないことは U1（12 章）。 |
| qualified rule：prelude、declarations、child rules（2021–2030） | `R:35-48`、`Y:881-927` | 一致 | R: "returns style rules with selectors and declarations in source order"、D: "a { c: d; b {} e: f }" | child の出力は U2。 |
| declaration：name、value、important、original text（2032–2049） | `R:64-69`、`Y:718-735` | 一致 | R: "flags !important declarations" | value は token 列でなく source の offset の範囲で持つ。original text は C1 で扱う（8 章）。 |
| component value、preserved tokens、function、simple block（2050–2089） | 木は作らず、括弧の深さだけを数える：`Y:231-251`、`P:574-647` | 一致 | D: "a { x: (a; b) ; c: d }"、"a { x: f(a; b) ; c: d }" | 範囲と深さで、仕様の木と同じ境界が決まる（10 章）。 |
| token stream：next token、consume a token、discard a token、empty、`<eof-token>`（2098–2186） | `K:130-151`（`token`、`consume`）、`P:500-553`、EOF は `K:279-281` | 一致 | 全テスト | discard whitespace は各 phase が空白の token を一つ読み飛ばす形で行う。空白の連続は一つの token なので同じ。 |
| 宣言の値の原文を再現できること（2129–2137） | 値の offset（`Y:637-648`）と入力の文字列（`L:136-169`） | 一致 | R: "treats a custom property's value like any other" | |

## 2. Parser Entry Points と入力の前処理（441–470、2198–2613）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| normalize：文字列なら filter code points をして tokenize する（2209–2224） | `G:140`（`ruby_verified_text`）、`K:105-118` | 一致 | 全テスト | |
| filter code points：CR、FF、CR LF を LF にする（452–456） | トークナイザが空白として扱う。値は空白を一つにする（`L:154`）。prelude と `:bad_style` の文字列は source のまま（`L:217-222`） | 一致 | 手動確認：`a{b:c\r\nd}`、`a{b:c\fd}` の値は `c d` | prelude に CR が書かれていれば、出力の prelude にも CR が残る。Syntax は直列化を定めないので、出力の文字列としての差にとどめる。 |
| filter code points：U+0000 と surrogate を U+FFFD にする（458–460） | `G:140`：NUL を含む文字列は `Makiri::Error` | 意図した差 | R: "rejects a NUL byte" | Makiri のプログラム向け API の入力の契約（`CLAUDE.md` の「Text-input contract」：NUL と不正な UTF-8 は修復せずに例外）。二つの差分文書には記載がない。surrogate は Ruby の UTF-8 の String に入らない。 |
| parse a stylesheet step 1：byte stream なら decode bytes（2393–2395） | `G:140`：UTF-8 として正しくない String は例外。ほかのエンコーディングの String も変換せずに例外 | 意図した差 | R: "rejects invalid UTF-8" | 入力は Ruby の String で、byte stream ではない。手動確認：ISO-8859-1 の String は "must be valid UTF-8" で例外。HTML の解析（`ruby_to_utf8` で変換）とは扱いが違う。先頭の U+FEFF は文字列の一部として残り、ident の文字になる（手動確認：`﻿a{b:c}` のセレクタは `﻿a`）。文字列の入力としては仕様と同じ（U+FEFF は 878 行の範囲の non-ASCII ident code point）。 |
| step 2：normalize（2397–2399） | 同上 | 一致 | 全テスト | |
| step 3：新しい stylesheet を作る（2401–2404） | `Y:31-36` | 一致 | R: "returns an empty array for an empty stylesheet" | location は持たない（該当する値がない）。 |
| step 4：consume a stylesheet's contents、入力をすべて読む（2406–2410） | `Y:37-50`、`P:448-491` | 一致 | R: "never raises a syntax error on broken input" | `run` の呼び出し回数の上限（`P:433`、入力 1 バイトあたり 1000 回）は、移植版の不具合を止めるためのもので、到達しない。 |
| step 5：stylesheet を返す（2412–2413） | `Y:51-58`、`L:331-337`、`G:148-161` | 一致 | R: "returns style rules with selectors and declarations in source order" | |
| （実装の制限）at-rule の入れ子の深さ 64 を超えると例外 | `L:54`、`L:259-262`、`G:151-155` | 意図した差 | R: "fails closed on pathologically deep nesting" | 仕様に上限はない。fail closed の安全性の方針（`CLAUDE.md`）。出力しない style rule の child は数えない。 |
| （実装の制限）メモリ不足は途中までの結果でなく失敗にする | `P:477-485`、`Y:550-563` ほか | 意図した差 | `rake oom` の `css_stylesheet` シナリオ | S3。 |
| parse a stylesheet's contents（2417–2434） | `Y:29-59`（同じ入口） | 一致 | R: "returns style rules with selectors and declarations in source order" | Makiri には別の入口がない。 |
| parse something according to a CSS grammar：セレクタ（`Node#css`）と qualified rule の prelude を `<selector-list>` として解析し、合わなければ失敗（2306–2325、3727–3730） | `P:353-374`（`Node#css`）、`Y:824-877`（prelude） | 一致 | R: "surfaces a syntactically rejected selector as :bad_style too (caller re-validates)"、手動確認：`css("a {")`、`css("a }")`、`css("a;")`、`css("")` は `CSS::SyntaxError` | 文法との照合そのものは別の監査。 |
| parse a list of component values（2580–2590）：`Node#css` は入力の全体を一つの component の規則として読む | `P:360`（stop token なし）、`P:574-604` | 一致 | 手動確認：上の行 | `}`、`)`、`]` は preserved token として残り、セレクタの文法に合わずに失敗する。 |
| parse a comma-separated list of component values（2595–2611）：セレクタのリストのコンマでの分割 | セレクタの状態（`P:1163-1170`、`P:2014-2057` の `list_end`） | 一致 | R: "splits a comma selector list into one entry per branch" | 分割はセレクタの文法の側で行う。分割した各部分の扱いは別の監査。 |

使っていない入口（parse a list of declarations 2436–2467、parse a rule 2469–2506、parse a declaration 2508–2542、parse a component value 2544–2575、parse a comma-separated list according to a CSS grammar 2328–2381）は、Makiri に対応する API がないので表に入れていない。移植版は declaration list を単独で解析する経路（Lexbor の `..._declarations_validate`、`..._declarations_drop`）も持たない（`Y:7-12`）。

## 3. Consume a stylesheet's contents（2640–2695）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `<whitespace-token>`：捨てる | `Y:160-163` | 一致 | R: "returns an empty array for an empty stylesheet"、D: " " | |
| `<EOF-token>`：rules を返す | `Y:168`、`Y:178-179`、`P:650-704` | 一致 | R: "returns an empty array for an empty stylesheet" | |
| `<CDO-token>`、`<CDC-token>`：捨てる | `Y:160-163` | 一致 | D: "<!-- a { b: c } -->"、"a <!-- { b: c }" | 手動確認：`<!-- a{b:c} --> <!-- d{e:f}` は style rule 二つ。 |
| `<at-keyword-token>`：consume an at-rule。返れば足す | `Y:164-167`、`Y:190-227`、`Y:738-744` | 一致 | R: "surfaces an unknown at-rule (as :at_rule) and keeps a following good rule" | |
| anything else：consume a qualified rule。規則が返れば足す | `Y:169-175`、`Y:183-187`、`Y:461-484`、`Y:738-744` | 一致 | R: "returns style rules with selectors and declarations in source order"、D: "} a { c: d }" | 振り分けは一致する。qualified rule が「何も返さない」場合にも規則を作ることは F2、F3（5 章）。根の規則の `block_end` は `T::Undef` なので、トップレベルの `}` は qualified rule の始まりになる（`Y:170`）。 |

## 4. Consume an at-rule（2698–2754）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| at-keyword を消費し、その値を name にする（2705–2711） | `Y:190-206`（`dup(token.string)`：エスケープを解いた値） | 一致 | D: "@\\66oo { }"、R: "lower-cases an at-rule name" | name の後の空白 token を一つ読み飛ばしてから prelude を始める（`Y:222-225`）。prelude の始まりの offset だけに効く。 |
| （出力）name を ASCII 小文字にする | `L:306-313` | 意図した差 | R: "lower-cases an at-rule name" | C2。 |
| （Lexbor の型）`media`、`font-face`、`namespace` を型付きで作り、prelude か block が型に合わなければ `_undef` にして名前を空にする | `Y:63-73`、`Y:320-332`、`Y:774-794`、`Y:806-821`、`L:306-308` | 意図した差 | D: "@media;"、"@media { a {} }"、"@font-face x { src: y }"、"@namespace x {a{}}"、手動確認：`@media screen;` の name は `""` | C2 と「stylesheet パーサ」の決定（構造は Lexbor のまま）。prelude と block の中身は保つ。 |
| `<semicolon-token>`、`<EOF-token>`：捨てて、規則を返す（2716–2722） | `Y:294`（`;` は `AtPreludeEndConsumeToken` で消費）、`Y:295`、`Y:335-350` | 一致 | R: "surfaces other at-rules (e.g. @keyframes, @import) without dropping them"、D: "@foo bar baz; @qux { }"、"a { b: c } @foo" | 手動確認：`@foo bar`（EOF）は at-rule になる。 |
| `<}-token>`、nested が true：規則を返す（`}` は消費しない）（2724–2729） | `Y:281-287`（`nested` は `Y:522` で true） | 一致 | 未カバー | 手動確認：`@media x { @foo bar } a{b:c}` は `@foo` が `}` で終わり、`@media` の block が閉じて `a` が続く。Lexbor で確認：同じ。 |
| `<}-token>`、nested が false：prelude に足す（2731–2732） | `Y:281-288`（深さ 0、nested でないので token を返す） | 一致 | 未カバー | 手動確認：`@foo } x; a{b:c}` の prelude は `} x`。 |
| `<{-token>`：consume a block を child rules にし、規則を返す（2734–2748） | `Y:270-276`、`Y:320-332`、`Y:377-397` | 一致 | R: "surfaces @media uniformly with name, prelude (condition), and nested rules"、R: "surfaces @supports with its condition prelude" | block の中の宣言を出力しないことは U1。 |
| anything else：consume a component value を prelude に足す（括弧の中の `;`、`{`、`}` は prelude の一部）（2750–2753） | `Y:277-280`、`Y:290-293`、`Y:296`、`Y:231-251` | 一致 | D: "@foo [ { ] } a { b: c }"、"@foo ( { ) } a { b: c }" | 手動確認：`@foo [ { ] } a { b: c }` の prelude は入力の終わりまで。 |
| 規則が valid なら返す（2720、2727、2746） | 判定しない | 意図した差 | R: "surfaces an unknown at-rule (as :at_rule) and keeps a following good rule" | 11 章を参照。 |

## 5. Consume a qualified rule（2757–2891）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `<EOF-token>`：parse error。何も返さない（2772–2776） | `Y:295`（`end_phase`）、`Y:363-372`（prelude を閉じて規則を作る） | **逸脱（F2）** | 未カバー | `a{b:c} d` の `d` が style rule になる。 |
| stop token（block の中では `;`）：parse error。何も返さない（2773–2776） | `Y:457`（stop は `T::Semicolon`）、`Y:266-267`、`Y:363-372` | **逸脱（F2）** | D: "a { c: d; b; e: f }"、"a { 1: 2; c: d }"（Lexbor との比較だけ） | block のない規則を作り、block の宣言の列を分ける。 |
| `<}-token>`、nested が true：parse error。何も返さない（2778–2782） | `Y:281-287`（`prelude_done` で規則を作る） | **逸脱（F2）** | 未カバー | 手動確認：`@media x { d }` に style rule `d` ができる。 |
| `<}-token>`、nested が false：prelude に足す（2783–2785） | `Y:281-289` | 一致 | D: "} a { c: d }"、"a } b { c: d }"、R: "never raises a syntax error on broken input" | 手動確認：`a{b:c;}}d{e:f}` の二つ目は `:bad_style`（prelude `}d`）。 |
| `<{-token>`、prelude の最初の二つが `--` で始まる ident と `:`、nested が true：remnants of a bad declaration を読み、何も返さない（2794–2797） | 該当する経路なし。block の中では `lookup_colon` が先に宣言と決める（`Y:420`） | 一致 | 未カバー | 仕様でもこの分岐に来るのは、宣言として読んだカスタムプロパティが文脈で無効なときだけである。手動確認：`a { --foo:hover { x: y } ; c: d }` は宣言 `--foo` と `c`（仕様と同じ）。`@media x { --foo:hover { x: y } ; c{d:e} }` は規則 `c` だけを出力する（仕様でも `<rule-list>` の中の宣言は無効）。 |
| `<{-token>`、同じ条件、nested が false：consume a block をして、何も返さない（2798–2800） | なし（qualified rule として読む） | **逸脱（F3）** | 未カバー | |
| `<{-token>`、それ以外：consume a block。最初の項目が宣言のリストなら declarations にする（2865–2869） | `Y:353-360`、`Y:377-397`、`Y:881-906` | 一致 | R: "returns style rules with selectors and declarations in source order"、D: "a { c: d; b {} e: f }" | |
| 残りの宣言のリストを nested declarations rule にし、child rules にする（2870–2873） | `Y:907-921`（`DeclarationList` のまま `child` に残る） | 一致 | D: "a { c: d; b {} e: f }"、"a { c: d; @foo; e: f }"（Lexbor との比較だけ） | 包む規則の種類はないが、child の中の位置と中身は同じ。出力しないことは U2。 |
| valid なら返し、そうでなければ invalid rule error（2875–2877） | `Y:846-877`（セレクタが失敗すれば `BadStyle`）、`L:288-296` | 意図した差 | R: "surfaces a pseudo-element rule as :bad_style with raw text + declarations"、R: "surfaces a syntactically rejected selector as :bad_style too (caller re-validates)" | 無効な規則を `:bad_style` として返し、検証を呼び出し側に任せる設計（`docs/parser_differences.ja.md` の「stylesheet の読み取り」が構造に含める）。block の中では `BadStyle` も規則として宣言の列を分けるので、invalid rule error と同じ効果になる（7 章）。 |
| anything else：consume a component value を prelude に足す（2882–2885） | `Y:277-280`、`Y:290-293`、`Y:296` | 一致 | D: "a[x=\"}\"] { b: \"}\" }"、"a; b { c: d }" | |
| prelude をセレクタに渡す（2023–2030、3727–3730） | `Y:824-842`（`St::ComplexList`、`prelude_begin`）、`Y:846-877`（`prelude_end` は `{` の offset） | 一致 | R: "splits a comma selector list into one entry per branch"、R: "computes :is/:not/:where specificity the way Selectors L4 does" | 入れ子の qualified rule の prelude も同じ `St::ComplexList` で読む。CSS Nesting の relative selector と `&` は Syntax の外で、入れ子の規則は出力もしない（U2）。 |

## 6. Consume a block（2894–2908）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `{` を捨て、block's contents を読み、`}` を捨てる | `Y:377-397`（`{` を消費して block の規則を積む）、`Y:411-413`（`}` は `EndConsumeToken`）、`Y:418`（EOF） | 一致 | R: "surfaces @media uniformly with name, prelude (condition), and nested rules"、D: "@media x { a { b: c } " | |

## 7. Consume a block's contents（2911–3064）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `<whitespace-token>`、`<semicolon-token>`：捨てる | `Y:407-410`、`Y:659-662` | 一致 | D: "a { ;;; c: d;;; }"、"a { b: c; ; d: e }" | |
| `<EOF-token>`、`<}-token>`：rules を返す（溜めた宣言を足す。「実装の構造」の最後の項を参照） | `Y:411-418`、`Y:663-666`、`Y:705-711` | 一致 | R: "returns style rules with selectors and declarations in source order"、D: "a { x: y" | |
| `<at-keyword-token>`：decls が空でなければ rules に足し、consume an at-rule（nested）、返れば足す（2944–2953） | `Y:419`、`Y:522`（`nested` は true）、`Y:677-682`（宣言の列を閉じる）、`Y:747-753` | 一致 | D: "a { c: d; @foo; e: f }"、"a { @media x { b {} } c: d }" | |
| anything else：宣言として読み、返れば decls に足す（2955–2963）。ident、空白、`:` で始まる場合 | `Y:420`、`Y:431-452`、`Y:667-676`（`lookup_colon`、`K:164-184`） | 一致 | R: "flags !important declarations"、D: "a { c : d }" | 値に `{}`-block を含む宣言は F1（8 章）。 |
| 同上、名前と `:` の間にコメントがある場合 | `K:175-183`（入力のバイト列を見て、`/` で偽を返す） | **逸脱（F6）** | 未カバー（D: "a { c/**/: d }" は Lexbor との比較だけ） | 宣言にならず qualified rule になる。 |
| 宣言にならなければ restore して、consume a qualified rule（nested、stop token は `;`）（2965–2968） | `Y:421`、`Y:455-458`、`Y:461-484` | 一致 | D: "a { b {} c:d }"、"a { c: d; b {} e: f }" | 振り分けは先読みで行う（「実装の構造」）。 |
| 何も返らなかった：何もしない（2971–2972） | 何も返らない場合がない（F2 の三つの場合に規則を作る） | **逸脱（F2）** | D: "a { c: d; b; e: f }"（Lexbor との比較だけ） | 作った規則が宣言の列を分けるので、後の宣言が declarations から落ちる。 |
| invalid rule error：decls が空でなければ rules に足す（2974–2979） | `BadStyle` を規則として block のリストに足す（`Y:747-753`）。次の宣言は新しい `DeclarationList` になる（`Y:431-452`） | 一致 | 未カバー | 手動確認：`a { c: d; 1x {} e: f }` の declarations は `c: d` だけ（仕様と同じ。`e: f` は nested declarations rule）。 |
| 規則が返った：decls が空でなければ rules に足し、規則を足す（2981–2986） | 同上 | 一致 | D: "a { c: d; b {} e: f }"、"a { b {} c:d }" | |

## 8. Consume a declaration と consume the remnants of a bad declaration（3068–3175）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| step 1：ident なら消費し、その値を name にする（3080–3084） | `Y:488-510`、`Y:667-676` | 一致 | R: "reads an escaped name as the name it spells" | 手動確認：`a{b\:c: d}` の名前は `b:c`、`a{--:x}` の名前は `--`。 |
| （出力）カスタムプロパティでない名前を ASCII 小文字にする | `L:124-130` | 意図した差 | R: "lower-cases a declaration name unless it is a custom property" | C2。仕様でもプロパティ名は ASCII case-insensitive（3746）。カスタムプロパティの判定はエスケープを解いた値で行う。 |
| step 1：ident でなければ remnants を読み、何も返さない（3086–3089） | 宣言として読まず、qualified rule として読む（`Y:421`） | 一致 | D: "a { 1: 2; c: d }" | 仕様でも何も返さずに qualified rule として読み直す。そこで規則を作ることは F2。 |
| step 2：空白を捨てる（3091–3092） | `Y:515-518` | 一致 | D: "a { c : d }"、R: "lower-cases a declaration name unless it is a custom property"（`--Mixed: z` の前後の空白） | |
| step 3：`:` なら捨てる（3094–3096） | `Y:519`、`Y:526` | 一致 | R: "returns style rules with selectors and declarations in source order" | |
| step 3：`:` でなければ remnants を読み、何も返さない（3098–3101） | `Y:519-525`（不正なデータとして解析全体を失敗させる） | 意図した差 | 未カバー（到達しない） | S4。`lookup_colon` が先に `:` を確かめているので到達しない。 |
| step 4：空白を捨てる（3103–3104） | `Y:528-532` | 一致 | R: "treats a custom property's value like any other" | `b: /* x */ c` のようにコメントで空白 token が二つになっても、値の正規化（`L:154`）が先頭の空白を捨てる。 |
| step 5：consume a list of component values（nested、stop token は `;`）を値にする（3106–3110） | `Y:566-648` | 一致 | D: "a { x: (a; b) ; c: d }"、"a { x: [a; b] ; c: d }"、"a { x: {a; b} ; c: d }"、"a { x: f(a; b) ; c: d }"、"a { x: f(a}; c: d }"、"a { x: ); c: d }" | 9 章を参照。 |
| step 6：末尾の非空白 token 二つが `!` と `important`（ASCII case-insensitive）なら、取り除いて important を立てる。基本の形（`!important`、`! important` ではない） | `Y:577-594`、`Y:611-621`、`Y:550-563`、`K:188-232`、`K:241-261` | 一致 | R: "flags !important declarations"、R: "leaves !important out of the value"、D: "a { x: y !important; z: w!IMPORTANT }"、"a { b: c\t!important\n}" | 手動確認：`a{b: !important}` は値が空で important。`a{b: c !important !important}` は値 `c !important` で important（仕様と同じ）。 |
| 同上、`!` と `important` の間の空白やコメント、`important` のエスケープ、後ろのコメント | `K:188-232`（`!` の直後のバイト列を `important` と比べ、その後ろは空白か `;` か `}` だけを許す） | **逸脱（F4）** | D: "a { x: y ! important }"、"a { b: c! important }"（Lexbor との比較だけ） | |
| 同上、括弧の中の `!important` | `Y:611-621`（`!` の分岐は深さを見ない） | **逸脱（F5）** | 未カバー | |
| step 7：末尾の空白 token を取り除く（3119–3121） | `Y:585`（空白の前で `value_end`）、`L:154` | 一致 | R: "collapses the whitespace between a value's tokens"、D: "a { x: y " | |
| step 8：カスタムプロパティなら original text を値の原文にする（3123–3128） | `L:136-169`（すべての値を同じ規則で正規化する） | 意図した差 | R: "treats a custom property's value like any other" | C1。カスタムプロパティの値の空白も詰める。 |
| step 8：カスタムプロパティでなく、値に top-level の `{}`-block と、ほかの非空白の値があれば、何も返さない（3130–3135） | なし | **逸脱（F1）** | 未カバー | `{}`-block だけの値は宣言のまま（手動確認：`a { b: {c} ; d: e }`）。 |
| step 8：`unicode-range` なら値の原文を unicode-range を許して tokenize し直す（3137–3142） | 値は source の文字列として返す（`L:136-169`） | 一致 | 未カバー（`..._on_random_inputs` の値に `U+00-ff` がある） | `normalize_value` は unicode-range を許さずに tokenize するが、空白でない token は書かれたとおりに並べるだけなので、出力の文字列は変わらない。 |
| step 9：valid なら返す（3147–3150） | 判定しない。値はすべて一つの状態で読む（`Y:930-933`） | 意図した差 | R: "keeps every token but whitespace as written" | C3。 |
| remnants of a bad declaration：`;`、EOF で捨てて終わる。`}`（nested）で終わる。ほかは component value（3153–3175） | 対応する経路はない。宣言にならないものは qualified rule として読む（`Y:421`） | 一致 | D: "a { 1: 2; c: d }" | 仕様でも remnants の結果は捨てられ、mark に戻って qualified rule として読み直すので、出力に効かない。 |

## 9. Consume a list of component values（3178–3210）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `<eof-token>`、stop token：values を返す | 宣言の値：`Y:573-575`、`Y:595-610`、`Y:630`。`Node#css`：`P:579-580` | 一致 | D: "a { x: y"、"a { x: (a; b) ; c: d }" | |
| `<}-token>`、nested が true：values を返す | `Y:573-575`（宣言の規則の `block_end` は `}`、`Y:444`） | 一致 | R: "returns style rules with selectors and declarations in source order" | |
| `<}-token>`、nested が false：parse error。token を values に足す | `P:586-591`（`Node#css`） | 一致 | 手動確認：`css("a }")` は `CSS::SyntaxError` | stylesheet では nested でない値の読み取りがない。 |
| anything else：consume a component value を足す | `Y:622-631`、`P:582-599` | 一致 | D: "a { color: rgb(1, 2, 3) }" | |

## 10. Consume a component value、simple block、function（3213–3291）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `{`、`[`、`(`：consume a simple block（3221–3225） | `Y:231-243`（閉じ括弧を積んで深さを足す）、`P:582-599` | 一致 | D: "a { x: [a; b] ; c: d }"、"a { x: {a; b} ; c: d }" | |
| `<function-token>`：consume a function（3227–3229） | 同上（`)` を積む） | 一致 | D: "a { x: f(a; b) ; c: d }"、"a { width: calc(1px + 2px) !important }" | |
| anything else：token を消費して返す（3231–3233） | 各 phase の既定の分岐（`Y:296`、`Y:631`） | 一致 | 全テスト | |
| simple block：対になる閉じ括弧か EOF で終わる（3247–3262） | `Y:246-251`（スタックの一番上と同じ種類のときだけ閉じる） | 一致 | D: "a { x: f(a}; c: d }"、"@foo [ { ] } a { b: c }" | 対にならない閉じ括弧は中身の token になる。手動確認：`a { x: f(a}; c: d } e{f:g}` は値が入力の終わりまで続く。 |
| function：`)` か EOF で終わる（3284–3287） | 同上 | 一致 | D: "a { b: url(}) }"、"a { x: f(a}; c: d }" | 手動確認：`a{b:url(x` の値は `url(x`。 |

## 11. CSS stylesheets、style rules、at-rules、`@charset`（3704–3834）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| トップレベルの qualified rule は style rule。prelude が `<selector-list>` に合わなければ規則は invalid で、捨てる（3707–3714、3727–3730） | `Y:846-877`、`L:266-296` | 意図した差 | R: "surfaces a pseudo-element rule as :bad_style with raw text + declarations" | 捨てずに `:bad_style` として返す。セレクタの文字列に書き戻せない規則も `:bad_style` にする（`L:274-286`）。 |
| 認識しない at-rule、文法に合わない at-rule は捨てる（3711–3714） | `L:297-319`（すべて返す） | 意図した差 | R: "surfaces an unknown at-rule (as :at_rule) and keeps a following good rule"、R: "surfaces (does not drop) recognized at-rules like @font-face" | 判定は呼び出し側。型付きの at-rule の `_undef` は C2。 |
| style rule の block は宣言として読む。at-rule は、ほかの仕様が許さなければ無効（3732–3740） | `Y:881-927`、`L:273` | 一致 | R: "returns style rules with selectors and declarations in source order" | 入れ子の規則の出力は U2。 |
| 未知のプロパティや値の合わない宣言は無効（3742–3745） | 判定しない | 意図した差 | R: "keeps every token but whitespace as written" | C3。 |
| プロパティ名は ASCII case-insensitive。カスタムプロパティの名前は case-sensitive（3746–3748） | `L:124-130` | 一致 | R: "lower-cases a declaration name unless it is a custom property" | |
| `@charset` は認識しない規則として扱う（3823–3826） | `Y:63-73`（`AtKind::Custom`） | 一致 | D: "@charset \"utf-8\";" | 手動確認：名前 `charset` の at-rule として返り、後の規則は残る。fallback encoding の判定（3795–3797）は、入力が Ruby の String なので該当しない（2 章）。 |

## 12. 出力の文字列（Serialization 3846–3967）と出力の範囲

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| コメントは保持してよいが、解析に影響してはならない（3849–3852） | `K:286-288`（パーサには届かない）、`L:142`（値の文字列には残す） | 一致 | R: "keeps a comment inside a value" | ただし `!important` の判定と、宣言名の後の `:` の判定は入力のバイト列を見るので、コメントが解析に影響する（F4、F6）。 |
| 直列化は解析と round-trip すること。空白の連続は一つにしてよい（3858–3862）：宣言の値 | `L:136-169` | 一致 | R: "collapses the whitespace between a value's tokens"、R: "keeps every token but whitespace as written" | C1。 |
| `\` の `<delim-token>` は、`\` と改行で書く（3871–3876） | `L:154-165`（`\` の後の空白を空白一つにする。末尾なら削る） | **逸脱（F7）** | 未カバー | |
| style rule のセレクタの文字列 | `L:197-213`、`ext/makiri/rust/src/lexbor/selector_text.rs` | 一致 | T: `selector_text::written_text_reads_back_as_itself`、`selector_text::any_name_or_value_survives_the_round_trip`、R: "selector text" の各 example、NS: "writes deeply nested selector lists back in full" | CSSOM の直列化。セレクタの直列化の詳細は別の監査。 |
| `:bad_style` の prelude と at-rule の prelude：source の範囲から CSS の空白だけを前後から削る。エスケープが持つ空白は残す | `L:217-257` | 一致 | R: "trims a :bad_style prelude"、R: "keeps an escaped trailing space"、R: "never leaves a lone trailing backslash"、R: "keeps it in an at-rule prelude"、R: "trims only CSS whitespace"、IC: "takes each prelude from the rule itself, not an identical piece elsewhere" | prelude の中の空白とコメントは書かれたとおり（値と違って詰めない）。 |
| at-rule の block の宣言（2015、2739–2744）を出力する | `L:320-321`（`DeclarationList` を出力しない） | **未実装（U1）** | 未カバー | |
| style rule の child rules（入れ子の規則と nested declarations rule、2026、2870–2873）を出力する | `L:266-287`（`child` を読まない） | **未実装（U2）** | 未カバー | |

---

## 集計

| 状態 | 件数 |
|---|---|
| 一致 | 69 |
| 意図した差 | 15 |
| 未実装 | 2（U1、U2） |
| 逸脱 | 11 行（F1–F7 の 7 件。F2 は 4 行に当たる） |

一致の行のうち、Lexbor との比較（`D:`）しかテストがない行と、テストがない行は「未カバーの step」に挙げる。parse error は、どの分岐でも記録も報告もしない（「状態の欄」を参照）。

意図した差の内訳：C1（値の正規化、カスタムプロパティの original text）、C2（名前の小文字化、型付きの at-rule の `_undef`）、C3（値の検証をしない）、S3（メモリ不足）、S4（宣言名の後のコロン）、`:bad_style` と未知の at-rule を返す設計（valid の判定を呼び出し側に任せる）、入力の契約（NUL と不正な UTF-8 は例外。`CLAUDE.md` に記録、二つの差分文書には記載なし）、入れ子の深さの上限（64）。

---

## 逸脱と未実装

逸脱のうち F1–F6 は、Lexbor の構文パーサの振る舞いを移したもので、Lexbor で確認した（監査用の C プログラム）。F7 は移植版の読み取り層（C1 の正規化）で生じ、Lexbor の構文パーサとは関係しない。

### F1 値に `{}`-block とほかの値が並ぶ宣言を、宣言のまま残す（`ident:` で始まる規則が宣言に吸い込まれる）

- **仕様**：
  - consume a declaration step 8（3130–3135）：「Otherwise, if decl's value contains a top-level simple block with an associated token of <{-token>, and also contains any other non-<whitespace-token> value, return nothing.」
  - consume a block's contents の anything else（2955–2968）：宣言として何も返らなければ、mark に戻り、consume a qualified rule（nested、stop token は `;`）で読み直す。実装上の早期の打ち切り（3027–3033「font:bar {...」は無効なプロパティ）も同じ結論を述べている。
- **コード**：
  - block の中で、ident の後に（空白を飛ばして）`:` があれば宣言と決める（`Y:420`、`Y:667`、`K:164-184`。Lexbor の `X:1180`、`X:1829`、`XT:197`）。
  - 宣言の値の phase は `{` を括弧として数えるだけで（`Y:622-629`）、step 8 の検査がない。値は `;` か、深さ 0 の `}` まで続く。
  - at-rule の block も同じ block の phase で読む（`Y:331`、`Y:377-397`）ので、`@media` などの中でも同じことが起きる。宣言の列は at-rule の出力に出ないので（U1）、その規則は出力から消える。
- **再現**：
  - `@media (max-width:600px){ .x{a:b} a:hover{color:red} .y{c:d} }`
    - 仕様：`@media` の block は style rule `.x`、`a:hover`、`.y` の三つ。
    - 実装（手動確認）：規則は `.x` だけ。`a` を名前とする宣言の値が `hover{color:red} .y{c:d}` になり、出力されない。Lexbor で確認：同じ（`decl a = "hover{color:red} .y{c:d}"`）。
    - 同じく消える：`@media print { p::before { content: "x" } }`、`@supports (x:y) { input:focus { a: b } }`（どちらも `rules` が空）。
  - `a { b: x {c} ; d: e }`
    - 仕様：`b: x {c}` は宣言にならず、qualified rule `b: x` として読まれる（セレクタとして無効なので invalid rule error）。declarations は `d: e` だけ。
    - 実装（手動確認）：declarations は `b: "x {c}"` と `d: e`。
  - `a { b:hover { x: y } }`
    - 仕様：入れ子の style rule `b:hover`。declarations は空。
    - 実装（手動確認）：declarations は `b: "hover { x: y }"`。
- **影響**：型セレクタの直後に擬似クラスか擬似要素を書いた規則（`a:hover`、`li:first-child`、`p::before`、`input:focus`）を `@media`、`@supports` の中に書くと、その規則と、同じ block の後ろの規則が、閉じる `}` まで失われる。型セレクタで始まらない規則（`.x:hover`、`nav a:hover`）は影響を受けない。
- **確信度**：高い。仕様の文は明確で、実装に該当する検査がない。三つの入力を Ruby 拡張と Lexbor の両方で確かめた。

### F2 block のない qualified rule（EOF、stop token、入れ子の `}` で終わるもの）を規則として作る

- **仕様**：consume a qualified rule の `<EOF-token>` と stop token（2772–2776）、`<}-token>` で nested が true（2778–2782）は、いずれも「Return nothing」。consume a block's contents は、何も返らなければ何もしない（2971–2972）。宣言の列（|decls|）も分けない。
- **コード**：prelude の phase は、EOF（`Y:295`）、block の中の stop token `;`（`Y:457` の `T::Semicolon` と `Y:266-267`）、nested の `}`（`Y:284-287`）で `QualifiedPreludeEnd` に進み、`Y:363-372` が prelude を閉じて規則を作る（セレクタが読めれば `Style`、読めなければ `BadStyle`）。Lexbor の `X:938-1032` の `done:` と同じ。その規則は block のリストに入り（`Y:747-753`）、宣言の列を分ける。qualified rule は block のリストの最初の `DeclarationList` だけを declarations にする（`Y:888-906`）。
- **再現**（いずれも手動確認、Lexbor で確認）：
  - `a{b:c} d`：仕様は規則 `a` だけ。実装は style rule `d`（declarations は空）も返す。`a{b:c} }` は `:bad_style`（prelude `}`）も返す。
  - `@media x { b c; a{b:c} }`：仕様は `@media` の中に規則 `a` だけ。実装は style rule `b c` も返す。`@media x { d }` も style rule `d` を返す。
  - `a { 1: 2; c: d }`：仕様の declarations は `c: d`。実装の declarations は空（block のリストの最初が `BadStyle` `1: 2` になり、`c: d` は child に回る）。`a { b c; d: e }` も declarations が空。
  - `a { c: d; b; e: f }`：仕様の declarations は `c: d` と `e: f`。実装は `c: d` だけ（`e: f` は style rule `b` の後の、別の宣言の列になる）。
- **確信度**：高い。

### F3 トップレベルのカスタムプロパティのような prelude を style rule にする

- **仕様**：consume a qualified rule の `<{-token>`（2787–2800）：prelude の最初の二つの非空白の値が `--` で始まる ident と `:` なら、nested が false のときは block を読んで何も返さない。注（2856–2862）は「`--foo:hover { color: blue; }` is consistently invalid everywhere」と述べる。
- **コード**：`Y:353-360`、`Y:824-877`。prelude を検査せずにセレクタの解析に渡す。`--foo` はセレクタとしては正しい型セレクタなので、style rule になる。Lexbor の `X:938-1032` と同じ。
- **再現**：`--foo:hover { color: blue } a{b:c}`
  - 仕様：規則 `a` だけ。
  - 実装（手動確認）：style rule（セレクタ `--foo:hover`、specificity `[0, 0, 1]`、declarations `color: blue`）と `a`。Lexbor で確認：同じ。
- **影響**：小さい。`--foo` という名前の要素がなければ照合に当たらない。
- **確信度**：高い。

### F4 `!important` を source のバイト列で判定する（空白、コメント、エスケープ）

- **仕様**：consume a declaration step 6（3112–3117）：値の末尾の二つの非空白 token が `!` の `<delim-token>` と、値が「important」に ASCII case-insensitive に一致する `<ident-token>` なら、important を立てる。コメントは token にならず、ident の値はエスケープを解いた値である。railroad diagram（1934–1941）も `!` と `important` の間と後ろに `ws*` を許す。
- **コード**：`!` の delim で `lookup_important` を呼び（`Y:611-621`）、空白の後でも `lookup_declaration_ws_end` から呼ぶ（`Y:577-594`）。先読みの token がないとき（stylesheet では通常こちら）は、`!` の直後のバイト列を `important` と比べ（`K:213-220`）、その後ろに空白、`;`、`}`、入力の終わりしか許さない（`K:223-232`）。Lexbor の `XT:242`、`XT:294`、`XT:313` と同じ。
- **再現**（いずれも手動確認、Lexbor で確認）：

  | 入力 | 仕様 | 実装 |
  |---|---|---|
  | `a { x: y ! important }` | 値 `y`、important | 値 `y ! important`、important でない |
  | `a{b: c !/**/important}` | 値 `c`、important | 値 `c !/**/important`、important でない |
  | `a{b: c !important /**/ }` | 値 `c`、important | 値 `c !important /**/`、important でない |
  | `a{b: c !imp\ortant}` | 値 `c`、important | 値 `c !imp\ortant`、important でない |

- **影響**：書き方によって `!important` が失われ、値の文字列に `!important` が残る。cascade の結果が変わる。
- **確信度**：高い。一つ目は railroad diagram と step 6 の両方が許す形である。

### F5 括弧の中の `!important;` を宣言の `!important` とみなし、値を切る

- **仕様**：step 6（3112–3117）は値の末尾の二つの token（top-level の component value）を見る。括弧の中の `!` と `important` は simple block や function の中身で、末尾の token にはならない。括弧の中の `;` も値を終えない（consume a simple block、3257–3266）。
- **コード**：値の phase の `!` の分岐（`Y:611-621`）は深さ（`deep`）を見ずに `lookup_important` を呼ぶ。後ろに `;` があれば真になり、`value_end` を `!` の位置にし、important を立てる。値はその後も括弧の終わりまで読まれるが、`value_end` はもう動かない（`Y:603`、`Y:640`）。Lexbor の `X:1560-1585` と同じ。
- **再現**（いずれも手動確認、Lexbor で確認）：
  - `a{b: (x !important; y); c:d}`
    - 仕様：`b` の値は `(x !important; y)`、important でない。`c: d` が続く。
    - 実装：`b` の値は `(x`、important。`c: d` は同じ。
  - `a{--v: {x !important; y}; c:d}`：実装は `--v` の値 `{x`、important。
  - `a{b: f(x !important; c:d}`：仕様は値 `f(x !important; c:d}`（function が入力の終わりまで続く）で important でない。実装は値 `f(x`、important。
- **影響**：カスタムプロパティの値に `{}` や `()` を使い、その中に `!important;` を書いた場合に限られる。
- **確信度**：高い。

### F6 宣言名と `:` の間にコメントがあると、宣言にならない

- **仕様**：コメントは token にならない（3849–3852、トークナイザ）。consume a declaration step 2–3（3091–3101）は ident の後の空白を捨てて `:` を見るので、`c/**/: d` も `c /**/ : d` も宣言である。
- **コード**：宣言か規則かを決める `lookup_colon`（`K:164-184`）は、先読みの token がないとき入力のバイト列を見て、空白以外の最初のバイトが `:` かどうかだけを調べる。`/` で偽になり、qualified rule として読まれる（`Y:421`、`Y:455-458`）。Lexbor の `XT:197` と同じ。
- **再現**（いずれも手動確認、Lexbor で確認）：
  - `a { c/**/: d }`：仕様の declarations は `c: d`。実装は空（`c/**/: d` は `BadStyle` として child に入る）。
  - `a{x:y; c /**/ : d; e:f}`：仕様は `x: y`、`c: d`、`e: f`。実装は `x: y` だけ（`BadStyle` が宣言の列を分け、`e: f` も child に回る）。
- **確信度**：高い。

### F7 値の正規化で、`\` の delim の後の改行が空白になり、値の意味が変わる

- **仕様**：
  - Serialization（3858–3862）：直列化は解析と round-trip しなければならない（空白の連続を一つにすることだけは許す）。
  - 同（3871–3876）：「A <delim-token> containing U+005C REVERSE SOLIDUS (\) must be serialized as U+005C REVERSE SOLIDUS followed by a newline.」トークナイザは、`\` の後が改行のときだけ `\` を delim として出す（valid escape の判定、1497–1504）。
  - 文字列の終わりの `\` は valid escape で、U+FFFD になる（1470–1473、1501–1504）。
- **コード**：`normalize_value`（`L:136-169`）は空白 token を、前に token があれば空白一つにし（`L:154`、`L:160-163`）、末尾では捨てる。`\` の delim の後の改行も空白一つになるか、消える。`docs/rewrite_parser.ja.md` は、この正規化が「CSS の値としての意味は変えない」としている。prelude と `:bad_style` の切り出し（`L:233-257`）は末尾の `\` を残す対策を持つが、値の正規化にはない。
- **再現**（手動確認）：
  - `a{b: c\` 改行 `d}`：トークンは ident `c`、delim `\`、空白、ident `d`。実装の値は `c\ d` で、これを解析し直すと `\` と空白がエスケープになり、ident 一つ（値 `c d`）になる（手動確認：同じ文字列をセレクタ `#c\ d` として使うと `id="c d"` の要素に当たる）。
  - `a{e: x \` 改行 `}`：実装の値は `x \`。末尾の `\` は U+FFFD のエスケープとして読まれる。
- **Lexbor との関係**：移植版の読み取り層の正規化で生じる。Lexbor は未知のプロパティの値の前後を削るだけだった（`docs/rewrite_parser.ja.md`）。
- **確信度**：中程度から高い。仕様の round-trip の要件は直列化の一般論で、`parse_stylesheet` の値はその直列化そのものではない。ただし設計の文書が「意味を変えない」と述べており、この入力ではそれが成り立たない。

### U1 at-rule の block の中の宣言を出力しない

- **仕様**：block at-rule は宣言のリストと child rules を持つ（2012–2016）。consume an at-rule の `<{-token>`（2734–2744）は block's contents を child rules にし、宣言のリストを含みうる。`@font-face`（`<declaration-list>`）や `@page`（`<declaration-rule-list>`）の中身は宣言である（3632–3643）。
- **コード**：パーサは block のリストに `DeclarationList` を入れる（`Y:431-452`）が、読み取り層は `Style`、`BadStyle`、`At` 以外の項目を捨てる（`L:320-321`「A block's declarations and anything else: not surfaced.」）。出力の形（`G:117-126`）にも場所がない。
- **再現**（手動確認）：`@font-face { font-family: x }` は `rules: []`。`@page :first { margin: 1in; @top-left { content: "x" } }` は `margin` も `content` も出ない。
- **確信度**：高い（出力の形の問題）。二つの差分文書には記録がない。

### U2 style rule の child rules（入れ子の規則と nested declarations rule）を出力しない

- **仕様**：qualified rule は child rules を持つ（2026）。block の最初の宣言のリストより後の規則と宣言のリストは child rules になる（2870–2873）。
- **コード**：パーサは `child` を作る（`Y:919-921`）が、読み取り層は `Style` と `BadStyle` の `child` を読まない（`L:266-296`）。
- **再現**（手動確認）：`a { color: red; & b { c: d } e: f }` は declarations `color: red` だけを返し、`& b { c: d }` と、nested declarations rule の `e: f` は出ない。
- **確信度**：高い（出力の形の問題）。二つの差分文書には記録がない。入れ子の規則の prelude を relative selector として読む処理（CSS Nesting）も持たない。

---

## 未カバーの step

状態が一致または意図した差で、`R:`、`IC:`、`NS:`、`T:` のどれも当たらないものを挙げる。

テストがまったくないもの（手動確認かコードの経路の追跡で判断した）：

1. **consume an at-rule の `<}-token>`**（2724–2732）。nested のとき規則を閉じること、nested でないとき prelude に足すこと。手動確認：`@media x { @foo bar } a{b:c}`、`@foo } x; a{b:c}`。
2. **block の中で invalid rule error のときに宣言の列を分けること**（2974–2979）。手動確認：`a { c: d; 1x {} e: f }`。
3. **consume a qualified rule の、カスタムプロパティのような prelude で nested のとき**（2794–2797）。手動確認：`a { --foo:hover { x: y } ; c: d }`、`@media x { --foo:hover { x: y } ; c{d:e} }`。
4. **consume a declaration step 3 の `:` でない分岐**（3098–3101、S4）。到達しない。
5. **`unicode-range` の値**（3137–3142）。Lexbor との比較の無作為な入力にだけ値 `U+00-ff` がある。
6. **入力の前処理の CR と FF**（452–456）。手動確認：`a{b:c\r\nd}`、`a{b:c\fd}`。

Lexbor との比較（`D:`）だけが当たるもの（答えは仕様と照らしていない。仕様と Lexbor が同じ答えであることは手動確認した）：

1. トップレベルの CDO、CDC（2659–2662）：D: "<!-- a { b: c } -->"。
2. 型付きの at-rule の `_undef`（C2）：D: "@media;"、"@namespace x {a{}}" など。
3. at-rule の prelude の括弧の対応（2750–2753）：D: "@foo [ { ] } a { b: c }"。
4. qualified rule の nested でない `}`（2783–2785）：D: "} a { c: d }"。R: "never raises a syntax error on broken input" は例外が出ないことだけを確かめる。
5. block の中の at-rule と、規則が返ったときの宣言の列の分け方（2944–2953、2981–2986）：D: "a { c: d; @foo; e: f }"、"a { c: d; b {} e: f }"。
6. 宣言の値の括弧の対応、閉じない function、対にならない閉じ括弧（3106–3110、3247–3291）：D: "a { x: f(a}; c: d }"、"a { x: ); c: d }"、"a { b: url(}) }"。
7. `@charset`（3823–3826）：D: "@charset \"utf-8\";"。

逸脱の行（F1–F7）は、どれも仕様の答えを確かめるテストがない。F2、F4、F6 の入力の一部は `D:` にあるが、Lexbor と同じ答えであることを確かめるだけで、逸脱を検出しない。
