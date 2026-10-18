# CSS トークナイザ監査：入力の前処理と、§4 Tokenization のすべて

- 対象の仕様：CSS Syntax Level 3 `css-syntax-3/Overview.bs`（w3c/csswg-drafts commit `7b4c69ecaf9289d1bd5455d6bea10eec0b4a4399`）
  - §3.3「Preprocessing the input stream」（441–468）
  - §4「Tokenization」の総則とトークンの種類（471–537）、Definitions（810–976）、Tokenizer Algorithms（987–1826）
  - 行番号はこのファイルの行番号である。
- 対象の実装：Makiri の移植版 CSS 構文トークナイザ（Lexbor の `css/syntax/tokenizer.c`、`state.c` の移植）、そのトークン型、数値の変換（Lexbor の `core/strtod.c` の移植）
- 既知の差（`docs/parser_differences.ja.md` の S1–S4、stylesheet の C1–C3）は読んだうえで、新しい差としては数えていない。トークナイザに当たる既知の差は、この表にはない。
- この版の仕様には、旧版にあった「Convert a string to a number」の節はない。数値の値は「Consume a number」の step 6（1717–1725）が定める。16 章の該当行で扱う。
- 直列化（§9）はトークナイザでは行わない（selector text の直列化は `lexbor::selector_text` で、範囲外）。

## 読み方

### 略号

| 略号 | パス（リポジトリ相対） |
|---|---|
| `K` | `ext/makiri/rust/src/selectors/tokenizer.rs` |
| `T` | `ext/makiri/rust/src/selectors/token.rs` |
| `N` | `ext/makiri/rust/src/selectors/strtod.rs` |
| `Y` | `ext/makiri/rust/src/lexbor/stylesheet.rs` |
| `P` | `ext/makiri/rust/src/lexbor/selector_port_tests.rs` |
| `L` | `vendor/lexbor/source/lexbor/css/syntax/state.c`（Lexbor の原本） |

### テストの欄

- `P固定:` は `P:273` の `the_tokenizer_reads_lexbors_tokens` の固定入力。トークンを一つずつ（型、offset、長さ、文字列、数値、delim）Lexbor の C のトークナイザと比べる。
- `P乱:` は `P:295` の `the_tokenizer_reads_lexbors_tokens_on_random_inputs`。`P:174-262` の `PIECES` をつないだ入力 2 万件で同じ比較をする。どの piece が効くかだけを書く。
- `P選:` は `P:493` の `the_parser_builds_lexbors_selectors`（`P:310-485` の `SELECTORS`）、`P表:` は `P:797` の `the_stylesheet_parser_builds_lexbors_rules` の入力。どちらもパーサの出力を Lexbor と比べる。
- **`P` のテストはすべて Lexbor をオラクルにしている。** 移植は Lexbor を写したものなので、Lexbor と共有する仕様からのずれ（後述の F1–F6 はすべてそう）は、これらのテストでは見えない。仕様どおりの答えを書いて確かめているのは Ruby の spec だけである。
- Ruby の spec はファイル名と行番号、または `it` の文言で書く（`spec/css_spec.rb`、`spec/lexbor_css_spec.rb`、`spec/conformance/css_selectors_spec.rb`）。
- 「手動確認」は、この監査で組み込み済みの Ruby 拡張に入力を与えて確かめたことを示す。リポジトリのテストではない。入口は `Makiri::HTML(html).css(selector)`（構文エラーは `Makiri::CSS::SyntaxError`）と `Makiri::Lexbor::CSS.parse_stylesheet(text)`。
- WPT の `css/css-syntax/` はリポジトリに取り込まれていない。仕様が挙げる WPT（`escaped-eof.html`、`unclosed-url-at-eof.html` など）に当たるテストはない。

### 状態の欄

一致、意図した差、未実装、逸脱の四つを使う。parse error の報告については、出力が変わらない限り備考に書くだけにして、状態は変えない。トークナイザの parse error には全体に関わる注意がある。

- トークナイザは parse error を記録しない（`K:17`「The parse-error list is not kept: nothing reads it.」）。Lexbor は記録するが、読む者がいない。したがってどの分岐についても parse error の報告はなく、観測もできない。
- 観測できるのは、セレクタとして受け付けるか、何に当たるか（`css`、`matches?`）と、`parse_stylesheet` が返す規則、名前、prelude、値、selector text である。`parse_stylesheet` の値は、宣言の範囲を CSS のトークンとして読み直し、空白トークンの連続を空白一つにした、各トークンの**書かれたとおりの**文字列である（C1、`Y:136-170` の `normalize_value`）。このため、文字列、エスケープ、url のトークンの**値**は値の欄には現れず、見えるのはトークンの境界と種類である。トークンの値が見えるのは、セレクタ（属性値、クラス名、ID、An+B）、宣言名、at-rule の名前である。

### 実装の構造（前提）

- **前処理の段階はない。** 入力（UTF-8 のバイト列）をそのまま読み、各状態が CR、FF、CR LF、NUL を仕様の前処理後と同じに扱う。空白とコメントのトークンの文字列は `cr_block`、`ff_block`（`K:357-375`）で LF に正規化するが、どのトークンの文字列も読む者はいない。空白の判定（`K:63-65` の `is_ws`）は CR と FF を含む。改行の判定は各所で `0x0A | 0x0C | 0x0D` を見る。
- **NUL と不正な UTF-8 は Ruby の入口で拒否される。** セレクタも stylesheet も、不正な UTF-8 と埋め込みの NUL を受け付けない（`spec/css_spec.rb:236`、`spec/lexbor_css_spec.rb:321`、`:420`）。NUL の分岐と、不正な UTF-8 を U+FFFD にする分岐は、Rust のテストからしか届かない。
- **トークンは一つの状態関数で作る。** 最初のバイトで `dispatch`（`K:298-330`）が状態を選び、状態はトークンの終わりの位置を返す。位置は入力の添字で、EOF は `begin >= end` である（`K:277-281`）。
- **コメントはトークンとして作り、捨てる。** `comment`（`K:456-499`）はコメントのトークンを作り、`with_comment` が偽なら `next_token` が捨てて次を読む（`K:286-288`）。パーサが読むトークンの列は仕様の「consume comments」と同じになる。`with_comment` を真にするのは値の文字列を作る `normalize_value`（`Y:142`）だけである。
- **unicode-range は作らない。** `with_unicode_range` はどこでも偽のままで（`ext/makiri/rust/src/lexbor/css_tokens.rs:12`）、仕様の |unicode ranges allowed| の既定値（偽）と同じである。`@font-face` の `unicode-range` の値は、ほかの値と同じく文字列として返すだけで解釈しない。
- **トークンの型と値。** Lexbor のトークンは共用体で、`T:51-72` はその成員を並べたもの。hash の型（id か unrestricted か）を持たない（**F5**）。数値の符号は `have_sign`（真偽値）と `num` の符号で表す。
- **先読みの補助関数は範囲外。** `lookup_colon`、`lookup_important`、`lookup_declaration_ws_end`（`K:161-261`）は Lexbor のパーサが宣言の境界を決めるための先読みで、トークナイザのアルゴリズムではない。

---

## 1. Preprocessing the input stream（441–468）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| CR、FF、CR LF を LF 一つに置き換える（453-456） | 前処理はせず、各所で扱う。空白 `K:301`、`K:502-519`（`cr_block`、`ff_block`）。コメント `K:482-483`。文字列の改行 `K:530-534`。文字列の `\` と改行 `K:545-551`。エスケープの後の空白 `K:1110-1118`（CR LF を一つとして読む）。valid escape `K:1166-1174`。url の空白 `K:926`、`K:1015-1020` | 一致 | P固定: `'bad\nstring'`。P乱: `\r`、`\r\n`、`\x0c`、`\\\r\n`。`spec/css_spec.rb:352`「rejects one after a string a newline ends」（`\r`、`\n`、`\r\n`、`\f`）。`spec/lexbor_css_spec.rb:401`、`:408` | CR LF を二つの改行として数えると結果が変わるのは、文字列の `\` の直後と、エスケープの 16 進数字の直後だけで、どちらも CR LF を一つとして読む。手動確認：`#\31\r\na`、`#\31\ra`、`#\31\fa` はどれも id `1a` に当たる。`[t="x\` + CR LF + `y"]` は `xy` に当たる。`[t="x` + CR + `y"]` は構文エラー（bad string）。 |
| U+0000 を U+FFFD に置き換える（459-462） | 先頭の NUL は ident として読む `K:300`。コメント `K:484`、文字列 `K:529`、`K:552`、ident `K:836-837`、url `K:994`（`null_block`、`K:378-382`）。エスケープの値 0 は `K:427` | 一致 | P固定: `\0`（エスケープ）。P乱: `\x00`、`\\0` | Ruby の入口は NUL を拒否するので、生の NUL の分岐は Rust のテストからしか届かない。`start_ident_sequence` などの判定も NUL を名前の始まりとして扱う（`K:1223`、`K:1229`、`K:1233`）。U+FFFD は non-ASCII ident code point なので一致する。 |
| surrogate を U+FFFD に置き換える（459-462） | 入力は UTF-8 で、surrogate は符号化できない。不正なバイト列は `decode_utf_8_up_80`（`K:1241-1282`）が `ERROR_CODEPOINT` にし、`utf8_block`（`K:387-401`）が U+FFFD を書く。`0xED` の分岐 `K:326`、`K:968-985` | 一致 | P固定: `\xc3(\xff\xed\xa0\x80`。P乱: `\xed\xa0\x80`、`\xff`、`\x80` | Ruby の入口は不正な UTF-8 を拒否するので、Ruby からは届かない。CESU 形式の surrogate（`ED A0..BF ..`）は U+FFFD をいくつ書くかが WHATWG の復号と異なりうるが、届かない。 |

## 2. Tokenization の総則とトークンの種類（471–537）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| EOF トークンまで consume a token を繰り返す（474-479） | `K:269-295`（`next_token`）、`K:128-151`（読み置きの列） | 一致 | 全テスト | パーサはトークンを必要なときに読む。仕様の note（481-483）が許す読み方である。 |
| トークンの種類（484-510） | `T:5-38` | 一致 | 全テスト | Lexbor の型には `Comment` と `End` が加わる。`Comment` は `with_comment` のときだけ外に出て、パーサには届かない（`K:286-288`）。`End` はトークナイザが作らない。 |
| ident、function、at-keyword、hash、string、url は値を持つ（513-514） | `T:60-62`、`K:405-423`（`string_set`） | 一致 | 全テスト | |
| **hash は type flag（"id" か "unrestricted"）を持つ（515、536-537）** | なし | **未実装（F5）** | 未カバー | `T:51-72` に型の欄がない。ID セレクタとして受け付けてよいのは id 型だけだが、`#1a` も ID セレクタとして受け付ける。後述の F5 を参照。 |
| delim は一つの code point を値に持つ（518） | `T:67-68`、`K:1133-1137`、`K:1140-1150` | 一致 | P固定: `# \\`。P選: `a!b` | 非 ASCII の delim は `delim_set_cp` が code point に復号する。 |
| number、percentage、dimension は値と符号（"+"、"-"、なし）を持つ（521-523） | `T:63-66`。符号は `have_sign` と `num` の符号（`K:618`、`K:627`、`K:639-640`） | 一致 | P固定: `12px 1.5em -3 +.5`。P選: `:nth-child(n- 1)`、`:nth-child(3n+ 2)`、`:nth-child(n+-3)` | "+" と "-" は `num` の符号で区別する。`-0` も `-0.0` として符号が残る。 |
| number と dimension は type flag（integer か number）を持つ（525） | `T:65`（`is_float`）、`K:694-696` | 一致 | P選: `:nth-child(1.5)`、`:nth-child(2.0n)`、`:nth-child(1e3)` | percentage にも `is_float` を書くが、読む者はいない。手動確認：`li:nth-child(1e1)`、`li:nth-child(.5)` は構文エラー（integer 型でないため）。 |
| dimension は unit を持つ（526） | `T:60-62`（`string`） | 一致 | P固定: `12px 1.5em` | |
| unicode-range は始まりと終わりの code point を持つ（529-531） | `T:20`、`T:69-71` | 一致 | 未カバー | 作られない（14 章、17 章）。 |

## 3. Definitions（810–976）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| next input code point、current input code point、reconsume、EOF code point（816-834） | 入力の添字。EOF は `data >= end`（`K:277-281` ほか各状態） | 一致 | 全テスト | |
| digit（836-841） | `u8::is_ascii_digit` | 一致 | P固定: `12px` | |
| hex digit（843-847） | `K:52-59`（`hex`） | 一致 | P固定: `\\41 bc` | |
| uppercase letter、lowercase letter、letter（849-860） | `K:39-48`（`NAME_MAP` の 1） | 一致 | 全テスト | |
| non-ASCII ident code point（862-882） | `K:68-83`（`non_ascii`） | 一致 | P固定: `é€\u{10348}`。P乱: `\u{2070}`、`\u{00d7}`。P選: `é`、`.é`、`#€` | 範囲は仕様の 15 項目と同じ。手動確認：`.é`、`.a·`（U+00B7）、`.𐀀`（U+10000）は当たる。`.×`（U+00D7）、`.　`（U+3000）は構文エラー。 |
| ident-start code point（927-931） | ASCII は `NAME_MAP == 1`（`K:39-48`）、非 ASCII は `non_ascii` | 一致 | P選: `_a`、`é` | `NAME_MAP` の 0x80–0xFF の欄は Latin-1 の code point の表で、UTF-8 の先頭バイトに当てると意味がない。この欄を読むのは `start_ident_sequence` のハイフンの後だけで、それが F2、F3 である（12 章）。 |
| ident code point（933-937） | `NAME_MAP != 0`（`-` と数字は 2）、非 ASCII は `non_ascii` | 一致 | P選: `a--b` | |
| non-printable code point（939-947） | `K:1001`（0x0B、0x7F）、`K:1038`（0x01–0x08、0x0E–0x1F）、NUL は `K:994` で U+FFFD | 一致 | P乱: `\x01`、`\x0b`、`\x7f` | url の中でだけ使う。NUL は前処理で U+FFFD になるので non-printable には当たらない。実装も同じ。 |
| newline（949-955） | 各所の `0x0A \| 0x0C \| 0x0D` | 一致 | 1 章の行を参照 | CR と FF を含めて見るのは、前処理をしていないためで、前処理後の LF と同じになる。 |
| whitespace（957-961） | `K:63-65`（`is_ws`：空白、TAB、LF、FF、CR） | 一致 | P選: `a\tb`、`a\nb`、`a\rb`、`a\u{c}b` | |
| maximum allowed code point（963-964） | `K:427` | 一致 | P固定: `\\110000` | |
| ident sequence（966-976） | 定義だけ | 一致 | — | |

## 4. Consume a token（992–1210）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| まず consume comments（1000） | `K:310` → `K:456-499`、`K:286-288` | 一致 | 5 章を参照 | コメントをトークンとして読んでから捨てる。パーサが見る列は同じ。 |
| whitespace：空白をできるだけ読み、whitespace トークン（1005-1008） | `K:301`、`K:502-519` | 一致 | P固定: `a /* comment */ b`。P選: `a  >  b`。`spec/lexbor_css_spec.rb:58`「collapses the whitespace between a value's tokens」 | |
| U+0022：consume a string token（1010-1013） | `K:302` → `K:522-583` | 一致 | 8 章を参照 | |
| U+0023：次が ident code point か valid escape なら hash（1015-1036） | `K:303`、`K:586-609`（`consume_ident` は `K:608`） | 一致 | P固定: `#-a #1 #\\31 # \\`。P選: `#1`、`#-a`、`#\\31`、`#€`。`spec/lexbor_css_spec.rb` の selector text `#x\.y` | 手動確認：`#\31 a` は id `1a` に、`#a\`（EOF）は `a` + U+FFFD に当たる。 |
| U+0023：次の 3 つが ident sequence を始めるなら type flag を "id" にする（1027-1028） | なし | **未実装（F5）** | 未カバー | 2 章の行と F5 を参照。 |
| U+0023：それ以外は delim（1038-1040） | `K:588-589`（EOF）、`K:597`（非 ASCII で ident でない）、`K:600-605`（ident code point でない、`\` と改行） | 一致 | P固定: `# \\` の `# `。P乱: `#` | 手動確認：`#`、`#` + U+00A0、`#\` + LF は構文エラー。 |
| U+0027：consume a string token（1042-1045） | `K:302` | 一致 | P固定: `'unterminated` | |
| `(`、`)`（1047-1053） | `K:304-305` | 一致 | P選: `a(b)`、`a)` | |
| U+002B：starts with a number なら numeric（1055-1060） | `K:612-631`（13 章の判定を内側に書いたもの） | 一致 | P固定: `+.5`。P選: `:nth-child(+n)`、`:nth-child(n+-3)` | 手動確認：`li:nth-child(+2)` は 2 番目に当たる。 |
| U+002B：それ以外は delim（1062-1064） | `K:614-615`、`K:623-624`、`K:632` | 一致 | P選: `a+b`、`:nth-child(+ n)` | |
| `,`（1066-1068） | `K:307` | 一致 | P選: `a, b` | |
| U+002D：starts with a number なら numeric（1072-1075） | `K:637-642` | 一致 | P固定: `--x -x - -1 -.5 ->`、`12px 1.5em -3` | |
| U+002D：次の 2 つが `->` なら CDC（1077-1083） | `K:643-646` | 一致 | P表: `<!-- a { b: c } -->`。P乱: `-->` | 手動確認：`li-->` は構文エラー。stylesheet の最上位では CDO と CDC を読み飛ばす（P表の入力）。 |
| U+002D：starts with an ident sequence なら ident-like（1085-1089） | `K:647-649` → `K:957-965` | 一致（判定は 12 章、F2、F3） | P選: `--a`、`-a`、`-` | 分岐そのものは一致する。判定 `start_ident_sequence` のずれは 12 章で数える。 |
| U+002D：それ以外は delim（1091-1093） | `K:650` | 一致 | P選: `-` | |
| U+002E：starts with a number なら numeric（1095-1100） | `K:654-658` | 一致 | P固定: `.5e5` | |
| U+002E：それ以外は delim（1102-1104） | `K:659` | 一致 | P選: `.a.b#c`、`.1` | `.1` は数値なので、クラスセレクタにならない（構文エラー）。 |
| `:`、`;`（1106-1112） | `K:315-316` | 一致 | P選: `a;b`、`:first-child` | |
| U+003C：次の 3 つが `!--` なら CDO（1114-1122） | `K:317`、`K:1153-1161` | 一致 | P表: `<!-- a { b: c } -->`、`a <!-- { b: c }` | 手動確認：`li<!--` は構文エラー。 |
| U+003C：それ以外は delim（1124-1126） | `K:1162` | 一致 | P乱: `<` | |
| U+0040：次の 3 つが ident sequence を始めるなら at-keyword（1128-1134） | `K:318`、`K:663-668` | 一致（判定は 12 章、F2、F3） | P表: `@media screen { ... }`、`@\\66oo { }`、`@MEDIA screen;`。`spec/lexbor_css_spec.rb:95`「lower-cases an at-rule name」 | |
| U+0040：それ以外は delim（1136-1138） | `K:669` | 一致 | P表: `@ { }` | |
| `[`、`]`、`{`、`}`（1140-1169） | `K:321`、`K:323-325` | 一致 | P選: `[a]`、`a]`、`{a}`、`a}` | |
| U+005C：valid escape なら ident-like（1144-1149） | `K:322`、`K:673-676`（url を判定する `ident_like` に入る） | 一致 | P選: `\\61`、`\\31 23`。P表: `p{Col\\6fr:x}`。`spec/lexbor_css_spec.rb:71`「reads an escaped name as the name it spells」 | 手動確認：`\6C i`、`\4C I`、`\6Ci` はどれも `li` に当たる。 |
| U+005C：それ以外は delim（1154-1157） | `K:677` | 一致 | P乱: `\\\n`。`spec/lexbor_css_spec.rb:290`「never leaves a lone trailing backslash」（`.a\` + LF） | `\` の直後が EOF なら valid escape なので、delim になるのは改行が続くときだけ。 |
| digit：numeric（1171-1175） | `K:311-314` | 一致 | P固定: `12px 1.5em -3 +.5 1e3 ...` | |
| `U`、`u`：unicode ranges allowed かつ would start a unicode-range なら unicode-range（1177-1184） | `K:319`、`K:909-915` | 一致 | 未カバー | フラグは常に偽（既定値）。この分岐には入らない。 |
| `U`、`u`：それ以外は ident-like（1186-1189） | `K:319`、`K:916-939` | 一致 | P選: `url(a)`、`:is(url(x))`。P乱: `U+1?`、`u` | 手動確認：`U+0-7F` のような値は、フラグが偽の仕様と同じく ident、数値などに分かれる。 |
| ident-start code point：ident-like（1191-1195） | ASCII の letter と `_`：`K:320`（url を判定しない `ident_like_not_url`）。非 ASCII：`K:327` → `K:943-954`、`K:326` → `K:968-985`。NUL：`K:300` | 一致 | P選: `div`、`_a`、`é`。P乱: `€`、`\u{10348}` | url になりうるのは `u`、`U`、`\` で始まる名前だけなので、ほかの文字で url の判定を省いても結果は同じ。 |
| EOF：EOF トークン（1197-1199） | `K:279-281` | 一致 | 全テスト | |
| anything else：delim（1201-1204） | `K:328`（ASCII）、`K:953`、`K:984`（非 ASCII、`delim_set_cp`） | 一致 | P選: `a!b`。P乱: `$`、`^`、`\u{00d7}` | 手動確認：`.×` は構文エラー。 |

## 5. Consume comments（1213–1230）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `/*` から最初の `*/` までを読む（1219-1224） | `K:456-493` | 一致 | P固定: `a /* comment */ b`。P選: `a /* c */ b`、`a/**/b`。P表: `a /* c */ { b /* c */ : /* c */ d /* c */ ; }`。`spec/lexbor_css_spec.rb:85`「keeps a comment inside a value」 | 手動確認：`li/**/.é` は `li.é` と同じに当たる。`li:nth-child(2n/**/+1)` は `2n+1`。 |
| step の初めに戻る（連続するコメント、1225） | `K:272-290`（コメントを捨てて次のトークンを読む） | 一致 | P乱: `/*`、`*/`、`/**/` の連続 | |
| EOF で終わる：parse error（1224、1227-1228） | `K:494-496` | 一致 | P固定: `/* unterminated` | 手動確認：`a{b:c/*d}` の値は `c/*d}`（`}` もコメントに入る）。 |
| `/` の後が `*` でなければ、consume a token の anything else で delim | `K:459-461` | 一致 | P乱: `/` | |

## 6. Consume a numeric token（1233–1266）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| consume a number（1239）の後、次の 3 つが ident sequence を始めるなら dimension。unit は consume an ident sequence（1241-1252） | consume a number は `K:700-798`（16 章）。`K:807-810`。指数の `e` が数字に続かない場合 `K:752-759`（`1e` で EOF）、`K:771-779`（`1e-x`、`1e+`、`1ex`） | 一致 | P固定: `12px 1.5em ... 1e 1e- 1e+x`。P選: `:nth-child(2n+1)`、`:nth-child(10n-3)` | `1e+` では unit を `e` にし `+` を残す、`1e-x` では unit を `e-x` にする、という仕様の結果を、位置を戻して `consume_ident` を呼ぶことで作っている（`K:772-778`）。手動確認：`li:nth-child(2N-1)` は `2n-1` として当たる。判定 `start_ident_sequence` のずれ（F2、F3）はここにも及ぶが、unit が変わっても値の文字列は同じで、An+B では構文エラーのままなので、観測できない。 |
| `%` なら percentage（1254-1258） | `K:811-814` | 一致 | P固定: `2E-2%`。P乱: `%` | 手動確認：`li:nth-child(3%)` は構文エラー。 |
| それ以外は number（1264-1266） | `K:815` | 一致 | P固定: `-3 +.5 1e3` | |

## 7. Consume an ident-like token（1269–1308）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| consume an ident sequence（1275） | `K:916`、`K:958` | 一致 | 15 章を参照 | |
| 値が `url` に ASCII 大文字小文字を無視して一致し、次が `(`（1277-1279） | `K:917-919` | 一致 | P選: `url(a)`、`a url(b)`。P表: `a { b: url( x ) }`、`@import url(x.css) screen;`。P乱: `url(`、`URL(` | 比べるのは復号した値なので、`u\72l(` も url になる（手動確認）。 |
| 空白を、次の 2 つが空白である間読む。次が引用符、または空白と引用符なら function（1280-1287） | `K:923-929`、`K:935-936`（`(` の直後を返す） | 一致 | P固定: `url('q')`。P乱: `url('a')`。P表のランダム入力の `url( "a" )` | 実装は空白を一つも読まず、後の空白トークンに残す。仕様は最後の一つだけを残す。どちらも function トークンの後に空白トークンが一つ続く列になり、値の文字列も同じ（手動確認：`a{b:url(  "x"  )}` の値は `url( "x" )`）。 |
| それ以外は consume a url token（1288-1290） | `K:930-931` | 一致 | P固定: `url(  x.png  ) url(x y) url('q') url(\\)` | |
| **`url(` の後が空白だけで EOF になる** | `K:924-936`（空白を読み切って EOF なら function を返す） | **逸脱（F6）** | 未カバー | 仕様では、次の 1 つか 2 つが引用符ではないので consume a url token に進み、空白を読み、EOF で url トークン（値は空）を返す。実装は function トークンと空白トークンを返す。後述の F6 を参照。`url(` の直後がすぐ EOF の場合も同じく function になるが、トークンの文字列がどちらも `url(` で、観測できない。 |
| `url` 以外の名前で次が `(` なら function（1298-1303） | `K:920-921`、`K:959-961` | 一致 | P選: `a(b)`、`:is(a, b)` | |
| それ以外は ident（1305-1308） | `K:938-939`、`K:963-964` | 一致 | P選: `div` | |

## 8. Consume a string token（1311–1359）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| ending code point：string を返す（1327-1329） | `K:569-572` | 一致 | P選: `[a='b']`、`[a="b" i]` | 終わりの引用符は開いた引用符と同じもの（`K:523`）。 |
| EOF：parse error。string を返す（1331-1334） | `K:581-582` | 一致 | P固定: `'unterminated`。P表: `a { b: 'unterminated` | 手動確認：`[t="x` と `[t='x` は `t="x"` に当たる。 |
| newline：parse error。reconsume し bad-string を返す（1336-1340） | `K:530-534`（改行を読まずに返す） | 一致 | P固定: `'bad\nstring'`。P表: `a { b: "x\ny" }`。`spec/css_spec.rb:352`。`spec/lexbor_css_spec.rb:401` | 手動確認：`a{b:"x` + LF + `c:d}` の値は `"x c:d`。 |
| **`\` の次が EOF：何もしない（1344-1345）** | `K:539-543`（`escaped` を呼び、U+FFFD を足す） | **逸脱（F1）** | 未カバー | 後述の F1 を参照。 |
| `\` の次が newline：それを読む（1347-1349） | `K:545-551`（CR LF は二つとも読む） | 一致 | P乱: `\\\n`、`\\\r\n`。`spec/lexbor_css_spec.rb:408`「reads a backslash-newline as a string continuation」 | 手動確認：`[t="x\` + LF/CR LF/FF/CR + `y"]` はどれも `t="xy"` に当たる。 |
| `\` の次がそれ以外：consume an escaped code point（1351-1354） | `K:552`（NUL）、`K:553-565`（ASCII は `escaped`、非 ASCII はその文字） | 一致 | P乱: `\\'`。`spec/lexbor_css_spec.rb:219-222`（`[data-x="a\"b"]`、`[a="x\\\\y"]`、`[a="a\a b"]`） | 手動確認：`[t='x\79']`、`[t="x\y"]` は `xy` に当たる。`[t="\0"]` は U+FFFD に当たる。 |
| anything else：追加する（1356-1358） | `K:573-576`（非 ASCII）、`K:579`、NUL は `K:529` | 一致 | P選: `[a='b']` | |

## 9. Consume a url token（1366–1438）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| url トークンを作り、空白を読む（1381-1385） | 空白は `ident_like` が読む（`K:924-934`）。`url` は最初の空白でない位置から始まる（`K:988-991`） | 一致 | P固定: `url(  x.png  )` | |
| `)`：url を返す（1391-1393） | `K:995-1000` | 一致 | P固定: `url(  x.png  )`。P選: `url(a)` | |
| EOF：parse error。url を返す（1395-1398） | `K:1050-1054` | 一致 | P乱: `url(` の後に piece が続く入力 | 手動確認：`a{b:url(x\` の値は `url(x\`。 |
| whitespace：空白を読み、次が `)` か EOF なら url（1400-1405） | `K:1015-1025`、`K:1030-1031` | 一致 | P固定: `url(  x.png  )` | 手動確認：`a{b:url(x   ` の値は `url(x   `（url トークンが末尾の空白を含む）。 |
| whitespace の後がそれ以外：bad url の残りを読み、bad-url（1406-1409） | `K:1026` | 一致 | P固定: `url(x y)`。`spec/lexbor_css_spec.rb:75`「keeps every token but whitespace as written」（`url( x  y )`） | |
| `"`、`'`、`(`、non-printable：bad url（1411-1419） | `K:1001-1004`、`K:1038-1041`、`K:1046-1049` | 一致 | P乱: `'`、`"`、`(`、`\x01`、`\x0b`、`\x7f` | 手動確認：`url(x"y)`、`url(x(y)`、`url(x` + U+0001/U+000B/U+007F + `y)` の後の `;c:d` はどれも別の宣言として読まれる。 |
| `\` が valid escape：consume an escaped code point（1421-1425） | `K:1005-1007`、`K:1011-1013` | 一致 | P固定: `url(\\)`（`\)` を値に入れ EOF まで url） | 手動確認：`url(x\);c:d) ;e:f` は `x);c:d` を値とする url（宣言は b と e の二つ）。 |
| `\` が valid escape でない：bad url（1427-1431） | `K:1008-1010` | 一致 | P乱: `\\\n` | 手動確認：`url(x\` + LF + `y) ;c:d` の後の `c:d` は別の宣言になる。 |
| anything else：追加する（1433-1436） | `K:1033-1037`（非 ASCII）、`K:1044` | 一致 | P固定: `url(  x.png  )` | |

## 10. Consume an escaped code point（1444–1478）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| hex digit：合わせて 6 つまで読む（1456-1459） | `K:1087-1108` | 一致 | P固定: `\\41 bc`。`spec/lexbor_css_spec.rb` の selector text `.\31 0` | 手動確認：`#\1F600b` は 6 桁 `1F600b` を読み、U+10FFFF を超えるので U+FFFD（id U+FFFD に当たる）。 |
| 続く whitespace を一つ読む（1460-1462） | `K:1109-1120`（CR LF は一つとして読む。FF も読む） | 一致 | P固定: `\\41 bc`。P乱: `\\fffd ` | 手動確認：`#\31\r\na`、`#\31\ta`、`#\31\fa`、`#\31\ra` はどれも id `1a`。`#\1F600 b` は `😀b`、空白二つなら子孫結合子。 |
| 0、surrogate、U+10FFFF を超える値は U+FFFD（1463-1468） | `K:426-429` | 一致 | P固定: `\\0 \\110000 \\D800` | 手動確認：`#\0`、`#\D800`、`#\110000` は id U+FFFD、`#\10FFFF` は id U+10FFFF に当たる。 |
| EOF：parse error。U+FFFD（1470-1473） | `K:1088-1091`（`cp = 0` のまま `K:427` で U+FFFD） | 一致 | P固定: `#-a #1 #\\31 # \\` の末尾の `\` | 手動確認：`#a\` は id `a` + U+FFFD、`li\` は何にも当たらない（`li` + U+FFFD）。 |
| anything else：その code point（1475-1477） | `K:1093-1102`（不正な UTF-8 は `ERROR_CODEPOINT` から U+FFFD） | 一致 | P乱: `\\'`。`spec/lexbor_css_spec.rb` の selector text `.md\:block`、`.a\ b` | |

## 11. Check if two code points are a valid escape（1485–1505）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 一つ目が `\` でなければ偽（1498-1499） | `K:1167` | 一致 | 全テスト | |
| 二つ目が newline なら偽（1501-1503） | `K:1169`（LF、FF、CR） | 一致 | P乱: `\\\n`、`\\\r\n`。`spec/lexbor_css_spec.rb:290` | |
| それ以外は真（EOF を含む）（1505） | `K:1169-1171` | 一致 | P固定: 末尾の `\` | |

## 12. Check if three code points would start an ident sequence（1508–1546）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| U+002D：二つ目が ASCII で、ident-start code point、`-`、または二つ目と三つ目が valid escape なら真、ほかは偽（1525-1531） | `K:1213-1226`（`NAME_MAP == 1`、`-`、`valid_escape`、NUL） | 一致 | P選: `--a`、`-a`、`-`、`.\\-`。P固定: `--x -x -`。`spec/lexbor_css_spec.rb` の selector text `.\-\-x`、`.-\31` | |
| **U+002D：二つ目が非 ASCII のとき、それが ident-start code point なら真（1527-1528）** | `K:1227-1229`（UTF-8 の**先頭バイト**を `NAME_MAP` で引く） | **逸脱（F2、F3）** | 未カバー | 先頭バイトが 0xD7 の文字（U+05C0–U+05FF）は偽になる（F2）。ほかの先頭バイト（0xC2–0xD6、0xD8–0xF4）の文字は、ident code point でなくても真になる（F3）。後述を参照。 |
| ident-start code point：真（1533-1535） | `K:1233`（ASCII、NUL）、`K:1235`（非 ASCII、`start_ident_utf_8_80`） | 一致 | P選: `div`、`é` | ハイフンで始まらない非 ASCII は正しく `non_ascii` で判定する。 |
| U+005C：一つ目と二つ目が valid escape なら真（1537-1541） | `K:1230-1231` | 一致 | P表: `@\\66oo { }` | |
| anything else：偽（1543-1545） | `K:1233` | 一致 | P表: `@ { }` | |

## 13. Check if three code points would start a number（1552–1598）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `+`、`-`：二つ目が digit なら真（1571-1573） | `K:1187-1194`（`+` は `K:617`） | 一致 | P固定: `-3`、`-1` | |
| `+`、`-`：二つ目が `.` で三つ目が digit なら真（1575-1580） | `K:1196-1202`（符号の後の位置で `.` の判定に落ちる）。`+` は `K:621-629` | 一致 | P固定: `+.5`、`-.5` | 符号の後に digit も `.` もなければ、`K:1196`、`K:1199` のどちらも成り立たず偽になる。 |
| `+`、`-`：それ以外は偽（1582） | `K:1203`、`K:1189-1190` | 一致 | P選: `:nth-child(+ n)` | |
| `.`：二つ目が digit なら真（1584-1589） | `K:1199-1202` | 一致 | P固定: `.5e5`。P選: `.1` | |
| digit：真（1591-1593） | `K:1196-1198` | 一致 | P固定: `12px` | |
| anything else：偽（1595-1597） | `K:1203` | 一致 | P選: `--a` | |

## 14. Check if three code points would start a unicode-range（1601–1623）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 一つ目が `U`/`u`、二つ目が `+`、三つ目が `?` か hex digit なら真、ほかは偽（1615-1623） | `K:909-913`（一つ目は `K:319` で決まっている） | 一致 | 未カバー | `with_unicode_range` が偽なので評価されない。 |

## 15. Consume an ident sequence（1626–1657）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| ident code point：追加する（1644-1646） | ASCII `K:823-842`（`NAME_MAP != 0`）、非 ASCII `K:843-849`（`non_ascii`）、NUL `K:836-837` | 一致 | P選: `a--b`、`.é` | |
| valid escape：consume an escaped code point（1648-1651） | `K:825-827`、`K:833-835`（`\` + EOF も `escaped` で U+FFFD） | 一致 | P選: `a\\:b`、`\\61`。`spec/lexbor_css_spec.rb` の selector text | |
| anything else：reconsume して返す（1653-1656） | `K:839`、`K:845-848`。`\` + 改行は `\` の前で止まる（`K:828-831`） | 一致 | P選: `a)`。`spec/lexbor_css_spec.rb:290` | |

## 16. Consume a number（1660–1729）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| type を integer にする（1676-1679） | `K:719`（`is_float = false`） | 一致 | P選: `:nth-child(2)` | |
| 先頭の `+`、`-` を読み、sign character にする（1681-1685） | 呼び出し側が読む：`K:618`、`K:627`（`+`）、`K:638-640`（`-`、値を反転） | 一致 | P固定: `-3 +.5` | |
| **digit を読み続け number part に足す（1687-1689）** | `K:702-716`（128 バイトの buffer に入れ、129 桁目以降を捨てる） | **逸脱（F4）** | 未カバー | 整数部が 128 桁を超えると、捨てた桁を指数に足さないので値が変わる。後述の F4 を参照。 |
| `.` と digit が続けば、小数部を読み type を number にする（1691-1700） | `K:721-744`（`.` の後が digit でなければ `.` の前で止まる：`K:723-726`） | 一致 | P固定: `1.5em`、`+.5`、`.5e5`。P選: `:nth-child(1.5)` | 小数部も 128 バイトの buffer に入れ、溢れた桁は捨てる。指数は buffer に入れた桁数で決めるので、失うのは精度だけで、桁の位置はずれない。number 型の値を読む者（An+B）は number 型を拒否するので観測できない。 |
| `E`/`e`、符号、digit が続けば指数を読み type を number にする（1702-1715） | `K:746-797` | 一致 | P固定: `1e3 1e 1e- 1e+x 2E-2% .5e5`。P乱: `1e308`、`1e400`、`1.0e-400` | 指数の値は `i32::MAX / 10` を超えたら増やさない（`K:783`）。値は無限大か 0 になり、どちらにしても An+B では使えない。 |
| 値を求める：number part を 10 進数として読み、指数の 10 の冪を掛ける（1717-1725） | `K:681-697`（`numeric_set`）、`N:296-322`（`strtod_internal`）、`N:245-292` | 一致 | `N:329` `integers_read_exactly`。P固定、P乱の数値（`num` のビット列を Lexbor と比べる）。P選: `:nth-child(9223372036854774784)`、`:nth-child(99999999999999999999)` | 値は double で、変換は正しく丸めない（`N:4-6`、big number の補正がない）。手動確認：2^53 未満の整数 2 万個を `li:nth-child(-Kn+(K+1))` で調べ、すべて正確だった。2^53 を超える整数は double に丸まる（`:nth-child(-9007199254740993n+9007199254740994)` は仕様の数学的な値では 1 番目、実装は 2 番目に当たる）。CSS は有限の精度と範囲を UA に許しており（2^63 の扱いは S2）、ここでは差に数えない。 |
| value、type、sign character を返す（1727-1728） | `K:694-696`、`token.have_sign` | 一致 | P固定 | |

## 17. Consume a unicode-range token（1736–1797）

このトークンは「consume the value of a unicode-range descriptor」（3294-3308）からしか呼ばれない。Makiri はその値を解釈しないので、どの行にも入らない。コードは Lexbor のまま残っている。

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 最初の 2 つ（`U+`）を捨てる（1757） | `K:863` | 一致 | 未カバー | 到達しない。 |
| hex digit を 6 つまで、足りなければ `?` を合計 6 つまで読む（1759-1767） | `K:865-891` | 一致 | 未カバー | `?` を読むのは最初の部分だけ（`K:871` の `mq`）。 |
| `?` があれば、0 にした値を始まり、F にした値を終わりにする（1769-1779） | `K:881-884` | 一致 | 未カバー | `cp << 4q` と `\| (16^q - 1)` は、`?` を 0 と F に置き換えた値と同じ。 |
| なければ始まりにする（1781-1782） | `K:892-894` | 一致 | 未カバー | |
| `-` と hex digit が続けば、hex digit を 6 つまで読み終わりにする（1784-1795） | `K:895`、`K:899-903` | 一致 | 未カバー | |
| それ以外は始まりと終わりを同じにする（1797） | `K:895-897` | 一致 | 未カバー | |

## 18. Consume the remnants of a bad url（1800–1826）

| step（分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `)`、EOF：戻る（1812-1815） | `K:1062-1064`、`K:1077-1078` | 一致 | P固定: `url(x y)` | |
| valid escape：consume an escaped code point（1817-1821） | `K:1065-1070`（`\` の次の一文字を飛ばす） | 一致 | 未カバー（P乱で `\\)` が bad url の中に来る入力に当たりうる） | エスケープを最後まで読まず、`\` の次の一文字だけを飛ばす。エスケープが読む残り（hex digit と空白一つ）は `)` を含まないので、どこで戻るかは変わらない。`\` + 改行（valid escape でない）でも改行を飛ばすが、改行は `)` ではないので同じ。手動確認：`url(x y\);c:d)}e{f:g}` は `\)` で終わらず、`;c:d)` までが一つの bad url になる。 |
| anything else：何もしない（1823-1825） | `K:1071-1075` | 一致 | P固定: `url(x y)` | |

---

## 集計

| 状態 | 件数 |
|---|---|
| 一致 | 118（このうち 2 行は「一致（判定は 12 章、F2、F3）」：4 章のハイフンと `@` の分岐そのものは一致し、使う判定が F2、F3 に当たる） |
| 意図した差 | 0 |
| 未実装 | 2（F5：hash の type flag。2 章と 4 章の 2 行） |
| 逸脱 | 4（F1、F2 と F3（12 章の 1 行）、F4、F6） |

合計 124 行。

一致の行のうち、Lexbor と同じく、観測できない点で仕様と違うものは次のとおり。

- parse error をまったく記録しない（`K:17`）。
- `url(` の後の空白を一つも読まずに function を返す（7 章、引用符が続く場合）。トークンの列と値の文字列は同じになる。
- `url(` の直後が EOF のとき、url ではなく function を返す（7 章）。文字列はどちらも `url(` である。
- 数値の小数部の 129 桁目以降と、指数の大きすぎる桁を捨てる（16 章）。number 型の値と、無限大か 0 になる値で、An+B は使わない。
- 数値の変換は正しく丸めず、2^53 を超える整数は double に丸める（16 章）。
- bad url の残りの中のエスケープを最後まで読まない（18 章）。戻る位置は同じ。
- percentage にも `is_float` を書く（2 章）。

範囲外で気づいたこと（数には入れない）：An+B の `:nth-child(2n 1)` を受け付け、`2n` として当たる（手動確認：3 つの `li` のうち 2 番目と 4 番目）。仕様（3459-3475）の `<n-dimension>` の後には `<signed-integer>` か、`+`/`-` と `<signless-integer>` しか続かず、構文エラーになるはずである。An+B の解析（§6）はこの監査の範囲外なので、別の監査で確かめる。

---

## 逸脱と未実装

### F1 文字列の中で `\` の直後が EOF のとき、U+FFFD を足す

- **仕様**：consume a string token の U+005C の分岐（1342-1345）：「If the next input code point is EOF, do nothing.」値には何も足さず、次の反復で EOF の分岐（1331-1334）が文字列を返す。
- **コード**：`K:535-543`。`\` の後が EOF（`data >= end`）なら `escaped`（`K:1083-1123`）を呼ぶ。`escaped` は何も読めずに `cp = 0` のまま `codepoint_to_ascii`（`K:426-429`）に渡し、U+FFFD を書く。consume an escaped code point の EOF の分岐（1470-1473）を、文字列の中にも当ててしまっている。
- **再現**：`Makiri::HTML('<p id=a t="x"></p><p id=h t="x&#xFFFD;"></p>').css(%([t="x\\))`（セレクタの文字列は `[t="x\`）
  - **仕様の結果**：属性値の文字列は `x`。`]` がないまま EOF で閉じた属性セレクタ `[t="x"]` として、`p#a` に当たる（手動確認：`[t="x` は `p#a` に当たるので、EOF で閉じる部分は仕様どおりに動く）。
  - **実装の結果**：属性値は `x` + U+FFFD で、`p#h` に当たる（手動確認）。`[t='x\` も同じ。
- **Lexbor**：同じ（`L:543-547`）。移植はこれを写した。
- **テスト**：なし。`P` のテストは Lexbor と比べるので見えない。
- **確信度**：高い。仕様の文は明示的で、Ruby 拡張で確かめた。影響は、文字列が `\` で終わる入力（途中で切れたセレクタ）だけで小さい。

### F2 ハイフンの後の U+05C0–U+05FF を名前の始まりと認めない

- **仕様**：would start an ident sequence の U+002D の分岐（1525-1531）。二つ目が ident-start code point なら真。U+05C0–U+05FF は non-ASCII ident code point（「between U+037F and U+1FFF」、862-882）なので ident-start code point である。
- **コード**：`start_ident_sequence` の `K:1213-1229`。二つ目のバイトが 0x80 以上のとき、`K:1219` の ASCII の判定に入らず、`K:1229` で `NAME_MAP[ch]` を引く。`NAME_MAP`（`K:39-48`）の 0x80–0xFF の欄は Latin-1 の code point の表（U+00B7、U+00C0–U+00FF から U+00D7、U+00F7 を除く）で、UTF-8 の先頭バイトに当てている。U+05C0–U+05FF の先頭バイトは 0xD7 で、表では 0（× の位置）なので偽になる。`K:1227-1228` のコメントはこの読み方を Lexbor のものとして記録しているが、`docs/parser_differences.ja.md` には載っていない。
- **再現**：
  - `Makiri::HTML(%(<p id=a class="-&#x5D0;"></p>)).css(".-א")`（`.-א`）
    - **仕様の結果**：`-א` は ident なのでクラスセレクタになり、`p#a` に当たる。
    - **実装の結果**：`Makiri::CSS::SyntaxError`（手動確認）。`-` が delim になり、`.` の後に ident がない。`.-\5D0`（エスケープ）なら `p#a` に当たる。U+05BF（先頭バイト 0xD6）や U+0600（0xD8）は正しく当たり、U+05C0、U+05D0、U+05FF だけが構文エラーになることを確かめた。
  - `Makiri::Lexbor::CSS.parse_stylesheet("@-א x {}")`
    - **仕様の結果**：at-rule、名前 `-א`、prelude `x`。
    - **実装の結果**：`[{:type=>:bad_style, :selector_text=>"@-א x", ...}]`（手動確認）。
  - `parse_stylesheet("a{-א: 1}")`：仕様では宣言 `-א: 1`。実装では宣言がない（手動確認）。続く宣言も消えるが（`a{-א: 1; c: 3}` の宣言は空）、それは宣言でないものの後の回復（パーサ）の振る舞いで、ここでは数えない。
- **Lexbor**：同じ（`L:1759-1786`）。
- **テスト**：なし。P乱の `PIECES` に `-` と非 ASCII の文字はあるが、0xD7 で始まる文字はなく、あっても Lexbor と比べるので見えない。
- **確信度**：高い。コードの経路を追い、Ruby 拡張で確かめた。影響はヘブライ文字の一部（U+05C0–U+05FF）をハイフンの直後に書いた名前に限られる。

### F3 ハイフンの後の ident でない非 ASCII 文字を、名前の始まりと認める

- **仕様**：F2 と同じ分岐（1525-1531）。二つ目が ident-start code point でも `-` でも valid escape の始まりでもなければ偽で、`-` は delim になる（1091-1093）。at-keyword も、`@` の後の 3 つが ident sequence を始めないので delim になる（1136-1138）。
- **コード**：F2 と同じ `K:1229`。先頭バイトが 0xC2–0xD6、0xD8–0xF4 の文字は、ident code point でなくても `NAME_MAP` が 1 なので真になる。たとえば U+00A0–U+00B6（0xC2）、U+00D7 ×（0xC3）、U+037E（0xCD）、U+2000–U+206F の空白や句読点（0xE2）、U+3000（0xE3）、U+FDD0–U+FDEF（0xEF）。真になると `consume_ident` は `-` だけを読み、その文字の前で止まる（`K:845-848`）ので、名前が `-` の ident、at-keyword、dimension ができる。
- **再現**：`Makiri::Lexbor::CSS.parse_stylesheet("@-¶ x {}")`（`@-¶ x {}`）
  - **仕様の結果**：`@`、`-`、`¶` は delim で、規則は prelude `@-¶ x` の qualified rule になり、セレクタとして不正なので `{:type=>:bad_style, :selector_text=>"@-¶ x", ...}`（F2 の `@-א x {}` に実装が返すのと同じ形）。
  - **実装の結果**：`[{:type=>:at_rule, :name=>"-", :prelude=>"¶ x", :rules=>[]}]`（手動確認）。`@-× x {}`、`@-` + U+3000 + ` x {}` も、名前 `-` の at-rule になる。
  - セレクタと宣言名では観測できなかった。名前 `-` の後に ident でない文字の delim が続き、仕様と同じく構文エラーか不正な宣言になる（手動確認：`.-×`、`-× {x:y}`、`a{-×: 2}`）。
- **Lexbor**：同じ（`L:1759-1786`）。
- **テスト**：なし。P乱の `PIECES` に `-` と `\u{00d7}` があり、この組み合わせはありうるが、Lexbor と比べるので見えない。
- **確信度**：高い。観測できるのは at-rule の名前（と、そこから決まる規則の種類）だけである。

F2 と F3 の原因は同じ一行（`K:1229`）で、直すなら、二つ目のバイトが 0x80 以上のときに `start_ident_utf_8_80`（`K:1177-1183`）で code point を判定すればよい。Lexbor との比較テストでは、この差を説明する述語（ハイフンの直後の非 ASCII の文字）で許すことになる。

### F4 数値の整数部の 129 桁目以降を捨て、値が変わる

- **仕様**：consume a number の step 3（1687-1689）は、digit をすべて number part に足す。step 6（1717-1719）はそれを 10 進数として読む。桁数に上限はない。
- **コード**：`K:700-716`。digit を 128 バイトの buffer に入れ、満ちたら以降の桁を捨てる（`K:707`）。小数部は、buffer に入れた桁数で指数を決めるので桁の位置がずれないが（`K:727-738`）、整数部の捨てた桁は指数に足さない。値は「最初の 128 桁を整数として読んだもの」になる。先頭の 0 も 1 桁と数えるので、0 を 128 個並べた後に書いた数字はすべて失われる。
- **再現**：`Makiri::HTML("<ul><li id=1></li><li id=2></li><li id=3></li></ul>").css("li:nth-child(#{'0' * 128}1)")`
  - **仕様の結果**：値は 1。`li#1` に当たる。
  - **実装の結果**：値は 0 で、何にも当たらない（手動確認：`[]`）。0 が 127 個なら（合わせて 128 桁）`li#1` に当たる。
  - 同じ結果になる入力：`li:nth-child(#{'0' * 128}2n)`（仕様では 2 番目に当たる、実装は `[]`）、`li:nth-child(-n+#{'0' * 128}2)`（仕様では 1、2 番目、実装は `[]`）、`li:nth-child(+#{'0' * 128}1)`。先頭が 0 でない 129 桁以上の整数（`1` の後に 0 が 130 個など）でも値は変わるが、どちらにしても文書の子の数を超えるので、An+B では観測できない。
- **Lexbor**：同じ（`L:989-1009`、`lxb_char_t buf[128]`）。
- **テスト**：なし。P乱の `123456789012345678901234567890`（30 桁）と P選の 20 桁の値は buffer に収まる。
- **確信度**：高い。An+B の値として観測でき、Ruby 拡張で確かめた。影響は、先頭に 0 を 128 個以上書いた整数という不自然な入力に限られる。

### F5 hash トークンが type flag を持たず、`#1a` などを ID セレクタとして受け付ける（未実装）

- **仕様**：トークンの定義（515）：hash は type flag（"id" か "unrestricted"）を持つ。consume a token の U+0023（1027-1028）：次の 3 つが ident sequence を始めるときだけ "id" にする。note（536-537）：「Only hash tokens with the "id" type are valid ID selectors.」
- **コード**：`T:51-72` に type flag の欄がなく、`hash`（`K:586-609`）は型を決めない。セレクタパーサは hash トークンなら何でも ID セレクタにする。
- **再現**：`Makiri::HTML('<p id="1a"></p><p id="-1"></p>').css("#1a")`
  - **仕様の結果**：`#1a` の hash は unrestricted（`1` は ident を始めない）なので、ID セレクタとして不正。`Makiri::CSS::SyntaxError` になる。ブラウザの `querySelector('#1a')` も SyntaxError を投げる。
  - **実装の結果**：`p#1a` に当たる（手動確認）。`#-1` も `p#-1` に当たる。`#--a`、`#-a`、`#\31 a` は id 型なので、受け付けるのは仕様どおり。
  - `parse_stylesheet("#1a{x:y}")` も style 規則として受け付ける。
- **Lexbor**：同じ。Lexbor のトークンにも type flag がない。
- **テスト**：P選の `#1` は Lexbor との一致だけを見る。仕様の答え（構文エラー）を確かめるテストはない。
- **確信度**：高い（トークナイザに欄がないこと）。観測できる差はセレクタパーサで生じる。数字で始まる ID を `#1a` と書いて当たることを当てにする呼び出し側がありうるので、直すなら互換への影響を確かめる必要がある。

### F6 `url(` の後が空白だけで EOF になると、url ではなく function を返す

- **仕様**：consume an ident-like token（1277-1290）。`url(` の後、次の 2 つが空白である間空白を読む。最後の空白の次は EOF なので、「次の 1 つか 2 つが引用符、または空白と引用符」は成り立たず、consume a url token に進む。そこで空白を読み（1385）、EOF の分岐（1395-1398）で値が空の url トークンを返す。トークンは `url(` から EOF までの全体である。
- **コード**：`K:923-936`。空白でない文字を探して最後まで空白だったら、`(` の直後を返して function トークンにする。空白は次の空白トークンになり、function は EOF で閉じる。
- **再現**：`Makiri::Lexbor::CSS.parse_stylesheet("a{b:url(   ")`
  - **仕様の結果**：値のトークンは url トークン一つ（`url(   `）。C1 の規則（トークンを書かれたとおりに並べる）では、値は `url(   ` になる。url トークンが末尾の空白を含むことは、`a{b:url(x   ` の値が `url(x   ` になることで確かめた。
  - **実装の結果**：値は `url(`（手動確認）。function トークン `url(` の後の空白トークンは、値の末尾の空白として削られる。
  - `@import url(   ` の prelude は、どちらでも `url(` になる（prelude は前後の空白を文字列として削る）。セレクタでは、どちらでも構文エラーになる。
- **Lexbor**：同じ（`L:1340-1366`）。
- **テスト**：なし。
- **確信度**：中。トークンの種類の差は確かだが、観測できるのは Makiri が定めた値の文字列（C1）の末尾の空白だけで、影響はごく小さい。仕様のトークンの列で CSSOM を作るなら、値は空の url（`url("")`）になり、関数の閉じ忘れとは意味が違う。

### 未実装

F5 の 1 件（2 章と 4 章の 2 行）。ほかの step はすべてコードがある。unicode-range（14 章、17 章）はコードがあり、到達しないだけである。

---

## 未カバーの step

状態が一致で、仕様の答えを確かめるテストも Lexbor との比較テストも当たらない step を挙げる。いずれも手動確認か、コードの経路の追跡で一致と判断した。

1. **unicode-range のすべて**（14 章、17 章、2 章の unicode-range の行、4 章の `U`/`u` の最初の分岐）。`with_unicode_range` を真にする呼び出しがなく、到達しない。P乱の `U+1?` はフラグが偽の経路だけを通る。
2. **bad url の残りの中の valid escape**（18 章）。P固定には `url(x y)` のような bad url はあるが、その中に `\` がある入力はない。P乱で偶然当たりうる。手動確認：`url(x y\);c:d)}e{f:g}`。

（参考）`P` のテストは多くの行に当たるが、すべて Lexbor との一致を見るもので、仕様の答えを書いたものではない。仕様の答えを Ruby の spec で確かめているのは、文字列の改行（8 章）、`\` と改行の続き（8 章）、エスケープした名前（4 章、15 章）、selector text のエスケープ（10 章、15 章）、url の空白と bad url（9 章）、コメントを残した値（5 章）、空白の畳み込み（4 章）に限られる。数値の値（16 章）、CDO と CDC（4 章）、エスケープの範囲外の値と EOF（10 章）、non-printable の url（9 章）は、Ruby の spec にも仕様の答えを書いたものがなく、手動確認だけである。
