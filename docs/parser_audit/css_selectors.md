# CSS セレクタパーサ監査：Selectors Level 4 の文法、An+B、specificity

- 対象の仕様：
  - Selectors Level 4（w3c/csswg-drafts の `selectors-4/Overview.bs`、commit `7b4c69ecaf9289d1bd5455d6bea10eec0b4a4399`）
    - 3 章「Selector Syntax and Structure」（768–1404。構造、擬似クラス、擬似要素、大文字小文字、無効なセレクタ）
    - 4 章「Logical Combinations」（1405–1817。`:is()`、`:not()`、`:where()`、`:has()`）
    - 5–6 章の型、全称、名前空間、属性、クラス、ID（1818–2483）
    - 7 章の `:dir()` と `:lang()`（2486–2766）、14 章の子の位置による擬似クラス（3724–4267）
    - 16 章「Calculating a selector's specificity」（4420–4521）、17 章「Grammar」と `<forgiving-selector-list>`（4523–4695）
  - CSS Syntax Level 3（同じ commit の `css-syntax-3/Overview.bs`）の 6 章「The An+B microsyntax」（3323–3560）
  - 行番号は、この commit のファイルの行番号である。以下 `S4:n` は selectors-4、`SY:n` は css-syntax-3 の行を指す。
- 対象の実装：Makiri の移植版セレクタパーサ（Lexbor の `css/selectors/` と `css/syntax/anb.c` の移植）、擬似クラスと擬似要素の名前の表、AST、`parse_stylesheet` が返すセレクタの文字列と specificity
- 既知の差（`docs/parser_differences.ja.md` の S2、S3）は読んだうえで、新しい差としては数えていない。S2（An+B の値が double で 2^63 になる整数の飽和）は 9 章の行で触れる。S3（パーサ自身のメモリ不足）は文法の外なので、表には入れていない。
- 照合の意味（どの要素に当たるか）は範囲外である。照合器（`lexbor/css_match/`）と XML への変換（`css/lower.rs`）は、パーサが受理した形のうち後で「could not be run」として拒否されるものを確かめるためにだけ読んだ。
- トークナイザ（`selectors/tokenizer.rs`）そのものの監査も範囲外である。トークンの種類と値は CSS Syntax のとおりと仮定して、パーサの分岐を見た。

## 読み方

### 略号

| 略号 | パス（リポジトリ相対） |
|---|---|
| `P` | `ext/makiri/rust/src/selectors/parser.rs` |
| `Q` | `ext/makiri/rust/src/selectors/pseudo.rs` |
| `A` | `ext/makiri/rust/src/selectors/ast.rs` |
| `X` | `ext/makiri/rust/src/lexbor/selector_text.rs` |
| `ST` | `ext/makiri/rust/src/lexbor/stylesheet.rs` |
| `CM` | `ext/makiri/rust/src/lexbor/css_match/compile.rs` |
| `CQ` | `ext/makiri/rust/src/lexbor/css_match/query.rs` |
| `LW` | `ext/makiri/rust/src/css/lower.rs` |
| `L` | `vendor/lexbor/source/lexbor/css/selectors/state.c` |
| `LP` | `vendor/lexbor/source/lexbor/css/selectors/pseudo_state.c` |
| `LA` | `vendor/lexbor/source/lexbor/css/syntax/anb.c` |

### テストの欄

- `diff:` は `ext/makiri/rust/src/lexbor/selector_port_tests.rs` の `SELECTORS`（310–484 行）の項目で、`the_parser_builds_lexbors_selectors` が移植版と Lexbor のパーサの出力（各リストの specificity、各単純セレクタの結合子、名前、名前空間、種類、引数）を比べる。比べる相手は Lexbor であり、仕様ではない。したがってこのテストが当たる行でも、Lexbor と同じ誤りは見つからない。監査中にこのテストを実行し、通ることを確かめた（`target/debug/deps/makiri-f23d0f92a760d72e`、3 件とも ok）。
- `rnd:` は同じファイルの `the_parser_builds_lexbors_selectors_on_random_inputs`（`random_selector`、505–675 行）で、乱数で作ったセレクタを Lexbor と比べる。どの分岐に当たったかは記録されないので、生成表に該当する部品があることだけを書く。
- `conf:` は `spec/conformance/css_selectors_spec.rb`、`css:` は `spec/css_spec.rb`、`lcss:` は `spec/lexbor_css_spec.rb`（`parse_stylesheet`）、`corpus:` は `spec/conformance/css_corpus.rb`（`css_diff.rb` で Nokogiri::HTML5 と比べる。`rake spec` では走らない）、`rs:` は `ext/makiri/rust/src/lexbor/tests.rs` である。数字は `it` か `fn` の行。
- 「手動確認」は、この監査で組み込み済みの Ruby 拡張に入力を与えて確かめたことを示す。リポジトリのテストではない。確かめ方は `Makiri::HTML(html).css(sel)`（構文の誤りは `Makiri::CSS::SyntaxError`、走らせられない構文は `Makiri::Error`「could not be run」）と、`Makiri::Lexbor::CSS.parse_stylesheet("sel {}")`（セレクタごとの文字列と specificity `[a, b, c]`、セレクタリストが無効なら `:bad_style`）である。スクリプトは監査用の一時ファイルで、リポジトリには含めない。

### 状態の欄

一致、意図した差、未実装、逸脱の四つを使う。

- **意図した差**は、`docs/parser_differences.ja.md` に載っているもの、または機能を支持しないと決めて例外を出すもの（Lexbor が「not supported」と記録して拒否する擬似クラスと擬似要素、照合器が「could not be run」とする `:current()` と `||`、HTML で名前空間接頭辞を無視する扱い）である。Selectors 4 は、支持しない擬似クラス、擬似要素、結合子を無効として扱うよう求めている（S4:1388-1391）。したがって、支持しないものを構文の誤りとして拒否するのは仕様に沿った振る舞いでもある。
- **逸脱**は、無効なセレクタを受理するもの、支持すると称する機能で有効なセレクタを拒否するもの、仕様と違う AST を作るもの、specificity を誤って計算するもの。
- **未実装**は、文法が受理するのにパーサに対応するコードがないもの。

### 実装の構造（前提）

- **Lexbor の制御の流れをそのまま移している。** 規則（成分のリスト、関数の引数）ごとに phase があり、括弧を数えて規則の終わりの token を `_END` に変える（`P:574-647`）。状態は token を消費し、`false` を返すと同じ token で再び呼ばれる。入れ子のリストは状態スタックに積み、ネイティブの再帰は使わない（`P:1-25`）。
- **AST は Lexbor の構造体を index の arena にしたもの**（`A:19-118`）。セレクタリストは `ListNode` の連結リスト、complex selector は `SelNode` の連結リストで、各 `SelNode` が自分の左の結合子を持つ（`A:34-41`）。compound の中の単純セレクタは `CLOSE` でつながる。
- **specificity はリストごとの 32 ビットの値**で、a を 18 ビット目、b を 9 ビット目、c を 0 ビット目から 9 ビットずつ持つ（`P:285-294`、Lexbor の `selector.h:130-166`）。`parse_stylesheet` はこの値をそのまま `[a, b, c]` に分けて返す（`ST:100-105`）。トップレベルでは数を足し、関数の引数の中では別の規則（9 章の F8）で計算する。
- **Lexbor との差は S2 と S3 だけ。** 監査で見つけた逸脱は、どれも Lexbor の C コードと同じ振る舞いである（コードを読んで確かめた。`diff:` に入っている入力は、テストが一致を確かめている）。

---

## 1. セレクタリストと complex selector（S4 4533-4551、1430-1478、1364-1394）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `<selector-list>` = `<complex-selector>#`：コンマで区切ったリスト（4533-4535、1433-1438） | `P:1163-1168`（`complex_list`）、`P:2014-2055`（`list_end`：コンマで次の complex selector に移る） | 一致 | diff: "a, b"。conf:88（"p, span"）。lcss:25 | コンマの前後の空白は `list_end` の `P:2020-2028` と `compound_handler` の `P:1346-1353` が読み飛ばす。 |
| 空の項目は無効：`p,`、`,p`、`p,,q`（1385） | `P:1354-1363`（`compound_handler` の `_END` で、リストが空なら誤り） | 一致 | diff: "a,"、",a"、"a,,b" | |
| 空のセレクタ（compound selector を一つも含まない）は無効（1385） | `P:1354-1363`、`P:365-371`（リストが一つもなければ `ErrorUnexpectedData`） | 一致 | diff: ""、" " | |
| 無効なセレクタを一つでも含むリストは全体が無効（1384、1454-1478） | `P:2059-2101`（`function_error` の forgiving でない分岐：リストを捨てて終わりまで読み飛ばす）、`P:2004-2011` | 一致 | 手動確認：`p, :foo` は `SyntaxError`、`parse_stylesheet` は `:bad_style`。lcss:179（`p:unknown-pseudo` が `:bad_style`） | |
| `<complex-selector>` = unit [ `<combinator>`? unit ]*（4546） | `P:1226-1279`（`complex_end`）、`P:1218-1223`、`P:1282-1288` | 一致 | diff: "a b"、"a  >  b"、"a:is(.b, .c) > d:not(.e)" | |
| `<combinator>` = `>` \| `+` \| `~`（4559、849-858） | `P:1240-1267` | 一致 | diff: "a+b"、"a ~ b"。conf:50 | AST の `CHILD`、`SIBLING`、`FOLLOWING`（`A:34-41`）。 |
| 結合子が二つ続く、先頭にある、末尾にあるものは無効 | `P:1257-1262`、`P:1268-1272`（結合子の後に結合子）、`P:1359`（`_END` で結合子が残る）、`P:1335`（先頭の `>`） | 一致 | diff: "a > > b"、">a"、"a >"。css:231（`>>>bad`） | |
| 空白の descendant combinator。結合子を書かないときは空白が必須（4605-4608、853） | `P:1229-1237`（空白で `DESCENDANT` にする）、`P:1379-1405`（compound の途中の空白や Ident で compound を終える） | 一致 | diff: "a/**/b"（無効）、"a /* c */ b"、"a\tb"、"a\nb"、"a\rb"、"a\u{c}b" | コメントは空白ではないので `p/**/q` は無効、`p/**/.x` は `p.x` と同じ（手動確認）。U+3000 は空白ではない（1340-1344）。手動確認：`p　q` は `SyntaxError`（CSS Syntax の非 ASCII の ident code point に U+3000 は入らないので、Delim になる）。 |
| column combinator `\|\|` | `P:1245-1256`、`P:1192-1203`（`CELL` を作る）。照合は `CM:244-246`、`CQ:68` が拒否する | 意図した差 | css:248（"could not be run"）、css:276。lcss:259（`parse_stylesheet` が `col \|\| td` を返す）。diff: "a \|\| b" | 固定した Selectors 4 では column combinator は Selectors 5 に移り（5232）、`<combinator>` に入っていない（4559）。仕様の答えは「無効なセレクタ」である。Makiri は `css` では `Makiri::Error`（could not be run）を出すが、`parse_stylesheet` は有効な style 規則として specificity を付けて返す（手動確認：`div \|\| p {}` は `[0, 0, 2]`）。 |

## 2. compound selector と空白の規則（S4 4553-4557、798-812、4585-4617）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| type selector か universal selector は先頭に一つだけ（4553、802-804、1868-1870） | `P:1324-1366`（`compound_handler`：先頭でだけ Ident と `*` と `\|` を読む）、`P:1379-1405`（`compound_sub` は Ident と `*` を読まず、compound を終える） | 一致 | diff: "a[b][c]"、"[a=b]x"（無効） | 手動確認：`div*`、`*div`、`**`、`.x*`、`div.x*` は `SyntaxError`。`*.x` は受理。 |
| `<subclass-selector>*`：ID、クラス、属性、擬似クラスを任意の順で何度でも（4553、4566-4567） | `P:1379-1405` | 一致 | diff: ".a.b#c"、"a[b][c]"。conf:36 | |
| compound の成分の間に空白を置けない（4590-4595、810-812） | `P:1398`（空白で `states_set_back`、`complex_end` が descendant にする）、`P:1480-1486`（`.` の後は `token()` で空白を読まない） | 一致 | diff: ".1"（クラス名が数字） | 手動確認：`div .x`、`div #a`、`div [a]`、`div :hover` は descendant。`div. x` は `SyntaxError`。 |
| `<pseudo-compound-selector>` = 擬似要素 擬似クラス*（4547、4554、814-847） | `P:1302-1316`、`P:1408-1442`（`compound_pseudo`） | 意図した差 | conf:132。diff: "::before"、"a::before" | 擬似要素はすべて拒否する（7 章）ので、`compound_pseudo` に入ることはない。仮に擬似要素を受理したとしても、`compound_pseudo` は擬似要素の後のあらゆる擬似クラスと擬似要素を受理する（S4 1193-1201、1239-1245 は論理結合と user action の擬似クラスだけを許す）。 |

## 3. type selector、universal selector、名前空間（S4 4561-4564、1821-1958、1354-1362）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `<wq-name>` = `<ns-prefix>`? `<ident-token>`：接頭辞なしの type selector | `P:1524-1536`（`element`）、`P:1539-1548`（後ろに `\|` がなければ終わり） | 一致 | diff: "div"、"DIV"。conf:36、conf:44 | 名前は書かれたとおりに残す（AST の `name`）。大文字小文字の扱いは照合の側。 |
| `ident \|`：接頭辞つき（`ns\|E`、`ns\|*`） | `P:1551-1579`（`selector_ns_ident`：前の名前を `ns` に移す） | 一致 | diff: "a\|b"、"ns\|*" | specificity は F9。 |
| `* \|`：任意の名前空間（`*\|E`、`*\|*`） | `P:1501-1513`（`element_ns` の `*`）→ `P:1539-1548` | 一致 | diff: "*\|a"、"*\|*" | `*` だけのときは universal selector（`sel_type::ANY`、名前 `*`）。 |
| `\|` だけ：名前空間なし（`\|E`、`\|*`） | `P:1514-1520`（`ns` を空文字列にする） | 一致 | diff: "\|a"、"\|*" | |
| `<wq-name>` と `<ns-prefix>` の中に空白を置けない（4596、4600） | `P:1539-1548`、`P:1551-1579`（どちらも `token()` で、空白を読み飛ばさない） | 一致 | なし（diff の項目はどれも空白を挟まない） | 手動確認：`*\| div`、`div\| a` は `SyntaxError`。`* \|div` と `div \|a` は descendant の後の `\|div`、`\|a` として受理（仕様どおり）。 |
| 宣言されていない接頭辞は無効（1381、1956-1958。宣言の仕組みがなければ接頭辞は一つも宣言されていない：1360-1361） | パーサは接頭辞を確かめない（`P:1551-1579`）。HTML の照合器は接頭辞を無視し、XML の変換は XPath の名前空間の登録で解決する（`LW:62-75`） | 意図した差 | css:176（`:is(svg\|circle)` など、名前空間を無視する）、`spec/query_args_spec.rb:46` | 手動確認：HTML では `foo\|div` が `div` に当たる。XML では `foo\|a` が `Makiri::Error`（unknown namespace prefix 'foo'）。HTML の名前空間の接頭辞を無視するのは spec で固定された選択で、`css` は名前空間の Hash を受け取る。 |

## 4. ID セレクタとクラスセレクタ（S4 4569-4571、4609-4610、2345-2483）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `<id-selector>` = `<hash-token>` | `P:1454-1477`（`selector_hash`） | 一致 | diff: ".a.b#c"。conf:36 | |
| hash token の値は識別子でなければならない（4609-4610、2441-2443） | なし（`selector_hash` はどの hash token も受理する。トークンは hash の type flag を持たない：`selectors/token.rs:45-72`） | **逸脱（F5）** | diff: "#1"（Lexbor と同じく受理） | 後述の F5。 |
| `<class-selector>` = `.` `<ident-token>`、間に空白なし（4571、4596-4597） | `P:1329-1333`、`P:1480-1498`（`.` の次を `token()` で読み、Ident でなければ誤り） | 一致 | diff: ".1"、".\\-"、".é" | 手動確認：`. x` は `SyntaxError`。 |

## 5. 属性セレクタ（S4 4573-4576、4601、2008-2283）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `[` `<wq-name>` `]`：属性の有無 | `P:1582-1595`、`P:1645-1657`、`P:1680`、`P:1754-1758` | 一致 | diff: "[a]"。conf:60 | |
| 括弧の中の、wq-name と matcher と値の前後の空白は許す | `P:1621`、`P:1652-1655`、`P:1701`、`P:1714`、`P:1725`（`token_wo_ws`） | 一致 | diff: "[ a ]"、"[a= b ]" | 手動確認：`[a = b]`、`[ \|a]`。 |
| `ident \|` で始まる名前（`[ns\|a]`） | `P:1657-1672` | 一致 | diff: "[ns\|a=b]" | |
| `\|` だけで始まる名前（`[\|a]`、名前空間なし） | `P:1623-1643` | 一致 | lcss:267。diff: "[\|a]" | AST では名前空間を `*` として持つ（Lexbor と同じ）。`*` は本来「任意の名前空間」の記号なので AST の表現としては紛らわしいが、読む側（`X:183-191`、`LW:117-128`）はどちらも「名前空間なし」と読み、`[*\|a]` はそもそも受理されない（次の行）ので、意味が混ざることはない。 |
| `*\|` で始まる名前（`[*\|a]`、任意の名前空間。2252-2254、2266） | なし（`P:1623-1625`：最初の Delim が `\|` でなければ失敗） | **未実装（U1）** | lcss:274（`[*\|a]` が `:bad_style` であることを固定している）。diff: "[*\|a]" | 後述の U1。 |
| wq-name の中に空白を置けない（4600） | `P:1650`、`P:1659`（`\|` の前後は `token()`） | 一致 | なし | 手動確認：`[ns \|a]`、`[ns\| a]` は `SyntaxError`（前者は `\|` を `\|=` の始まりと読んで失敗する）。 |
| `<attr-matcher>` = [ `~` \| `\|` \| `^` \| `$` \| `*` ]? `=` | `P:1679-1717` | 一致 | diff: "[a=b]"、"[a\|=b]"、"[a~=b]"、"[a^=b]"、"[a$=b]"、"[a*=b]"。conf:60 | `[a\|=b]` は名前の後の `\|` を読んで、次が Ident でなければ `\|=` とする（`P:1658-1662`）。diff: "[a\|b\|=c]"（無効）。 |
| matcher の二つの文字の間に空白を置けない（4601） | `P:1695`（`~` などの後は `token()`） | 一致 | なし | 手動確認：`[a~ =b]` は `SyntaxError`。 |
| ほかの演算子（`!=`、`==`）は無効 | `P:1689`、`P:1710-1712` | 一致 | diff: "[a==b]"、"[=b]" | 手動確認：`[a!=b]`。 |
| 値は `<string-token>` か `<ident-token>`（4574、2058） | `P:1718-1721` | 一致 | diff: "[a=1]"、"[a="（無効） | |
| `<attr-modifier>` = `i` \| `s`、ASCII で大文字小文字を区別しない（4576、2183-2205、1306-1307） | `P:1729-1748` | 一致 | diff: "[a=\"b\" i]"、"[a=b s]"、"[a=b x]"、"[a=b ii]"（無効）。lcss:196 の表（`[a="x" i]`） | 手動確認：`[a=b I]`、`[a=b S]`、`[a="b"i]`（間に空白なし）は受理。`[a=b i i]` は `SyntaxError`。 |
| modifier は値の後にだけ置ける | `P:1652-1656`、`P:1707`（値のない `[a i]` は Op で Ident を読んで失敗） | 一致 | なし | 手動確認：`[a i]` は `SyntaxError`。 |
| 閉じていない `[`（入力の終わりで閉じる。CSS Syntax の simple block の規則） | `P:1706`、`P:1728`、`P:1744`（`_END` で終わる） | 一致 | diff: "[a"、"[a=b"、"[a=b i"。conf:117（`li[`：名前がないので無効） | `parse_stylesheet` では `{` が `[` のブロックに入るので `:bad_style` になる。これも CSS Syntax のとおり。 |
| 名前のない `[]`、数字の `[1]` | `P:1674` | 一致 | なし | 手動確認：どちらも `SyntaxError`。 |

## 6. 擬似クラスの構文と、知っている擬似クラス（S4 4578-4579、1055-1095、1302-1307、1364-1394）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `:` `<ident-token>` | `P:1293-1301`（`colon`）、`P:1783-1820`（`pseudo_class`） | 一致 | diff: ":first-child"、":root"。conf:75 | |
| `:` `<function-token>` `<any-value>` `)` | `P:1317-1320`、`P:1823-1840`、`P:1844-1864` | 一致 | diff: ":not(a)"、":nth-child(2)" | 引数の読み方は関数ごと（8、9、10 章）。 |
| `:` と名前の間に空白を置けない（4598、1080-1083） | `P:1295-1301`（`:` の次を `token()` で読み、Ident、Colon、Function 以外なら誤り） | 一致 | なし | 手動確認：`: root` は `SyntaxError`。 |
| 名前は ASCII で大文字小文字を区別しない（1079、1302-1305） | `Q:363-368`（`eq_ignore_ascii_case`） | 一致 | diff: ":FIRST-CHILD"、":nth-child(EVEN)"。lcss:255。rnd（"FIRST-OF-TYPE"、"NOT"、"Is"） | |
| 知らない擬似クラスと関数は無効（1388-1391） | `P:1788-1790`、`P:1828-1830` | 一致 | diff: ":foo"、":contains('x')"。conf:126。lcss:179 | |
| 引数なしの名前を関数として書く（`:hover()`）、関数の名前を引数なしで書く（`:nth-child`、`:not`） | `Q:129-172`（`CLASSES`）と `Q:209-350`（`CLASS_FUNCTIONS`）は別の表なので、どちらも見つからない | 一致 | なし | 手動確認：`:hover()`、`:first-child()`、`:nth-child` は `SyntaxError`。 |
| 支持する擬似クラス：`:active`、`:any-link`、`:checked`、`:disabled`、`:empty`、`:enabled`、`:first-child`、`:first-of-type`、`:focus`、`:hover`、`:last-child`、`:last-of-type`、`:link`、`:only-child`、`:only-of-type`、`:optional`、`:placeholder-shown`、`:read-only`、`:read-write`、`:required`、`:root` | `Q:129-172`、`P:1815-1819` | 一致 | rnd（PSEUDO の表）。conf:75。diff: ":hover" | `:blank` も受理するが、固定した版では Selectors 5 に移っている（5266）。Level 5 の拡張として受理するもので、Level 4 の文法とはぶつからない。specificity は F7。 |
| Lexbor が「not supported」として拒否する擬似クラス：`:default`、`:focus-visible`、`:focus-within`、`:fullscreen`、`:in-range`、`:indeterminate`、`:invalid`、`:out-of-range`、`:scope`、`:target`、`:user-invalid`、`:valid`、`:visited`（Level 4）と `:current`、`:future`、`:past`、`:local-link`、`:target-within`、`:warning`（Level 4 にない） | `P:1791-1814`（Lexbor の `L:1497-1522`） | 意図した差 | diff: ":visited"、":target"、":scope"。css:109（`:scope`）。lcss:276（`:focus-within` が `:bad_style`） | 支持しない擬似クラスを無効とするのは 1388-1391 のとおり。 |
| 表にない Level 4 の擬似クラス：`:defined`、`:open`、`:popover-open`、`:modal`、`:picture-in-picture`、`:playing` などのリソースの状態、`:autofill`、`:unchecked`、`:user-valid` | `P:1788-1790`（知らない名前） | 意図した差 | なし | 手動確認：`:defined`、`:open`、`:modal`、`:popover-open`、`:playing`、`:autofill`、`:state(x)`、`:host` は `SyntaxError`。 |
| legacy selector alias の `:matches()`（1552-1556、"may"） | なし | 一致 | なし | 任意の機能。手動確認：`:matches(p)` は `SyntaxError`。 |

## 7. 擬似要素（S4 4581-4582、4612-4640、1098-1291）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `::` `<ident-token>`：知っている擬似要素（`::after`、`::backdrop`、`::before`、`::first-letter`、`::first-line`、`::grammar-error`、`::inactive-selection`、`::marker`、`::placeholder`、`::selection`、`::spelling-error`、`::target-text`）はすべて拒否、知らない名前も拒否 | `P:1302-1311`、`P:1867-1897`（`L:1633-1644` と同じ表） | 意図した差 | conf:132。diff: "::before"、"::foo"、"a::before"。lcss:168。corpus（"p::before"） | 要素を返す API なので擬似要素は選べない。1388-1391 のとおり。 |
| `::` `<function-token>`（`::slotted()`、`::part()`、`::highlight()` など） | `P:1312-1315`、`P:1900-1909`（表は `#undef` だけ：`Q:352-360`） | 意図した差 | diff: "::x()" | 手動確認：`::slotted(p)`、`::part(x)` は `SyntaxError`。 |
| `::` の二つのコロンの間、コロンと名前の間に空白を置けない（1151、4598） | `P:1302-1316`（`token()`） | 一致 | なし | 手動確認：`: :before`、`:: before` は `SyntaxError`。擬似要素を拒否するので結果は同じだが、空白の判定が先に効く。 |
| legacy の一つのコロン（`:before`、`:after`、`:first-line`、`:first-letter`、4582、1153-1160）。擬似クラスとして読んではならない（4612-4616） | `P:1788-1790`（擬似クラスの表にないので拒否） | 意図した差 | なし | 手動確認：4 つとも `SyntaxError`。擬似クラスとして受理することはないので、4612-4616 の禁止は守られる。 |
| 擬似要素は compound の最後に置き、後ろには論理結合と user action の擬似クラスだけ（1193-1201、4624-4633）。擬似要素の後の擬似要素（1239-1245）、後ろの結合子（1250-1254） | `P:1408-1442`（`compound_pseudo`） | 意図した差 | なし | 擬似要素を受理しないので到達しない。2 章の備考を参照。 |
| `:is()`、`:not()`、`:where()`、`:has()` の中の擬似要素は無効（1528-1529、1587-1588、1776-1780） | 擬似要素の拒否（上の行）と、`:is()` の forgiving な読み飛ばし（`P:2077-2089`） | 一致 | なし | 手動確認：`:is(p, ::before)` は `:is(p)`、`:not(::before)` と `:has(::before)` は `SyntaxError`。ただし `div:has(p, ::before)` は F2 のため受理される。 |

## 8. 論理結合と :has()（S4 1405-1817、4643-4695）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `:is()` は `<forgiving-selector-list>`：読めない項目と無効な項目を捨てる（1507-1508、4654-4681） | `Q:250-258`（`forgiving: true`）、`P:2059-2089`（`function_error`：リストを外して次のコンマまで読み飛ばす）、`P:1988-2001`（`forgiving_cb`） | 一致 | diff: ":is(a, $, b)"、":is(a,, b)"、":is(a, (b), c)"。conf:88 | 手動確認：`:is(p, :foo)` は `:is(p)`、`:is(p !, q)` は `:is(q)`。 |
| 空になった `:is()` は有効で、何にも当たらない（1510-1511、4667、4681） | `P:1959-1974`（`function_empty`：`Q` の `empty` は全関数で false なので、セレクタを外して親の規則を失敗にする） | **逸脱（F1）** | diff: ":is()"、":is($)"、":where($)"（どれも Lexbor と同じく無効） | 後述の F1。 |
| `:where()`：構文は `:is()` と同じ、specificity は 0（1628-1633、4453） | `Q:340-348`、`P:1996-1998` | 一致（空の場合は F1） | diff: ":where(a, .b)"、":where(#x)"。lcss:37 | |
| `:is()` と `:where()` の項目は `<complex-real-selector>`：先頭の結合子は許さない（4674） | `P:2107-2112`（`ComplexList` で読む） | 一致 | なし | 手動確認：`:is(> p)`、`:where(~ p)` は `SyntaxError`（forgiving で捨てた結果、空になるので F1 の経路でもある）。 |
| `:not()` は `<complex-real-selector-list>`、forgiving ではない（1575-1576） | `Q:277-285`（`forgiving: false`）、`P:2091-2100` | 一致 | diff: ":not(a, b)"、":not(a b > c)"、":not(a,)"、":not(,a)"、":not()"、":not(:not(a))" | 手動確認：`:not(p, :foo)` は `SyntaxError`。 |
| 論理結合の入れ子 | `P:1844-1864`、`P:1924-1955`（`restore_parent`） | 一致 | diff: ":is(:is(:is(a)))"、":where(a, :is(b, c))"。css:220（深い `:is()`） | |
| `:has()` は `<relative-selector-list>`、forgiving ではない（1768-1769、4542、5257-5259） | `Q:241-249`（`forgiving: true`、`EndCb::FunctionForgivingRelative`） | **逸脱（F2）** | なし（diff: ":has()"、":has(> > a)" は Lexbor と一致するが、forgiving で捨てる入力はない） | 後述の F2。 |
| 相対セレクタ：先頭の `>`、`+`、`~`、なければ descendant（4550、1042-1052） | `P:1171-1176`、`P:1180-1215`（`relative_handler`） | 一致 | diff: ":has(> a)"、":has(+ a, ~ b)"、":has(a b)"。conf:88。rs:802 | |
| `:has()` の空の引数、結合子だけの引数、結合子が二つ続くものは無効 | `P:1359`、`P:1959-1974`、`P:1257-1262` | 一致 | diff: ":has()"、":has(> > a)" | 手動確認：`:has(>)` は `SyntaxError`。 |
| `:has()` の中の `:has()` は無効（1774-1775、5284-5285） | なし | **逸脱（F3）** | diff: ":has(:has(a))"（Lexbor と同じく受理）。rs:1491（入れ子の照合のテスト） | 後述の F3。 |
| `:has()` の中の `\|\|` | `P:1192-1203` | 意図した差 | css:276（"body:has(col \|\| td)"） | 1 章の column combinator と同じ。 |
| `:current()` | `Q:223-231`（`:is` と同じ読み方）。照合は `CM:96-100` が拒否 | 意図した差 | css:263、css:276。lcss:196 の表（`:current(.a\:b)`） | 固定した版では時間の擬似クラスは Selectors 5 に移った（5270）。Makiri は could not be run とする。 |

## 9. :nth-*() と An+B（S4 3814-4267、SY 3323-3560）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `odd`、`even`（ASCII で大文字小文字を区別しない、SY 3461、3507-3513） | `P:2274-2285` | 一致 | diff: ":nth-child(odd)"、":nth-child(EVEN)"。conf:75 | |
| `<integer>`：type flag が integer の number（符号はあってよい、SY 3462、3493） | `P:2216-2224`（`is_float` なら誤り） | 一致 | diff: ":nth-child(2)"、":nth-child(1.5)"、":nth-child(1e3)"。rnd（"+5"、"-5"） | |
| `<n-dimension>`（SY 3464、3488） | `P:2208-2214`、`P:2253-2259`、`P:2306-2333` | 一致 | diff: ":nth-child(2n)"、":nth-child(2.0n)"（無効） | |
| `'+'? n`、`-n`。`+` と `n` の間に空白を置けない（SY 3465-3466、3498-3502） | `P:2225-2238`（`+` の次を `token()` で読む）、`P:2262-2273` | 一致 | diff: ":nth-child(n)"、":nth-child(+n)"、":nth-child(-n)"、":nth-child(+ n)"（無効） | 手動確認：`- n`、`+ 5` は `SyntaxError`。 |
| `<ndashdigit-dimension>`、`'+'? <ndashdigit-ident>`、`<dashndashdigit-ident>`（SY 3468-3470、3526-3538） | `P:2334-2347`（`data_to_long`、`P:309-336`） | 一致（値の範囲は F6） | diff: ":nth-child(10n-3)"、":nth-child(n-1)"、":nth-child(n-a)"（無効）。rnd（"-2n-2"） | 手動確認：`n-1a` は `SyntaxError`。 |
| `<n-dimension> <signed-integer>` など（SY 3472-3474、3540-3545） | `P:2309-2316`、`P:2353-2359` | 一致 | diff: ":nth-child(n -1)"、":nth-child(n+-3)"（`+-3` は無効） | |
| `<ndash-dimension> <signless-integer>`、`n- 5`、`-n- 5`（SY 3476-3478、3547-3552） | `P:2335-2351`、`P:2353-2362` | 一致 | diff: ":nth-child(n- 1)"。rnd（"n- 2"） | 手動確認：`2n- -1`、`2n - +1` は `SyntaxError`。 |
| `<n-dimension> ['+' \| '-'] <signless-integer>`（SY 3480-3482） | `P:2317-2328`、`P:2353-2362` | 一致 | diff: ":nth-child(n - 1)"、":nth-child(2n - 1)"、":nth-child(3n+ 2)" | `3n + -6` は無効（SY 3420）：`P:2356`。手動確認。 |
| ほかの token の間の空白は許して無視する（SY 3502、S4 1084-1087） | `P:2239-2245`、`token_wo_ws` | 一致 | diff: ":nth-child( 2 )" | |
| 値が long の範囲を超える整数 | `P:298-306`（`double_to_long`：飽和）、`P:2340-2343`（`<ndashdigit-*>` の数字は範囲を超えると誤り） | **逸脱（F6）**（2^63 の飽和は S2、意図した差） | diff: ":nth-child(99999999999999999999)"、":nth-child(n-99999999999999999999)"、`DIVERGENT`（S2） | 後述の F6。 |
| An+B の後には `)` か（`:nth-child`、`:nth-last-child` のときだけ）`of S` しか置けない（S4 3915、4135） | `P:2135-2144`、`P:2181`（`success` にした後の余分な token は `failed` になるが、`function_end` が失敗を見ない：`P:1977-1985`） | **逸脱（F4）** | diff: ":nth-of-type(2 of .a)"（Lexbor と同じく `:nth-of-type(2)` として受理） | 後述の F4。 |
| `:nth-child(An+B of S)`、`:nth-last-child(An+B of S)`：`of` は ASCII で大文字小文字を区別しない。S は `<complex-real-selector-list>` で forgiving ではない（3915-3922、5257-5259） | `P:2147-2182`（`of` の判定は `P:2163`）、`P:2185-2200`。S は `ComplexList` を `EndCb::PseudoOfEnd` で読む | 一致 | diff: ":nth-child(2 of .a)"、":nth-child(2n of .a, .b)"、":nth-child(of .a)"、":nth-child(2 of)"、":nth-child(2 of :is(a, b))"。rs:765、rs:2387 | 手動確認：`odd OF p` は受理、`odd of p, :foo`、`odd of p,`、`odd of > p` は `SyntaxError`。specificity は F8。 |
| `:nth-of-type()`、`:nth-last-of-type()`：An+B だけ（4135、4166） | `Q:322-339`（`St::PseudoAnbBegin`）、`P:2135-2144` | 一致（余分な token は F4、specificity は F7） | diff: ":nth-of-type(2)"、":nth-last-of-type(2n)"。conf:75 | |
| 空の `:nth-child()` は無効 | `P:2246`（`_END` は誤り） | 一致 | diff: ":nth-child()" | |
| `:first-child`、`:last-child`、`:only-child`、`:first-of-type`、`:last-of-type`、`:only-of-type` | `Q:129-172` | 一致 | conf:75。rs:755 | specificity は F7。 |
| `:nth-col()`、`:nth-last-col()` | `P:1832-1835`（拒否） | 一致 | diff: ":nth-col(1)" | 固定した版では Selectors 5 に移った（5268）。Level 4 にない構文として拒否するので一致とした。 |

## 10. :dir() と :lang()（S4 2489-2766）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `:dir(ident)`（2519-2540） | `P:1832-1835`（引数を読む前に拒否）、`Q:232-240` | 意図した差 | diff: ":dir(ltr)" | Lexbor の「not supported」（`L:1563-1572`）。 |
| `:lang()`：識別子か文字列のコンマ区切りのリスト（2639 以降） | `P:1832-1835`、`Q:259-267` | 意図した差 | diff: ":lang(en)" | 手動確認：`:lang("en")`、`:lang(en, fr)` も `SyntaxError`。 |

## 11. 大文字小文字、escape、空白（S4 1293-1351）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| Selectors の構文（擬似クラスと擬似要素の名前、`i`/`s`、`odd`/`even`、`of`、An+B の `n`）は ASCII で大文字小文字を区別しない（1302-1307） | `Q:363-368`、`P:1732-1734`、`P:2163`、`P:2255`、`P:2266-2285` | 一致 | diff: ":FIRST-CHILD"、":nth-child(EVEN)"。lcss:255 | |
| 文書の名前と値（type、クラス、ID、属性の名前と値）は書かれたとおりに AST に残す（1309-1318） | `P:1530-1533`、`P:1471-1474`、`P:1492-1495`、`P:1647`、`P:1722` | 一致 | diff: "DIV"。conf:138、conf:144（照合の側の quirks mode） | |
| escape は特別な意味を打ち消す（1346-1351） | トークナイザが値を decode する（パーサは token の値だけを見る） | 一致 | diff: "\\61"、"\\31 23"、"#\\31"、"a\\:b"。lcss:196 の表、lcss:237 | 手動確認：`d\iv`、`\64 iv` は `div`。`:nth-child(\6e)` も `n` として受理（SY は token の値で判定するので一致）。 |
| 空白は SPACE、TAB、LF、CR、FF だけ（1340-1344） | トークナイザ | 一致 | diff: "a\tb"、"a\nb"、"a\rb"、"a\u{c}b" | 1 章の U+3000 の備考を参照。 |

## 12. specificity（S4 4420-4521）

`parse_stylesheet` が返す `[a, b, c]` について調べた。照合器は specificity を使わない。

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| A：ID セレクタの数（4426） | `P:1455-1466` | 一致 | lcss:31（`#b > span` が `[1, 0, 1]`） | 手動確認：4501-4511 の例のうち `#x34y`、`#s12:not(FOO)`、`.foo :is(.bar, #baz)` は仕様どおり。 |
| B：クラスセレクタと属性セレクタの数（4427） | `P:1487`、`P:1755`、`P:1760` | 一致 | lcss:31（`div.a`） | 手動確認：`LI.red.level` は `[0, 2, 1]`、`H1 + *[REL=up]` は `[0, 1, 1]`。 |
| B：擬似クラスの数（4427） | `P:1783-1820`（`pseudo_class` は数えない）、`P:1836`（関数では `:nth-child` と `:nth-last-child` だけを数える） | **逸脱（F7）** | なし | 後述の F7。`:hover`、`:first-child`、`:root`、`:nth-of-type()`、`:nth-last-of-type()` を数えない。 |
| C：type selector の数（4428） | `P:1525`、`P:1556` | 一致（接頭辞なし、`*\|E`、`\|E`）／**逸脱（F9）**（`ns\|E`、`ns\|*`） | なし | 後述の F9。 |
| universal selector は数えない（4429） | `P:1501-1513`、`P:1567-1576` | 一致 | なし | 手動確認：`*`、`*\|*`、`\|*` は `[0, 0, 0]`。`ns\|*` は F9。 |
| `:is()`、`:not()`、`:has()` は引数の中で最も詳細な complex selector の specificity（4440-4445） | `P:1111-1129`（関数の中の `specificity_set_bc`）、`P:1455-1466`、`P:1140-1152`（`func_specificity`）、`P:1924-1955`（`restore_parent`） | **逸脱（F8）** | lcss:37（`:where(.w) :is(.p, #q)` が `[1, 0, 0]`：引数が一つの compound のときは正しい） | 後述の F8。 |
| `:nth-child()`、`:nth-last-child()` は擬似クラス一つ ＋ S の中で最も詳細な complex selector（4446-4451、3947-3956） | `P:1836`、F8 と同じ経路 | 一致（S が一つの compound のとき）／**逸脱（F8）**（S に結合子があるとき、または compound に同じ種類の単純セレクタが二つ以上あるとき） | lcss:196 の表（`:nth-child(2n+1 of .a\:b)`） | 手動確認：4471 の例 `:nth-child(even of li, .item)` は `[0, 2, 0]` で一致。 |
| `:where()` は 0（4453） | `P:1996-1998` | 一致 | lcss:37 | 手動確認：`.qux:where(em, #foo#bar#baz)` は `[0, 1, 0]`（4466 の例）。 |
| forgiving list の無効な項目の specificity は 0（4684-4687） | 無効な項目はリストから外す（`P:2077-2080`） | 一致 | なし | |
| 同じ単純セレクタの繰り返しも数える（4515-4516） | トップレベル：`P:1117` | 一致（トップレベル）／F8（関数の中） | なし | 手動確認：`:first-child:first-child` は F7 で `[0, 0, 0]`。`.a.a` は `[0, 2, 0]`。 |
| 上限を超える値は上限に丸め、溢れさせない（4493-4496） | `P:292-294`（`sp_set` は値を 9 ビットに切らない）、`P:1117`、`P:1458` | **逸脱（F10）** | なし | 後述の F10。 |
| セレクタリストでは各セレクタごとに計算する（4432-4435） | `ST:193-210` | 一致 | lcss:25、lcss:31 | |

---

## 集計

表の行は全部で 98 行である。

| 状態 | 行数 |
|---|---|
| 一致 | 72（このうち 3 行は、別の行の逸脱を備考で参照する：`:where()` の空の場合は F1、`<ndashdigit-*>` の値の範囲は F6、`:nth-of-type()` の余分な token と specificity は F4 と F7） |
| 意図した差 | 13 |
| 未実装 | 1（U1） |
| 逸脱 | 12（このうち 3 行は一部だけが逸脱：12 章の C の行は `ns\|E` と `ns\|*` だけが F9、`:nth-child()` の specificity の行と繰り返しの行は関数の中だけが F8） |

逸脱は 10 項目（F1–F10）、未実装は 1 項目（U1）である。どれも Lexbor の C コードと同じ振る舞いで、移植で生じたものではない。

意図した差の内訳は次のとおり。

- 支持しない擬似クラス、擬似要素、関数を構文の誤りとして拒否するもの：Lexbor の「not supported」の擬似クラス、表にない Level 4 の擬似クラス、擬似要素（`::` の名前と関数、legacy の一つのコロン、擬似要素の後の compound の規則、pseudo-compound selector）、`:dir()`、`:lang()`。
- could not be run とするもの：column combinator（トップレベルと `:has()` の中）、`:current()`。
- HTML で名前空間の接頭辞を宣言なしに受理し、無視するもの。

このほか S2（An+B の 2^63 の飽和）は、9 章の F6 の行の備考で扱った。

---

## 逸脱と未実装

### F1 空になった `:is()` と `:where()` を無効にする

- **仕様**：
  - `:is()` は `<forgiving-selector-list>` を取り、「If the argument, after parsing, is an empty list, the pseudo-class is valid but matches nothing.」（S4 1507-1511）。`:where()` も同じ構文（1628-1630）。
  - `<forgiving-selector-list>` は構文上 `<any-value>?` で（4667）、forgiving に読んだ結果は「This might be empty.」（4677-4681）。したがって `:is()`、`:is( )`、`:is(:foo)`、`:where(:foo, :bar)` は、どれも有効で何にも当たらない。
- **コード**：
  - `forgiving_cb`（`P:1988-2001`）は、引数のリストが残らなかったとき `function_empty`（`P:1959-1974`）を呼ぶ。`function_empty` は `function_can_empty`（`Q:402-404`）が false なら、そのセレクタを外し（`selector_remove`）、親の規則を失敗にする（`failed_set_parent`）。`Q` の表は、どの関数も `empty: false` で作っている（`Q:191-208` の `func`）。
  - Lexbor は `L:1806-1861`（`lxb_css_selectors_state_forgiving_cb`）と `L:1737-1784` で同じことをする。
- **最小の再現**：`Makiri::HTML("<p>").css(":is()")`
  - **仕様の結果**：有効なセレクタで、結果は空。`parse_stylesheet(":is() {}")` は style 規則になる。
  - **実装の結果**：`Makiri::CSS::SyntaxError`。`parse_stylesheet` は `:bad_style`（手動確認）。
  - 同じ結果になる入力：`:is( )`、`:where()`、`:is(:foo)`、`:where(:foo, :bar)`、`:is(::before)`、`:is(> p)`、`p:is()`、`p, :is()`（リスト全体が無効になる）、`:not(:is())`。対照として `:is(p, :foo)` は `:is(p)` として受理される（仕様どおり）。
- **Lexbor**：同じ（`diff: ":is()"`、`":is($)"`、`":where($)"` が一致する）。
- **確信度**：高い。仕様の文は明示的で、Lexbor のコードも `empty` を false にしている。

### F2 `:has()` の引数を forgiving に読む

- **仕様**：`:has()` は `<relative-selector-list>` を取る（S4 1768-1769、4542）。変更履歴に「Made `:has()` and the selector argument of `:nth-child()`/`:nth-last-child()` no longer forgiving.」（5257-5259）とあり、forgiving なのは `:is()` と `:where()` だけである（4659-4660、4689-4690）。したがって引数に無効な項目が一つでもあれば、`:has()` を含むセレクタ全体が無効（1382-1384）。
- **コード**：`Q:241-249` の `has` は `forgiving: true`、終わりのコールバックは `EndCb::FunctionForgivingRelative`（`P:924`、`P:1988-2001`）。`function_error`（`P:2059-2089`）は forgiving の分岐で、無効な項目を捨てて次のコンマから読み続ける。Lexbor は `vendor/lexbor/source/lexbor/css/selectors/pseudo_res.h:81-84`（`has` の行の最後の `true, true` が forgiving と comma）と `L:1796-1804`。
- **最小の再現**：`Makiri::HTML("<div><p></p></div>").css(":has(p, :foo)")`
  - **仕様の結果**：無効なセレクタ（`SyntaxError`）。
  - **実装の結果**：`:has(p)` として受理し、`html`、`body`、`div` を返す。`parse_stylesheet` は `:has(p) [0, 0, 1]`（手動確認）。
  - 同じ結果になる入力：`:has(p,)`（末尾のコンマ）、`:has(> p, :foo)`、`div:has(p, ::before)`。`:has(:foo)` は全項目を捨てて空になるので F1 の経路で `SyntaxError` になる（結果は仕様と同じ）。
- **Lexbor**：同じ（コードが同じ。`diff:` の `SELECTORS` には forgiving で捨てる `:has()` の入力がない。`rnd:` は `has` と STRAY の `$` などを組み合わせて作るので、当たっている可能性がある）。
- **確信度**：高い。Lexbor は `:has()` が forgiving だった時期の仕様に従っており、固定した版では変更が明記されている。

### F3 `:has()` の中の `:has()` を受理する

- **仕様**：「The `:has()` pseudo-class cannot be nested; `:has()` is not valid within `:has()`.」（S4 1774-1775、変更履歴 5284-5285）。論理結合の擬似クラスは位置の制約を引数に引き継ぐ（1418-1422、1203-1205）ので、`:has(:is(:has(p)))` も無効である。
- **コード**：`pseudo_class_function`（`P:1823-1840`）にも `start_function`（`P:1844-1864`）にも、外側の関数を確かめるコードがない。Lexbor の `L:1538-1608` も同じ。
- **最小の再現**：`Makiri::HTML("<div><p><span></span></p></div>").css(":has(:has(span))")`
  - **仕様の結果**：無効なセレクタ（`SyntaxError`）。
  - **実装の結果**：受理し、`html`、`body`、`div` を返す（手動確認）。
  - 同じ結果になる入力：`:has(:is(:has(p)))`、`:has(:not(:has(p)))`、`:has(p:has(q))`、`:has(:nth-child(1 of :has(p)))`。対照として `:not(:has(p))`、`:is(:has(p))` は仕様でも有効。
- **Lexbor**：同じ（`diff: ":has(:has(a))"` が一致する）。`rs:1491`（`has_nested_inside_has_is_heap_based_not_native_recursion`）は入れ子の `:has()` を照合できることを前提にしている。
- **確信度**：高い。

### F4 An+B の後ろの余分な token を捨てて受理する

- **仕様**：`:nth-child()` と `:nth-last-child()` の引数は `An+B [of S]?`（S4 3915）、`:nth-of-type()` と `:nth-last-of-type()` は `An+B` だけ（4135、4166）。`<a-n-plus-b>` の後に別の token が続く引数は文法に合わず、セレクタは無効（1377-1380）。また `2n+1of` の `+1of` は CSS Syntax では dimension token なので、`<a-n-plus-b>` のどの形にも当たらない（SY 3460-3482）。
- **コード**：
  - `pseudo_of_begin`（`P:2147-2182`）は An+B の次の token が `of` でなければ、その token を消費せずに `success` にする（`P:2181`）。`pseudo_anb_begin`（`P:2135-2144`）も同じ。
  - `success` の状態（`P:937-946`）は空白と `_END` 以外の token を見ると `failed` にし、`failed` の状態（`P:949-956`）は `_END` まで読み飛ばす。規則の `failed` は true になる。
  - しかし関数の終わりのコールバック `function_end`（`P:1977-1985`、`end_cb` の `P:919-934`）は `failed` を見ないので、セレクタはそのまま残り、パースは成功する。
  - `anb_state_ident_data`（`P:2306-2333`）は `n` の後の token が符号のない数や Delim 以外の token なら、その token を残して B = 0 で終わる。`2n+1of` では `+1of` が dimension なので B = 0 になり、`+1of p` が余分な token として捨てられる。
  - Lexbor は `LP:37-51`、`LP:329-405`、`L:1737-1784`、`LA:271-376` で同じ。
- **最小の再現**：`Makiri::HTML("<div><p>t</p><span>s</span><p>u</p><p>v</p></div>").css(":nth-child(2n+1of p)")`
  - **仕様の結果**：無効なセレクタ（`SyntaxError`）。
  - **実装の結果**：`:nth-child(even)` として受理し、`body`、`span`、`p`（4 番目の子）を返す。`parse_stylesheet` の文字列は `:nth-child(even)`（手動確認）。
  - 同じ結果になる入力：`:nth-child(3 n)`（`:nth-child(3)`）、`:nth-child(3n 1)`（`3n`）、`:nth-child(n 5)`（`n`）、`:nth-child(1 2)`（`1`）、`:nth-child(odd p)`（`odd`）、`:nth-child(odd,)`（`odd`）、`:nth-of-type(2 of p)`（`:nth-of-type(2)`）。いずれも SY 3433-3437 の無効な例（`3 n`）を含む。
- **Lexbor**：同じ（`diff: ":nth-of-type(2 of .a)"` が一致する）。
- **確信度**：高い。書いた条件が黙って捨てられ、別の要素に当たる（`2n+1of p` が偶数番目に当たる）ので、影響も大きい。

### F5 識別子でない hash token の ID セレクタを受理する

- **仕様**：「In `<id-selector>`, the `<hash-token>`'s value must be an identifier.」（S4 4609-4610）。ID セレクタの値は CSS の identifier でなければならない（2441-2443）。CSS Syntax では、`#` の後が ident sequence を始めない hash token（`#1a`、`#-1`）は type flag が "unrestricted" になる。
- **コード**：`selector_hash`（`P:1454-1477`）は hash token をすべて受理する。トークンの構造体（`selectors/token.rs:45-72`）には hash の type flag がなく、パーサは判定できない。Lexbor の `L:1052-1084` も同じ。
- **最小の再現**：`Makiri::HTML("<p id=1a>").css("#1a")`
  - **仕様の結果**：無効なセレクタ（`SyntaxError`）。
  - **実装の結果**：受理し、`p` を返す。`parse_stylesheet("#1a {}")` は `#\31 a [1, 0, 0]`（手動確認）。`#-1` も受理する。`#\31 a` と `#--` は仕様でも有効。
- **Lexbor**：同じ（`diff: "#1"` が一致する）。
- **確信度**：高い。ただし jQuery 系の利用者には `#1a` を受理するほうが都合がよく、ブラウザ（`querySelector`）は拒否する。

### F6 B が long の範囲を超える `<ndashdigit-*>` を拒否する

- **仕様**：`<ndashdigit-dimension>`、`<ndashdigit-ident>`、`<dashndashdigit-ident>` は「"n-*", where "*" is a series of one or more digits」で、数字の数に上限はない（SY 3490-3492、3526-3538）。値の範囲の扱いは仕様に書かれていない。
- **コード**：`anb_state_ident_data` は ident や unit の中の数字を `data_to_long`（`P:309-336`）で読み、long を超えると読んだ長さが足りなくなって誤りにする（`P:2340-2343`）。一方、別の token として書いた数（`n- 99999999999999999999`、`99999999999999999999`）は `double_to_long`（`P:298-306`）で飽和させて受理する。Lexbor の `LA:271-376` も同じ（`lexbor_conv_data_to_long`）。
- **最小の再現**：`Makiri::HTML("<p>").css(":nth-child(n-99999999999999999999)")`
  - **仕様の結果**：有効な An+B（A = 1、B は非常に大きな負の数）。実際には何にも当たらない、または B を下限に丸めて全部に当たる、のどちらか（範囲の扱いは仕様にない）。
  - **実装の結果**：`SyntaxError`。同じ値を `:nth-child(n- 99999999999999999999)` と書くと、B = -9223372036854775807 に飽和させて受理し、すべての要素に当たる（手動確認）。`:nth-child(-n-99999999999999999999)` も `SyntaxError`。
- **Lexbor**：同じ（`diff: ":nth-child(n-99999999999999999999)"` が一致する）。
- **確信度**：中程度。仕様は範囲を定めていないので、拒否そのものを誤りとは言い切れない。ただ、同じ値を書き方で受理したり拒否したりする点は一貫していない。S2 で 2^63 の飽和を決めたのと同じ規則（飽和）に揃える余地がある。

### F7 擬似クラスと `:nth-of-type()` を specificity の B に数えない

- **仕様**：B は「class selectors, attributes selectors, and pseudo-classes」の数（S4 4427）。例外は `:is()`、`:not()`、`:has()`、`:nth-child()`、`:nth-last-child()`、`:where()` だけ（4437-4454）。
- **コード**：`pseudo_class`（`P:1783-1820`）は specificity を変えない。`pseudo_class_function`（`P:1831-1838`）は `:nth-child` と `:nth-last-child` のときだけ `specificity_set_b` を呼ぶ。Lexbor の `L:1474-1535`、`L:1576-1579` も同じ。
- **最小の再現**：`Makiri::Lexbor::CSS.parse_stylesheet("p:first-child, :hover, :nth-of-type(2) {}")`
  - **仕様の結果**：`[0, 1, 1]`、`[0, 1, 0]`、`[0, 1, 0]`。
  - **実装の結果**：`[0, 0, 1]`、`[0, 0, 0]`、`[0, 0, 0]`（手動確認）。`#c > d:hover` は `[1, 0, 1]`（仕様は `[1, 1, 1]`）。`:nth-last-of-type(1)` も `[0, 0, 0]`。
- **Lexbor**：同じ（`diff:` は specificity を比べ、`":hover"`、`":nth-of-type(2)"` が一致する）。
- **確信度**：高い。`parse_stylesheet` の結果で cascade を組む利用者（`X` の冒頭に書かれた dommy など）では、規則の優先順位が変わる。

### F8 関数の引数の中の specificity を、各成分 1 に丸めて計算する

- **仕様**：`:is()`、`:not()`、`:has()` の specificity は、引数のうち最も詳細な complex selector の specificity（S4 4440-4445）。`:nth-child()` と `:nth-last-child()` は擬似クラス一つに S の中で最も詳細な complex selector を足す（4446-4451）。complex selector の specificity は、含まれる単純セレクタをすべて数えたもの（4423-4430）。
- **コード**：関数の引数の中（`self.sel.parent` があるとき）では、`specificity_set_bc`（`P:1111-1129`）と `selector_hash`（`P:1459-1464`）は、新しい compound の始まり（結合子が `CLOSE` でない）で値を 0 にし、その成分を 1 にするだけで、数を足さない。項目の間では `func_specificity`（`P:1140-1152`）が、前の項目と今の項目の値を 32 ビットの整数として比べて大きいほうを残す。`restore_parent`（`P:1924-1955`）は compound の中なら OR、そうでなければ整数として大きいほうを取る。結果として、引数の中の一つの complex selector は、最後の compound に現れた種類ごとに 1 を持つ値になる（`a`、`b`、`c` の各成分が 0 か 1）。Lexbor の `L:219-280`、`L:1052-1084`、`L:1863-1905` も同じ。
- **最小の再現**：`Makiri::Lexbor::CSS.parse_stylesheet("a:is(.b, .c .d), :is(.c.d), :not(#a.b, #c#d), :nth-child(1 of .a .b), :has(> #a .b) {}")`
  - **仕様の結果**：`[0, 2, 1]`、`[0, 2, 0]`、`[2, 0, 0]`、`[0, 3, 0]`、`[1, 1, 0]`。
  - **実装の結果**：`[0, 1, 1]`、`[0, 1, 0]`、`[1, 1, 0]`、`[0, 2, 0]`、`[1, 0, 0]`（手動確認）。ほかに `:not(a b)` は `[0, 0, 1]`（仕様は `[0, 0, 2]`）、`:nth-child(2n+1 of p, .x > span)` は `[0, 2, 0]`（仕様は `[0, 2, 1]`）。
  - 仕様の例（4456-4512）は、引数がどれも一つの compound で、同じ種類の単純セレクタを二つ持たないので、すべて仕様どおりになる。`lcss:37` もこの形だけを確かめている。
- **Lexbor**：同じ（`diff: ":not(a b > c)"` などが一致する）。
- **確信度**：高い。

### F9 名前空間接頭辞つきの type selector を C に二回数える

- **仕様**：C は type selector の数で、universal selector は数えない（S4 4428-4429）。名前空間接頭辞は type selector の一部であり（1831-1833、1881-1883）、別に数えるものではない。`ns|E` は C に 1、`ns|*` は 0。
- **コード**：`element`（`P:1524-1536`）が接頭辞になる ident を読んだ時点で `specificity_set_c` を呼び（`P:1525`）、その後 `selector_ns_ident`（`P:1551-1579`）が local name の ident を読むともう一度呼ぶ（`P:1556`）。`ns|*` では二回目は呼ばないが、一回目が残る。`*|E` は `element_ns`（`P:1501-1513`）が数えないので正しい。Lexbor の `L:1150-1172`、`L:1423-1472` も同じ。
- **最小の再現**：`Makiri::Lexbor::CSS.parse_stylesheet("foo|div, foo|* {}")`
  - **仕様の結果**：（接頭辞 `foo` が宣言されていれば）`[0, 0, 1]`、`[0, 0, 0]`。
  - **実装の結果**：`[0, 0, 2]`、`[0, 0, 1]`（手動確認）。`svg|rect` も `[0, 0, 2]`。
- **Lexbor**：同じ（`diff: "a|b"`、`"ns|*"` が一致する）。
- **確信度**：高い。`parse_stylesheet` は `@namespace` を解決しないので、接頭辞つきの selector は宣言の有無にかかわらずこの値で返る。

### F10 specificity の成分が 512 以上になると次の成分に繰り上がる

- **仕様**：「implementations may have limitations on the size of A, B, or C. If so, values higher than the limit must be clamped to that limit, and not overflow.」（S4 4493-4496）
- **コード**：各成分は 9 ビットだが、`sp_set`（`P:292-294`）は値を 9 ビットに切らずに左へずらして OR するので、512 は上の成分の最下位ビットに入る。トップレベルの加算（`P:1117`、`P:1458`、`P:1931-1933`）に上限の確認がない。`SP_B_MAX`、`SP_C_MAX`（`P:285-286`）は関数の中の分岐の判定にしか使わない。Lexbor の `selector.h:160-166` と `L:219-262` も同じ。
- **最小の再現**：`Makiri::Lexbor::CSS.parse_stylesheet(".a" * 512 + " {}")`
  - **仕様の結果**：`[0, 511, 0]`（上限に丸める）か、上限がなければ `[0, 512, 0]`。
  - **実装の結果**：`[1, 0, 0]`。513 個なら `[1, 1, 0]`、`#a` を 512 個なら `[0, 0, 0]`、type selector を 512 個（`a a ...`）なら `[0, 1, 0]`（手動確認）。A が溢れると、Lexbor が `!important` と style 属性の印に使う上位のビット（`selector.h:130-140`）に入る。
- **Lexbor**：同じ。
- **確信度**：高い。ただし現実のセレクタで 512 に届くことはまずない。

### U1 `[*|a]`（任意の名前空間の属性）を受理しない

- **仕様**：属性セレクタの名前は `<wq-name>` で、`<ns-prefix>` は `[ <ident-token> | '*' ]? '|'`（S4 4561-4562、4573）。「An asterisk may be used for the namespace prefix indicating that the selector is to match all attribute names without regard to the attribute's namespace.」（2252-2254、例は 2266）
- **コード**：`attribute`（`P:1621-1675`）は最初の token が Delim のとき、それが `|` でなければ失敗にする（`P:1623-1625`）。`*` から始まる名前を読む分岐がない。AST は `[|a]` を名前空間 `*` で表す（`P:1632-1636`）ので、`[*|a]` を受理するには表現も変える必要がある（`X:183-191`、`LW:117-128` が `*` を「名前空間なし」と読んでいる）。Lexbor の `L:1186-1210` も同じ。
- **最小の再現**：`Makiri::HTML("<p a=1>").css("[*|a]")`
  - **仕様の結果**：有効なセレクタで、名前空間を問わず属性 `a` を持つ要素に当たる。
  - **実装の結果**：`SyntaxError`。`parse_stylesheet` は `:bad_style`（手動確認。`lcss:274` がこの答えを固定している）。
- **Lexbor**：同じ（`diff: "[*|a]"` が一致する）。
- **確信度**：高い。HTML の照合器は属性の名前空間の接頭辞を無視する（`css:176` の `[svg|r]`）ので、HTML では `[a]` と書けば代わりになる。

---

## 未カバーの step

状態が一致で、リポジトリのテストが当たらない step を挙げる。いずれも手動確認か、コードの経路の追跡で一致と判断した。`diff:` が当たる step でも、比べる相手は Lexbor なので、仕様どおりであることはこの監査の手動確認で確かめた。

1. **`<wq-name>` と `<ns-prefix>` の中の空白の禁止**（4596、4600）：`*| div`、`div| a`、`[ns |a]`、`[ns| a]`。`diff:` の項目はどれも空白を挟まない（`rnd:` の COMB に `|` と空白があるので、組み合わせとして当たっている可能性はある）。
2. **`<attr-matcher>` の二文字の間の空白の禁止**（4601）：`[a~ =b]`。
3. **値のない属性セレクタの modifier の拒否**：`[a i]`。名前のない `[]`、数字の `[1]` も同じ。
4. **`:` と擬似クラスの名前の間の空白の禁止**（1080、4598）：`: root`。擬似要素の `: :before`、`:: before` も同じ。
5. **引数なしの擬似クラスを関数として書いたもの、関数を引数なしで書いたもの**：`:hover()`、`:first-child()`、`:nth-child`。
6. **`:is()` と `:where()` の中の先頭の結合子の拒否**（4674）：`:is(> p)`、`:where(~ p)`。
7. **`:not()` の中の無効な項目でリスト全体が無効になること**：`:not(p, :foo)`。`diff:` は `":not(a,)"` だけ。
8. **一つのコロンの legacy の擬似要素**（4582、4612-4616）：`p:before` など。
9. **`:nth-child(... of S)` の `of` の大文字小文字と、S の forgiving でない読み方**（3915-3922、5257-5259）：`odd OF p`、`odd of p, :foo`、`odd of p,`、`odd of > p`。`diff:` は `":nth-child(2 of)"`、`":nth-child(of .a)"` だけ。
10. **An+B の `+3n + -6` 型の拒否と `n- 5` の受理**（SY 3420、3476-3478）：`2n + -1`、`2n- -1`、`2n - +1`、`-n- 3`。`diff:` は `":nth-child(n+-3)"` と `":nth-child(n- 1)"`。
11. **ID、クラス、属性、擬似クラスの組み合わせの specificity**（4426-4430）：`lcss:31` と `lcss:37` は 3 つの値しか確かめない。4501-4511 の例のうち、擬似クラスを含まないものは手動確認で一致した。擬似クラスを含むもの（F7）と関数の中（F8）は逸脱である。
12. **トップレベルの selector list のうち一つが無効なら全体が無効**（1384、1454-1478）：`p, :foo`。`lcss:179` は一つのセレクタの規則だけを確かめる。
