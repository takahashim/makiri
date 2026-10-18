# HTML トークナイザ監査（2）：markup declaration open から numeric character reference end まで、および名前付き文字参照の表

- 対象の仕様：HTML Standard `source`（commit `de953a7cf0523f5873a1904ba2ce010c689dfb66`）の 144449–145820 行（13.2.5.42 Markup declaration open state 〜 Numeric character reference end state）と 151920–151973 行（Named character references）。
- 対象の実装：`ext/makiri/rust/src/html/tokenizer/`（`state.rs`、`state_comment.rs`、`state_doctype.rs`、`mod.rs`）、`ext/makiri/rust/src/html/res/`（`entities.rs`、`mod.rs`）、生成器 `ext/makiri/rust/src/lexbor/res_tests.rs`。
- 既知の差（`docs/parser_differences.ja.md` の P1–P3、L1、S1–S4、C1–C3、M1–M5、B1–B3）は、この範囲の状態には一つも当たらない（P* はすべて tree construction、S2–S4 と C* はセレクタ/stylesheet）。B2（PI token をブラウザ二つが未実装）はオラクル側の話で、実装は仕様どおり。
- 確認の方法：各分岐をコードで追い、観測できるものは組み込み済みの Ruby 拡張（`Makiri::HTML`、`Makiri::DocumentFragment.parse`）で入力を与えて確かめた。テストの対応は html5lib-tests の tokenizer（`spec/conformance/data/html5lib-tests/tokenizer/*.test`、Data state を初期状態に含むもの）と WPT の tree construction（`spec/conformance/data/wpt/html/syntax/parsing/resources/*.dat`、番号は runner と同じ 1 始まり）を正規表現で検索して取った。
- 実装の前提（全状態に共通）：入力は一つの chunk として渡され（`Tokenizer::chunk`）、EOF は `input.len()` の位置に置いた仮想の NUL 1 byte を `is_eof = true` で読む一回の追加パスで表す（`mod.rs:255-270`）。したがって各状態の「`c == 0x00 && t.is_eof`」が仕様の EOF 分岐、「`c == 0x00`（`!is_eof`）」が U+0000 分岐である。CR の扱い（CRLF/CR → LF）は入力ストリームの前処理に当たり、各状態の `cr_case!`（`state.rs:29-47`）が行う。
- chunk 境界の途中で読みかけのキーワードを持ち越す補助状態（`markup_declaration_comment`／`_doctype`／`_cdata`、`doctype_after_name_public`／`_system`）は、Makiri では「入力の終わりに 7 文字（または 6 文字）未満しか残っていない」ときにだけ入る。この場合の扱いを分岐ごとに確かめた。

状態の凡例：一致／意図した差／未実装／逸脱。パース・エラーのコードや位置だけの違いは逸脱としない（備考に書く）。

---

## 1. Markup declaration open state（spec 144449–144478）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `--`（144456）：消費し、空の comment token を作り comment start state へ | `state.rs:626-634`、`state_comment.rs:8-15` | 一致 | h5l test1「Start of a comment」以外の通常コメント全般、test3 `<!--` | 残りが 1 byte（`<!-` で入力が終わる）のときは `markup_declaration_comment`（`state.rs:675-684`）へ。EOF なら `-` を buffer に足して bogus comment へ行き、comment `-` になる（仕様どおり。h5l test3 `<!-`、test1「Start of a comment」）。 |
| ASCII case-insensitive `DOCTYPE`（144461） | `state.rs:635-644` | 一致 | h5l test1「Correct Doctype mixed case」、WPT doctype01.dat#2 | 残りが 7 byte 未満のときは `markup_declaration_doctype`（`state.rs:688-703`）が読んだ分を buffer に足すので、EOF では bogus comment に正しい文字列が入る（`<!DOC` → comment `DOC`、確認済み）。全一致の分岐（`rest.is_empty()`）は単一 chunk では到達しない。 |
| `[CDATA[`、adjusted current node があり HTML 名前空間でない（144464）→ CDATA section state | `state.rs:645-658`（`current_ns` は `tree/mod.rs:486`、`tree/mod.rs:332-337` の adjusted current node） | 一致 | WPT tests21.dat#1、#2、html5test-com.dat#18 | 大文字小文字を区別する比較（`==`）で、仕様どおり。`NS__UNDEF` は adjusted current node がない場合。`<svg><![cdata[a]]>` は bogus comment（確認済み）。 |
| `[CDATA[`、それ以外 → cdata-in-html-content、comment `[CDATA[`、bogus comment | `state.rs:659-661` | 一致 | h5l domjs「CDATA in HTML content」、WPT tests21.dat#3、tests10.dat#2 | `[CDATA[` を消費せずに bogus comment へ移り、bogus comment が同じ 7 文字を読み直す。結果の comment は同じ。 |
| Anything else（144472）：incorrectly-opened-comment、空の comment token、消費せずに bogus comment | `state.rs:664-670` | 一致（下の二つの場合を除く） | h5l test3 `<!`、WPT tests1.dat#42、doctype01.dat の `<!DOCTYPE` 以外、`<!x` 系 | `<!x` → comment `x`、`<!doc>` → comment `doc`、`<![x` → comment `[x`（確認済み）。 |
| Anything else：入力が `[CDATA[` の真の接頭辞（`<![`、`<![C`…`<![CDATA`、大文字小文字を問わず）で終わる | `state.rs:645-650` → `markup_declaration_cdata` `state.rs:707-728`（726 行） | **逸脱（D1）** | 未カバー | 読んだ `[CDA…` を buffer に入れずに持ち越すため、EOF で空の comment になる。仕様は bogus comment がそれらの文字を読み comment `[CDA…` になる。§逸脱と未実装 D1。 |
| Anything else：直前に文字データがあり、`<!` の直後で入力が終わる | `state.rs:157-160`（tag open の `!`）、`mod.rs:518-527`（`emit_text_not_empty`）、`state.rs:622-624` | **逸脱（D2）** | 未カバー | 文字 token を出した後の buffer が消されず、EOF パスでは `token_set_begin` も呼ばれないので、comment に直前の文字データが入る（`x<!` → comment `x`）。仕様は comment `""`。§逸脱と未実装 D2。 |

## 2. Comment states（spec 144481–144700）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| Comment start：`-` → comment start dash | `state_comment.rs:20-23` | 一致 | h5l test1「Short comment two」`<!--->`、test2「Comment with dash」 | |
| Comment start：`>` → abrupt-closing-of-empty-comment、data、emit | `state_comment.rs:24-30` | 一致 | h5l test1「Short comment」`<!-->`、WPT comments01.dat#11 | |
| Comment start：Anything else（EOF を含む）→ comment state で再消費 | `state_comment.rs:31` | 一致 | h5l test3 `<!--`（EOF）、test1 の通常コメント | 仕様にこの状態の EOF 分岐はなく、comment state の EOF に回る。実装も同じ。 |
| Comment start dash：`-` → comment end | `state_comment.rs:49-51` | 一致 | h5l test1「Comment, Central dash no space」`<!----->` 他 | |
| Comment start dash：`>` → abrupt-closing、emit | `state_comment.rs:52-57` | 一致 | h5l test1「Short comment two」、WPT comments01.dat#10 | |
| Comment start dash：EOF → eof-in-comment、emit | `state_comment.rs:58-60`、`38-44` | 一致 | h5l test3 `<!---` | |
| Comment start dash：Anything else → `-` を足し comment で再消費 | `state_comment.rs:61-63` | 一致 | h5l test2「Comment with dash」`<!---x`、test3 `<!---\u0000` | |
| Comment：`<` → `<` を足し comment less-than sign | `state_comment.rs:71-76` | 一致 | h5l test1「< in comment」 | `append_data(data+1)` で `<` を含めて足す。 |
| Comment：`-` → comment end dash | `state_comment.rs:77-82` | 一致 | h5l test1「Comment, two central dashes」 | |
| Comment：U+0000 → unexpected-null-character、U+FFFD を足す | `state_comment.rs:84-99` | 一致 | h5l test3 `<!--\u0000`、`<!-- \u0000` | `<!--a\0b-->` → `a�b`（確認済み）。 |
| Comment：EOF → eof-in-comment、emit | `state_comment.rs:86-95` | 一致 | h5l test3 `<!-- ` | |
| Comment：Anything else → 足す | `state_comment.rs:100`、`104` | 一致 | h5l test1 通常コメント | CR/CRLF は `cr_case!` で LF（`<!--a\r\nb-->` → `a\nb`、確認済み）。 |
| Comment less-than sign：`!` → 足して bang | `state_comment.rs:111-115` | 一致 | h5l test1「<! in comment」 | |
| Comment less-than sign：`<` → 足す | `state_comment.rs:116-119` | 一致 | h5l test1「<< in comment」、「Nested comment with extra <」 | |
| Comment less-than sign：Anything else → comment で再消費 | `state_comment.rs:120-123` | 一致 | h5l test1「< in comment」、test3 `<!-- <`（EOF） | |
| Comment less-than sign bang：`-` → bang dash | `state_comment.rs:129-132` | 一致 | h5l test1「<!- in comment」 | |
| Comment less-than sign bang：Anything else → comment で再消費 | `state_comment.rs:133-134` | 一致 | h5l test1「<! in comment」 | |
| Comment less-than sign bang dash：`-` → bang dash dash | `state_comment.rs:139-142` | 一致 | h5l test1「Nested comment」 | |
| Comment less-than sign bang dash：Anything else → comment end dash で再消費 | `state_comment.rs:143-144` | 一致 | h5l test1「<!- in comment」 | |
| Comment less-than sign bang dash dash：`>`、EOF → comment end で再消費 | `state_comment.rs:154-157` | 一致 | h5l test1「Comment, central less-than bang」`<!--<!-->`、「Unfinished comment after start of nested comment」`<!-- <!--` | |
| Comment less-than sign bang dash dash：Anything else → nested-comment、comment end で再消費 | `state_comment.rs:158-160` | 一致 | h5l test1「Nested comment」 | |
| Comment end dash：`-` → comment end | `state_comment.rs:166-168` | 一致 | h5l test1「Comment, two central dashes」 | |
| Comment end dash：EOF → emit | `state_comment.rs:169-171` | 一致 | h5l test3 `<!-- -` | |
| Comment end dash：Anything else → `-` を足し comment で再消費 | `state_comment.rs:172-174` | 一致 | h5l test3 `<!-- -\u0009` 他 | |
| Comment end：`>` → data、emit | `state_comment.rs:180-184` | 一致 | h5l test1 通常コメント | |
| Comment end：`!` → comment end bang | `state_comment.rs:185-187` | 一致 | h5l test3 `<!----!` | |
| Comment end：`-` → `-` を足す | `state_comment.rs:188-190` | 一致 | h5l test1「Comment, Central dash no space」、WPT comments01.dat#15 | |
| Comment end：EOF → emit | `state_comment.rs:191-193` | 一致 | h5l test3 `<!-- --` | |
| Comment end：Anything else → `--` を足し comment で再消費 | `state_comment.rs:194-196` | 一致 | h5l test1「Comment, two central dashes」、WPT comments01.dat#5 | |
| Comment end bang：`-` → `--!` を足し comment end dash | `state_comment.rs:202-205` | 一致 | h5l test3 `<!----!-`、`<!----!-->` | |
| Comment end bang：`>` → incorrectly-closed-comment、emit | `state_comment.rs:206-211` | 一致 | h5l test3 `<!----!>`、WPT comments01.dat#2 | |
| Comment end bang：EOF → emit | `state_comment.rs:212-214` | 一致 | h5l test3 `<!----!`、WPT comments01.dat#4 | |
| Comment end bang：Anything else → `--!` を足し comment で再消費 | `state_comment.rs:215-217` | 一致 | h5l test3 `<!----! >`、domjs「--!NUL in comment」 | |

## 3. DOCTYPE states（spec 144702–145303）

DOCTYPE token は Lexbor と同じく属性の列で表される（名前、`PUBLIC`/`SYSTEM` の属性とその値、名前のない system identifier 属性。`state_doctype.rs:1-6`）。「空文字列（missing ではない）」は値が `Some((s, s))` の属性、missing は属性なしまたは値 `None` で表す。名前の ASCII 小文字化は名前の intern（`mod.rs:354-369` → `names.rs:278`）で行う（`<!DOCTYPE ÄBC>` → `Äbc`、確認済み）。

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| DOCTYPE：空白 → before DOCTYPE name | `state_doctype.rs:31` | 一致 | h5l test1「Correct Doctype lowercase」 | |
| DOCTYPE：`>` → before DOCTYPE name で再消費 | `state_doctype.rs:32` | 一致 | h5l test2「DOCTYPE without name」`<!DOCTYPE>`、WPT doctype01.dat#4 | |
| DOCTYPE：EOF → eof-in-doctype、force-quirks、emit | `state_doctype.rs:34-36`、`20-26` | 一致 | h5l test3 `<!DOCTYPE` | 名前なし（DOM では `""`）、quirks（確認済み）。 |
| DOCTYPE：Anything else → missing-whitespace-before-doctype-name、再消費 | `state_doctype.rs:37`、`40-41` | 一致 | h5l test2「DOCTYPE without space before name」、WPT doctype01.dat#3 | |
| Before DOCTYPE name：空白 → 無視 | `state_doctype.rs:48` | 一致 | WPT doctype01.dat#12 | |
| Before DOCTYPE name：ASCII upper alpha → 小文字で名前を始める | `state_doctype.rs:68-73`（小文字化は `set_name`） | 一致 | h5l test1「Correct Doctype uppercase」 | 小文字化を名前の確定時にまとめて行う。結果は同じ。 |
| Before DOCTYPE name：U+0000 → 名前 U+FFFD | `state_doctype.rs:49-60` | 一致 | h5l test3 `<!DOCTYPE \u0000`、`<!DOCTYPE\u0000` | |
| Before DOCTYPE name：`>` → missing-doctype-name、force-quirks、emit | `state_doctype.rs:61-67` | 一致 | WPT doctype01.dat#5、h5l test3 `<!DOCTYPE>` | |
| Before DOCTYPE name：EOF → force-quirks、emit | `state_doctype.rs:50-52` | 一致 | h5l test3 `<!DOCTYPE ` | |
| Before DOCTYPE name：Anything else → 名前を始める | `state_doctype.rs:68-73` | 一致 | h5l test1「Correct Doctype lowercase」、test3 `<!DOCTYPE 􀀀` | |
| DOCTYPE name：空白 → after DOCTYPE name | `state_doctype.rs:85-91` | 一致 | h5l test3 `<!DOCTYPE a ` | |
| DOCTYPE name：`>` → data、emit | `state_doctype.rs:92-99` | 一致 | h5l test1「Correct Doctype lowercase」 | |
| DOCTYPE name：ASCII upper alpha → 小文字を足す | `state_doctype.rs:116`、`87`（`set_name`） | 一致 | h5l test1「Correct Doctype mixed case」 | |
| DOCTYPE name：U+0000 → U+FFFD を足す | `state_doctype.rs:100-115` | 一致 | h5l test3 `<!DOCTYPE a\u0000` | |
| DOCTYPE name：EOF → force-quirks、emit | `state_doctype.rs:102-110` | 一致 | h5l test1「Correct Doctype case with EOF」 | |
| DOCTYPE name：Anything else → 足す | `state_doctype.rs:116`、`120` | 一致 | h5l test1 | |
| After DOCTYPE name：空白 → 無視 | `state_doctype.rs:142` | 一致 | h5l test3 `<!DOCTYPE a  ` | |
| After DOCTYPE name：`>` → emit | `state_doctype.rs:143-147` | 一致 | WPT doctype01.dat#7 | |
| After DOCTYPE name：EOF → force-quirks、emit | `state_doctype.rs:149-151` | 一致 | h5l test2「Incomplete doctype」`<!DOCTYPE html ` | |
| After DOCTYPE name：`PUBLIC`（case-insensitive）→ after DOCTYPE public keyword | `state_doctype.rs:165-170` | 一致 | WPT doctype01.dat#19（`pUbLIc`）、h5l test4「Doctype public case-sensitivity (1)」 | 残りが 6 byte 未満のときは `after_name_keyword`（`state_doctype.rs:188-210`）。単一 chunk では全一致の枝（`pos + 1` を返す、206 行）は到達しない。EOF では不一致となり bogus へ（`<!DOCTYPE html PUBL` → quirks、確認済み）。 |
| After DOCTYPE name：`SYSTEM` → after DOCTYPE system keyword | `state_doctype.rs:171-177` | 一致 | WPT doctype01.dat#10（`sYstEM`）、h5l test2「DOCTYPE with systemId」 | 同上。 |
| After DOCTYPE name：Otherwise → invalid-character-sequence-after-doctype-name、force-quirks、bogus で再消費 | `state_doctype.rs:178`、`130-136` | 一致 | h5l test4「Doctype html x>text」、WPT doctype01.dat#8、#21 | 作りかけの属性を削除する（`attr_delete_last`）。 |
| After DOCTYPE public keyword：空白 → before public identifier | `state_doctype.rs:237-240` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC\u0009` | |
| After DOCTYPE public keyword：`"` → missing-whitespace…、public id = ""、double-quoted | `state_doctype.rs:241-245` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC"` | 値の開始は `identifier_quoted` が設定（`state_doctype.rs:392-394`）。 |
| After DOCTYPE public keyword：`'` → 同上、single-quoted | `state_doctype.rs:246-250` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC'`、WPT doctype01.dat#37 | |
| After DOCTYPE public keyword：`>` → missing-doctype-public-identifier、force-quirks、emit | `state_doctype.rs:251-257` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC>`、WPT doctype01.dat#19 | |
| After DOCTYPE public keyword：EOF → force-quirks、emit | `state_doctype.rs:259-265` | 一致 | h5l test2「DOCTYPE with EOF after PUBLIC」 | |
| After DOCTYPE public keyword：Anything else → missing-quote…、force-quirks、bogus | `state_doctype.rs:266-269` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC\u0008`、WPT doctype01.dat#21 | |
| Before DOCTYPE public identifier：空白 → 無視 | `state_doctype.rs:323` | 一致 | WPT doctype01.dat#32 | |
| Before DOCTYPE public identifier：`"` → public id = ""、double-quoted | `state_doctype.rs:324-327` | 一致 | h5l test2「DOCTYPE with publicId」 | `PUBLIC ""` は public id `""`（missing でない）。no-quirks（確認済み）。 |
| Before DOCTYPE public identifier：`'` → single-quoted | `state_doctype.rs:328-331` | 一致 | h5l test2「DOCTYPE with EOF after PUBLIC '」 | |
| Before DOCTYPE public identifier：`>` → force-quirks、emit | `state_doctype.rs:332-338` | 一致 | WPT doctype01.dat#20 | |
| Before DOCTYPE public identifier：EOF → force-quirks、emit | `state_doctype.rs:340-346` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC ` | |
| Before DOCTYPE public identifier：Anything else → force-quirks、bogus | `state_doctype.rs:347-350` | 一致 | WPT doctype01.dat#22 | |
| DOCTYPE public identifier (double-quoted)：`"` → after public identifier | `state_doctype.rs:398-404`、`444-457` | 一致 | h5l test2「DOCTYPE with publicId」 | |
| 〃：U+0000 → U+FFFD | `state_doctype.rs:416-434` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC"\u0000` | |
| 〃：`>` → abrupt-doctype-public-identifier、force-quirks、emit | `state_doctype.rs:405-414` | 一致 | h5l test2「DOCTYPE with > in double-quoted publicId」 | |
| 〃：EOF → force-quirks、emit | `state_doctype.rs:418-429` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC"` | |
| 〃：Anything else → 足す | `state_doctype.rs:435`、`439` | 一致 | h5l test2「DOCTYPE with publicId」 | CR は LF に正規化。 |
| DOCTYPE public identifier (single-quoted)：`'` | `state_doctype.rs:398-404`、`460-473` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC''` 系 | 本体は double-quoted と同じ関数（`identifier_quoted::<0x27>`）。 |
| 〃：U+0000 | 同上 | 一致 | h5l test3 `<!DOCTYPE a PUBLIC'\u0000` | |
| 〃：`>` | 同上 | 一致 | h5l test2「DOCTYPE with > in single-quoted publicId」 | |
| 〃：EOF | 同上 | 一致 | h5l test2「DOCTYPE with EOF after PUBLIC 'x」 | |
| 〃：Anything else | 同上 | 一致 | h5l test2「DOCTYPE with EOF after PUBLIC 'x」 | |
| After DOCTYPE public identifier：空白 → between | `state_doctype.rs:512-517`、`548-550` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC''\u0009` | |
| 〃：`>` → emit（force-quirks なし） | `state_doctype.rs:518-522` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC''>`、WPT quirks01.dat の `PUBLIC "html">` | |
| 〃：`"` → missing-whitespace-between…、system id = ""、double-quoted | `state_doctype.rs:523-534` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC''"`、WPT doctype01.dat#34 | |
| 〃：`'` → 同上、single-quoted | 同上 | 一致 | h5l test3 `<!DOCTYPE a PUBLIC'''`、WPT doctype01.dat#36、#37 | |
| 〃：EOF → force-quirks、emit | `state_doctype.rs:536-538` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC""` | |
| 〃：Anything else → missing-quote-before-doctype-system-identifier、force-quirks、bogus | `state_doctype.rs:539-542` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC""\u0000` | |
| Between public and system identifiers：空白 → 無視 | `state_doctype.rs:512-517`（`BETWEEN`） | 一致 | WPT doctype01.dat#27、quirks01.dat | |
| 〃：`>` → emit | `state_doctype.rs:518-522` | 一致 | WPT doctype01.dat#25 | |
| 〃：`"` → system id = ""、double-quoted（エラーなし） | `state_doctype.rs:523-534` | 一致 | h5l test2「DOCTYPE with publicId and systemId」 | |
| 〃：`'` | 同上 | 一致 | h5l test2「DOCTYPE with > in single-quoted systemId」 | |
| 〃：EOF | `state_doctype.rs:536-538` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC''\u0009` | |
| 〃：Anything else → force-quirks、bogus | `state_doctype.rs:539-542` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC"" \u0000`、WPT doctype01.dat#26 | |
| After DOCTYPE system keyword：空白 → before system identifier | `state_doctype.rs:237-240`、`292-306` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM\u0009` | 本体は public keyword と同じ関数（`after_keyword`）。 |
| 〃：`"` → missing-whitespace-after-doctype-system-keyword、system id = ""、double-quoted | `state_doctype.rs:241-245` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM"` | |
| 〃：`'` | `state_doctype.rs:246-250` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM'` | |
| 〃：`>` → missing-doctype-system-identifier、force-quirks、emit | `state_doctype.rs:251-257` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM>`、WPT doctype01.dat#10 | |
| 〃：EOF | `state_doctype.rs:259-265` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM` | |
| 〃：Anything else → force-quirks、bogus | `state_doctype.rs:266-269` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM\u0000`、WPT doctype01.dat#17 | |
| Before DOCTYPE system identifier：空白 → 無視 | `state_doctype.rs:323`、`369-379` | 一致 | WPT doctype01.dat#11 | |
| 〃：`"` | `state_doctype.rs:324-327` | 一致 | h5l test2「DOCTYPE with systemId」 | |
| 〃：`'` | `state_doctype.rs:328-331` | 一致 | h5l test2「DOCTYPE with single-quoted systemId」 | |
| 〃：`>` → force-quirks、emit | `state_doctype.rs:332-338` | 一致 | WPT doctype01.dat#11 | |
| 〃：EOF | `state_doctype.rs:340-346` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM\u0009` | |
| 〃：Anything else → force-quirks、bogus | `state_doctype.rs:347-350` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM \u0000`、WPT doctype01.dat#12、#13 | |
| DOCTYPE system identifier (double-quoted)：`"` | `state_doctype.rs:398-404`、`476-489` | 一致 | h5l test2「DOCTYPE with systemId」 | |
| 〃：U+0000 | `state_doctype.rs:416-434` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM"\u0000` | |
| 〃：`>` → abrupt-doctype-system-identifier、force-quirks | `state_doctype.rs:405-414` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM">`、test2「DOCTYPE with > in double-quoted systemId」 | |
| 〃：EOF | `state_doctype.rs:418-429` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM"` | |
| 〃：Anything else | `state_doctype.rs:435`、`439` | 一致 | WPT doctype01.dat#16 | |
| DOCTYPE system identifier (single-quoted)：`'` | `state_doctype.rs:492-505` | 一致 | h5l test2「DOCTYPE with single-quoted systemId」 | |
| 〃：U+0000 | 同上 | 一致 | h5l test3 `<!DOCTYPE a SYSTEM'\u0000` | |
| 〃：`>` | 同上 | 一致 | h5l test3 `<!DOCTYPE a SYSTEM'>` | |
| 〃：EOF | 同上 | 一致 | h5l test3 `<!DOCTYPE a SYSTEM'` | |
| 〃：Anything else | 同上 | 一致 | WPT doctype01.dat#14 | |
| After DOCTYPE system identifier：空白 → 無視 | `state_doctype.rs:564` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM''\u0009` | |
| 〃：`>` → emit | `state_doctype.rs:565-569` | 一致 | h5l test2「DOCTYPE with systemId」、WPT doctype01.dat#15 | |
| 〃：EOF → force-quirks、emit | `state_doctype.rs:571-573` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM''` | |
| 〃：Anything else → unexpected-character-after-doctype-system-identifier、bogus（force-quirks を立てない） | `state_doctype.rs:574-576` | 一致 | h5l test3 `<!DOCTYPE a SYSTEM''\u0000`、`<!DOCTYPE a SYSTEM'' x\u0000` | `<!DOCTYPE html SYSTEM "a" x>` は no-quirks（確認済み）。 |
| Bogus DOCTYPE：`>` → emit | `state_doctype.rs:585-589` | 一致 | h5l test4「Doctype html x>text」 | |
| Bogus DOCTYPE：U+0000 → unexpected-null-character、無視 | `state_doctype.rs:590-596` | 一致 | h5l test3 `<!DOCTYPE a PUBLIC\u0000`（after public keyword から再消費された U+0000） | |
| Bogus DOCTYPE：EOF → emit（force-quirks は変えない） | `state_doctype.rs:591-594` | 一致 | h5l test3 `<!DOCTYPE a \u0008` | |
| Bogus DOCTYPE：Anything else → 無視 | `state_doctype.rs:597` | 一致 | h5l test3 `<!DOCTYPE a a>`、WPT doctype01.dat#8 | |

## 4. CDATA section states（spec 145305–145364）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| CDATA section：`]` → bracket | `state.rs:748-753` | 一致 | WPT tests21.dat#1、h5l domjs「CDATA content」（CDATA section state） | |
| CDATA section：EOF → eof-in-cdata、EOF token | `state.rs:755-766` | 一致 | WPT tests21.dat#4、#5、h5l domjs「CDATA without end marker」 | それまでの文字は文字 token として出す（仕様では一文字ずつ emit 済み）。 |
| CDATA section：Anything else → 文字 token | `state.rs:775`、`779` | 一致 | WPT tests21.dat#1、h5l domjs「CDATA content」 | U+0000 はそのまま（`null_count` を数えるだけ）で、仕様の注記どおり foreign content の tree construction が U+FFFD にする（WPT plain-text-unsafe.dat#11、#35、h5l domjs「NUL in CDATA section」）。CR は LF に正規化（WPT domjs-unsafe.dat#2、#3）。 |
| CDATA section bracket：`]` → end | `state.rs:785-788` | 一致 | WPT tests21.dat#6 | |
| CDATA section bracket：Anything else（EOF を含む）→ `]` を出し CDATA section で再消費 | `state.rs:789-791` | 一致 | WPT tests21.dat#9、#10、h5l domjs「CDATA with single bracket ending」 | |
| CDATA section end：`]` → `]` を出す | `state.rs:797-799` | 一致 | WPT tests21.dat#11–#13、h5l domjs「CDATA with extra bracket」 | |
| CDATA section end：`>` → data | `state.rs:800-805` | 一致 | WPT tests21.dat#1 | |
| CDATA section end：Anything else（EOF を含む）→ `]]` を出し再消費 | `state.rs:806-808` | 一致 | WPT tests21.dat#7、#8、h5l domjs「CDATA with two brackets ending」 | |

## 5. Processing instruction states（spec 145366–145511）

入口は tag open state の `?`（spec 143368-143369、`state.rs:161-165`）。temporary buffer を空にし（`token_set_begin` が buffer を空にする）、PI open へ移る。PI token の target と data は一つの buffer に続けて置き、target の長さを `Token::null_count` に持つ（`mod.rs:83-85`、`state.rs:847-849`）。

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 「convert the temporary buffer to a comment」（143203）：comment = `?` + buffer | `state.rs:965-974` | 一致 | WPT processing-instructions.dat#66–#71、#72–#100、comments01.dat#12–#14 | buffer の先頭に `?` を差し込む。 |
| PI open：ASCII alpha、`_` → target で再消費 | `state.rs:814-818` | 一致 | h5l test2 `<?namespace>`、WPT processing-instructions.dat#52（`_prefix`）、#58 | |
| PI open：EOF → eof-in-processing-instruction、EOF token（何も出さない） | `state.rs:819-823` | 一致 | h5l test3 `<?`、WPT processing-instructions.dat#106 | `a<?` → 文字 `a` だけ（確認済み）。直前の文字データは tag open で出し済み。 |
| PI open：Anything else → invalid-first-character…、comment `?`、bogus で再消費 | `state.rs:824-828` | 一致 | WPT processing-instructions.dat#107（`<? `）、#72–#82、#76、h5l test3 `<?\u0000` | `<?>` → comment `?`、`<??>` → comment `??`（確認済み）。 |
| PI target：空白、`?`、`>`、target が `xml`/`xml-stylesheet`（ASCII case-insensitive）→ disallowed-processing-instruction-target、comment に変換、bogus で再消費 | `state.rs:837-845` | 一致 | WPT processing-instructions.dat#66–#71、comments01.dat#12–#14 | `<?xmlx a?>`、`<?xml-stylesheetx?>` は PI のまま（確認済み）。 |
| PI target：空白、`?`、`>`、それ以外 → PI token（data ""）を作り after target で再消費 | `state.rs:846-850` | 一致 | h5l test2 `<?namespace>`、`<?foo bar?>`、WPT processing-instructions.dat#1、#9、#12（CR） | target は大文字小文字を保つ（`<?XyZ a?>`、確認済み、WPT #42）。 |
| PI target：ASCII alphanumeric、`-`、`_` → buffer に足す | `state.rs:852`、`859`、`869` | 一致 | WPT processing-instructions.dat#18–#65 | 非 ASCII の英数字（`٥`、`é`）は Anything else（WPT #75、#80–#82）。 |
| PI target：EOF → EOF token（何も出さない） | `state.rs:854-858` | 一致 | h5l test3 `<?a`、WPT processing-instructions.dat#101 | |
| PI target：Anything else → invalid-processing-instruction-target、comment に変換、bogus で再消費 | `state.rs:859-865` | 一致 | WPT processing-instructions.dat#83–#100、#6（`a$`）、#89（`:`） | U+0000 もここ（bogus comment で U+FFFD、`<?x\0y?>` → comment `?x�y?`、確認済み）。 |
| After PI target：空白 → 無視 | `state.rs:882` | 一致 | WPT processing-instructions.dat#8、#10 | |
| After PI target：Anything else（EOF を含む）→ data で再消費 | `state.rs:883-886` | 一致 | h5l test2 `<?foo bar?>`、WPT processing-instructions.dat#103（空白の後に EOF） | |
| PI data：`?` → questionable | `state.rs:898-902` | 一致 | h5l test2 `<?foo bar?>`、WPT processing-instructions.dat#7 | |
| PI data：`>` → data、PI token を emit | `state.rs:903-910` | 一致 | WPT processing-instructions.dat#3、#17（`<?t d > ?>`） | |
| PI data：EOF → EOF token（PI は出さない） | `state.rs:927-931` | 一致 | WPT processing-instructions.dat#104、h5l test3 `<?a a` | |
| PI data：Anything else → data に足す | `state.rs:932`、`937` | 一致 | WPT processing-instructions.dat#4 | U+0000 は置換せずそのまま足す（仕様に NULL 分岐がない）：`<?x a\0b?>` → data `a\0b`（確認済み）。CR は LF（`state.rs:911-926`、`<?x a\r\nb\rc?>` → `a\nb\nc`、確認済み）。 |
| PI questionable：`>` → emit | `state.rs:951-957` | 一致 | h5l test2 `<?foo bar?>` | |
| PI questionable：EOF → EOF token | `state.rs:946-950` | 一致 | WPT processing-instructions.dat#102、#105、h5l test3 `<?Z Z?` | |
| PI questionable：Anything else → `?` を足し data で再消費 | `state.rs:958-960` | 一致 | h5l test2 `<?foo bar??>`、WPT processing-instructions.dat#9、#14、#16 | `<?x a?\r?>` → data `a?\n`（確認済み）。 |

## 6. Character reference states（spec 145513–145820）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| Character reference：temporary buffer = `&` | data/RCDATA/属性値の各状態が `&` を buffer に足してから `char_ref`/`char_ref_attr` へ（`state.rs:71-76`、`418-423`、`473-478`、`state_text.rs:49`）。`entity_start` が `&` の位置（`state.rs:1006`、`1011`） | 一致 | — | temporary buffer は token の buffer の末尾で表す。「consumed as part of an attribute」は `is_attribute`（`state.rs:989-998`）。 |
| Character reference：ASCII alphanumeric → named で再消費 | `state.rs:1003-1008` | 一致 | h5l namedEntities.test 全般 | |
| Character reference：`#` → `#` を足し numeric | `state.rs:1009-1014` | 一致 | h5l numericEntities.test 全般 | |
| Character reference：Anything else → flush（`&`）、return state で再消費 | `state.rs:1016-1019` | 一致 | h5l test1「Ampersand EOF」「Ampersand space EOF」「Non-ASCII character reference name」「Unquoted attribute ending in ampersand」、WPT entities01.dat#9、#10、entities02.dat#2–#4 | |
| Named：最長一致で消費（表の第 1 列のどれか） | `state.rs:1023-1060`（`sbst_find`、`res/mod.rs:55-68`） | 一致 | WPT entities01.dat#5、#6（`&notit;`、`&notin;`）、h5l entities.test「Semicolonless named entity 'not' followed by 'i;' in body」 | 探索木を進めるだけ進み、最後に値を持った節点を一致とする。一致の後に進んだ文字（必ず ASCII 英数字）は buffer に文字として残り、return state でそのまま扱われるのと同じ結果になる。入力末尾で途切れたときは EOF パスで続きを読む（`x&noti` → `¬i`、確認済み）。 |
| Named：一致あり、属性中、最後が `;` でなく、次が `=` か ASCII 英数字 → flush して return state（歴史的理由） | `state.rs:1070-1079` | 一致 | h5l entities.test「Valid entity name followed by the equals sign in …」（3 種）、「Undefined named entity in … ending in semicolon and whose name starts with a known entity name」、test1「Entity in attribute without semicolon ending in x」、WPT entities02.dat#5–#9、#14 | 「次の入力文字」は、一致の後に進んだ文字があればその最初、なければ停止位置の文字（`state.rs:1072-1075`）。 |
| Named：一致あり、それ以外 → `;` でなければ missing-semicolon、buffer を置換して flush、return state | `state.rs:1080-1101` | 一致 | h5l namedEntities.test（例「Named entity: AMP without a semi-colon」）、entities.test、WPT entities01.dat#1–#3、entities02.dat#10–#13 | 二つのコードポイントを持つ参照（`&acE;`、`&NotNestedGreaterGreater;`）も正しい（確認済み、h5l domjs「Non BMP-charref in attribute」）。 |
| Named：一致なし → flush、ambiguous ampersand | `state.rs:1062-1065` | 一致 | h5l entities.test「Ambiguous ampersand.」、WPT entities01.dat#7、#8 | |
| Ambiguous ampersand：ASCII alphanumeric → 属性なら値に足し、それ以外は文字 token | `state.rs:1106-1124` | 一致 | h5l entities.test「Ambiguous ampersand.」、WPT entities02.dat#15、#19（属性）、#21（本文） | どちらも buffer に足す（属性では buffer が属性値）。 |
| Ambiguous ampersand：`;` → unknown-named-character-reference、return state で再消費 | `state.rs:1111-1117` | 一致 | h5l entities.test「Ambiguous ampersand.」`&rrrraannddom;` | |
| Ambiguous ampersand：Anything else（EOF を含む）→ return state で再消費 | `state.rs:1108-1117` | 一致 | WPT entities02.dat#15（`_`）、#21 | |
| Numeric：character reference code = 0 | `state.rs:1130` | 一致 | — | |
| Numeric：`x`/`X` → 足して hexadecimal start | `state.rs:1132-1136` | 一致 | WPT entities01.dat#13、#14 | |
| Numeric：ASCII digit → decimal で再消費 | `state.rs:1138-1139`、`1155-1158` | 一致 | WPT entities01.dat#12 | 判定を `char_ref_decimal_start` に移しているが同じ。 |
| Numeric：Anything else → absence-of-digits…、flush（`&#`）、return state で再消費 | `state.rs:1159-1164` | 一致 | WPT entities01.dat#15、#16、h5l test1/test3 `&#` 系 | |
| Hexadecimal start：ASCII hex digit → hexadecimal で再消費 | `state.rs:1143-1144` | 一致 | WPT entities01.dat#13 | |
| Hexadecimal start：Anything else → absence-of-digits…、flush（`&#x`／`&#X`、書かれたとおり）、再消費 | `state.rs:1145-1150` | 一致 | WPT entities01.dat#18、#19、h5l `&#x` EOF 系 | |
| Hexadecimal：ASCII digit／upper hex／lower hex → code × 16 + 値 | `state.rs:1171`、`1182-1185` | 一致 | h5l numericEntities.test（`&#x000b;`、`&#xD800;` 他）、WPT entities01.dat#21 | `to_digit(16)` で三分岐をまとめる。code が 0x10FFFF を超えた後は増やさない（`state.rs:1182`）。仕様に上限はないが、超えた値はどれも end state で U+FFFD になるので結果は同じ（`&#xFFFFFF;`、WPT entities01.dat#69）。 |
| Hexadecimal：`;` → numeric end | `state.rs:1175-1176`、`1180` | 一致 | h5l numericEntities.test 全般 | |
| Hexadecimal：Anything else → missing-semicolon…、numeric end で再消費 | `state.rs:1177-1180` | 一致 | WPT entities01.dat#21、#22 | |
| Decimal：ASCII digit → code × 10 + 値 | `state.rs:1194`、`1205-1207` | 一致 | WPT entities01.dat#12、#70–#75 | 同じく 0x10FFFF を超えたら止める（`&#111111111111` → U+FFFD）。 |
| Decimal：`;` → numeric end | `state.rs:1198-1199`、`1203` | 一致 | WPT entities01.dat#12 | |
| Decimal：Anything else → missing-semicolon…、numeric end で再消費 | `state.rs:1200-1203` | 一致 | WPT entities01.dat#20、h5l entities.test「Decimal numeric entity followed by hex character a.」 | |
| Numeric end：0x00 → null-character-reference、U+FFFD | `state.rs:1239-1241` | 一致 | h5l numericEntities.test（`&#x0000;`）、WPT entities01.dat#23 | |
| Numeric end：> 0x10FFFF → character-reference-outside-unicode-range、U+FFFD | `state.rs:1242-1244` | 一致 | WPT entities01.dat#68、#69、h5l numericEntities.test | |
| Numeric end：surrogate → surrogate-character-reference、U+FFFD | `state.rs:1245-1247` | 一致 | WPT entities01.dat#60–#63、h5l numericEntities.test `&#xd800;`、`&#xdfff;` | |
| Numeric end：noncharacter → noncharacter-character-reference（エラーのみ） | `state.rs:1249-1251`、`1257-1259` | 一致 | h5l numericEntities.test（`&#xfdd0;` 系、`&#xfffe;`）、WPT entities01.dat#65 | U+FDD0–U+FDEF と下位 16 bit が FFFE/FFFF の値。文字はそのまま。 |
| Numeric end：0x0D または ASCII whitespace でない control → control-character-reference、表の置換 | `state.rs:1260-1270`、表 `state.rs:1215-1233` | 一致 | h5l entities.test「Windows-1252 … numeric entity」32×2 件、「CR as numeric entity」、numericEntities.test、WPT entities01.dat#26–#57 | 表の 32 行（0x80–0x9F）を仕様の表（145763–145800）と一行ずつ照合した。0x81、0x8D、0x8F、0x90、0x9D は自分自身。0x0D は CR のまま（`&#13;` → `"\r"`、確認済み）。表の置換は 0x9F 以下のすべての値に `REPLACEMENT[n]` を引くが、0x80 未満は恒等なので同じ。 |
| Numeric end：buffer を code point 一つにして flush、return state | `state.rs:1271-1275` | 一致 | 上の全テスト | 属性値でも同じ（WPT entities02.dat、h5l test1「Hexadecimal entity in attribute」）。 |

## 7. Named character references の表（spec 151920–151973）

| step | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 表の内容（`source` では `<!--BOILERPLATE entities.inc-->`、正規のデータは `/entities.json`） | `html/res/entities.rs`（`ENTITY_VALUES` 2231 件、`ENTITIES` 探索木 9854 節点） | 一致 | `lexbor::res_tests::entities_are_lexbors_tree`（常時）、`entities_are_the_whatwg_list_in_lexbors_tree`（`#[ignore]`、`rake conformance:parser` で実行） | 監査で独立に検証した：`vendor/lexbor/utils/lexbor/html/data/entities.json` の SHA-256 は `spec/conformance/parser_data.rb` の pin（`d741d877…62e6`）と一致し、その 2231 個の名前すべてが `entities.rs` の木で正しい文字列に行き着き、木から列挙できる名前も 2231 個でちょうど一致した（余分なし、不足なし）。 |
| 生成の手順 | `lexbor/res_tests.rs:320-415`（`build_entities`、`generate_entities`）、`lexbor_entities`（`res.h` の読み取り） | 一致 | 同上 | チェックインした `entities.rs` は Lexbor の `res.h` の木から生成され、常時のテストがそれを固定する。WHATWG の entities.json から作った木と Lexbor の木が等しいことは ignore 付きのテストだけが確かめ、オフラインの既定の `cargo test` では走らない（`spec/conformance/data/whatwg/entities.json` はこの checkout にない）。 |
| 探索（`lexbor_sbst_entry_static_find`） | `res/mod.rs:55-68` | 一致 | `lexbor::res_tests::the_generated_tables_answer_like_lexbor`（`&amp;`）、h5l namedEntities.test（4210 件） | |

---

## 逸脱と未実装

未実装はない。逸脱は二つで、どちらも Lexbor（`vendor/lexbor/source/lexbor/html/tokenizer/state.c`）の同じ箇所をそのまま移植したもので、Lexbor の DOM のビルドでも同じ答えになるはずである（Lexbor の C コードで同じ処理を確認した。Lexbor ビルドの実行はしていない）。`docs/parser_differences.ja.md` には載っていない。html5lib の tokenizer テストにも WPT にも、これらを踏む入力はない。

### D1：`[CDATA[` の真の接頭辞で入力が終わると、bogus comment が空になる

- **仕様**（144464-144477）：「If the next few characters are: … "[CDATA[" … Anything else: This is an incorrectly-opened-comment parse error. Create a comment token whose data is the empty string. Switch to the bogus comment state (don't consume anything in the current state).」次の数文字が `[CDATA[` でなければ何も消費せずに bogus comment state へ移り、bogus comment state（144425 以下）がその文字を comment に足し、EOF で emit する。
- **コード**：`ext/makiri/rust/src/html/tokenizer/state.rs:645-650` が、残りが 7 byte 未満のとき `markup_declaration_cdata` へ移る。`state.rs:707-728` はその部分一致の枝（726 行 `t.markup = rest; end`）で、読んだ文字を buffer に足さずに入力の終わりまで進める。EOF パスでは `[0]`（`chunk_slice`、`state.rs:1281-1286`）と比べて不一致になり、bogus comment は EOF から始まるので、comment は空になる。DOCTYPE 側の同じ形の関数（`markup_declaration_doctype`、`state.rs:700`）は `t.append(avail)` で足しているので、`<!DOC` は正しく comment `DOC` になる。比較が `ncasecmp_first`（大文字小文字を区別しない）なので、`<![cd` のような小文字の接頭辞も同じ枝に入る。
- **最小の再現**：

  | 入力 | 仕様の結果 | 実装の結果 |
  |---|---|---|
  | `<![` | `<!--[--><html><head></head><body></body></html>` | `<!----><html>…` |
  | `<![CDA` | comment `[CDA` | comment `""` |
  | `<![cd` | comment `[cd` | comment `""` |
  | `<![CDATA` | comment `[CDATA` | comment `""` |
  | `<svg><![CDA` | `<svg>` の子に comment `[CDA` | `<svg><!----></svg>` |
  | `x<![CD` | `x<!--[CD-->` | `x<!---->` |

  確認：`Makiri::HTML("<![CDA").to_html` は `"<!----><html><head></head><body></body></html>"`。`DocumentFragment.parse("<![CDA", context: "div").to_html` も `"<!---->"`。
- **影響**：入力の末尾が `<![`〜`<![CDATA` のときに、comment の文字が失われる。木の形は変わらない。
- **確信度**：高い（コードを追い、6 通りの入力で再現した）。
- **直し方の候補**：部分一致の枝でも `t.append(avail)` で buffer に足す（DOCTYPE と同じ）。全一致の枝（`state.rs:714-725`）は単一 chunk では到達しない。

### D2：文字データの直後に `<!` で入力が終わると、comment に直前の文字データが入る

- **仕様**：tag open state の `!`（143360 付近）で markup declaration open state へ移る。EOF は「Anything else」（144472-144477）に当たり、データが空の comment token を作って bogus comment state へ移り、bogus comment state の EOF がその空の comment を emit する。直前の文字データは文字 token として既に出ている。
- **コード**：`ext/makiri/rust/src/html/tokenizer/state.rs:157-160`（tag open の `!`）が `emit_text_not_empty`（`mod.rs:518-527`）で文字 token を出すが、この関数は token を `clean` するだけで buffer（`t.buf`）を空にしない（Lexbor の `lxb_html_tokenizer_state_token_emit_text_not_empty_m` も `tkz->pos` を戻さない）。通常は直後の `markup_declaration_open` が `token_set_begin`（`state.rs:622-624`）で buffer を空にするが、`<!` が入力の最後の 2 文字だと markup declaration open は EOF パスで走り、`if !t.is_eof` のため buffer を空にしない。その後の bogus comment（`state.rs:585-618`）は残った buffer をそのまま comment のデータにする。
- **最小の再現**：

  | 入力 | 仕様の結果（body） | 実装の結果（body） |
  |---|---|---|
  | `x<!` | `x<!---->` | `x<!--x-->` |
  | `<p>abc<!` | `<p>abc<!----></p>` | `<p>abc<!--abc--></p>` |
  | `x&amp;<!` | `x&amp;<!---->` | `x&amp;<!--x&-->`（文字参照を解いた後の文字が入る） |

  確認：`Makiri::HTML("x<!").to_html` は `"<html><head></head><body>x<!--x--></body></html>"`。`<p><!`（直前が文字でない）や `x<!-`、`x<!D` は正しい（markup declaration open が EOF パスより前に走り buffer を空にするため）。`x<?` も正しい（tag open の `?` は常に `token_set_begin` を呼ぶ、`state.rs:161-165`）。
- **影響**：入力の末尾がちょうど `<!` で、その前に文字データがあるときに、文字データが本文と comment の両方に現れる。木に余分な文字列が入る。
- **確信度**：高い（コードを追い、3 通りの入力で再現した）。
- **直し方の候補**：`markup_declaration_open` の EOF 分岐（`state.rs:664-667`）で buffer を空にする、または tag open の `!` で `emit_text_not_empty` の後に buffer を空にする。

### 参考：パース・エラーだけの違い（逸脱としない）

- エラーの位置は Lexbor の流儀（`markup_pos` や `t.last`）で、仕様は位置を定めていない。コードの対応は `error.rs` の略号（例 `Eoinprin` = eof-in-processing-instruction、`Diprinta` = disallowed-processing-instruction-target）。この範囲で欠けているエラー・コードは見つからなかった。
- 文字参照の数値は 0x10FFFF を超えたところで増やすのをやめる（`state.rs:1182`、`1205`）。仕様は上限を書かないが、結果とエラーは同じである。

---

## 未カバーの step

分岐の単位では、§1–§6 の「一致」のすべての分岐に html5lib の tokenizer テストか WPT のテストが少なくとも一つ当たった。テストのない部分は、分岐の中の次の場合である（どれも実装を追い、Ruby で確かめて一致とした）。

| 対象 | 内容 | 確認した入力と結果 |
|---|---|---|
| PI data：Anything else の U+0000 | 置換せずに data に入る（仕様に NULL 分岐がない）。WPT の processing-instructions.dat、html5lib のどちらにもない。 | `<?x a\0b?>` → PI target `x`、data `"a\u0000b"` |
| PI data：Anything else の CR/CRLF | `state.rs:911-926` の専用の CR 処理（`crlf` 状態を含む）。WPT #12 は target の中の CR だけ。 | `<?x a\r\nb\rc?>` → data `"a\nb\nc"`、`a<?x\r\n?>b` → PI data `""` |
| PI target：U+0000（EOF でない） | Anything else → comment。`<?\u0000`（PI open）はあるが target の途中はない。 | `<?x\0y?>` → comment `?x�y?` |
| PI questionable：Anything else が CR | `?` の後の CR が LF として data に入る。 | `<?x a?\r?>` → data `"a?\n"` |
| Numeric end：control の表で 0x0C | ASCII whitespace なのでエラーなし、恒等。html5lib に `&#x0C;`/`&#12;` がない（0x01–0x1F と 0x7F のうちこれだけ）。 | 恒等（コードで確認：`state.rs:1260` の除外と `REPLACEMENT[0x0C] = 0x0C`） |
| Numeric end：補助面の noncharacter（U+1FFFE など） | エラーのみ。`&#xFFFE;` と `&#x10FFFE;`（WPT entities01.dat#65）はあるが、U+1FFFE〜U+FFFFF はない。 | 文字はそのまま（`n & 0xFFFE == 0xFFFE`） |
| Markup declaration open：`[CDATA[` を fragment の context（foreign 要素）で判定 | adjusted current node が context 要素になる場合。WPT の fragment テストに `<![CDATA[` はない。 | コードで確認（`tree/fragment.rs:73-79`、`tree/mod.rs:332-337`） |
| Markup declaration open：残り 7 byte 未満で `DOCTYPE` の部分一致（`markup_declaration_doctype`） | `<!DOC` などで入力が終わる場合。WPT/h5l にない（`<!DOCTYPE` ちょうどはある）。 | `<!DOC` → comment `DOC`、`<!DOCTYP` → comment `DOCTYP` |
| After DOCTYPE name：残り 6 byte 未満で `PUBLIC`/`SYSTEM` の部分一致（`after_name_keyword`） | `<!DOCTYPE html PUBL` で入力が終わる場合。 | quirks、名前 `html`、public id なし |
| Named character reference 表の WHATWG との照合 | `entities_are_the_whatwg_list_in_lexbors_tree` は `#[ignore]` で、データの取得が要る。既定の `cargo test` は Lexbor の `res.h` との一致だけを見る。 | 監査で独立に照合済み（§7） |

逸脱の D1、D2 に当たる入力もテストがない（§逸脱と未実装）。

---

## 集計

| 節 | 行数 | 一致 | 意図した差 | 未実装 | 逸脱 |
|---|---|---|---|---|---|
| 1. Markup declaration open | 7 | 5 | 0 | 0 | 2 |
| 2. Comment | 33 | 33 | 0 | 0 | 0 |
| 3. DOCTYPE | 86 | 86 | 0 | 0 | 0 |
| 4. CDATA section | 8 | 8 | 0 | 0 | 0 |
| 5. Processing instruction | 18 | 18 | 0 | 0 | 0 |
| 6. Character reference | 29 | 29 | 0 | 0 | 0 |
| 7. Named character references の表 | 3 | 3 | 0 | 0 | 0 |
| 計 | 184 | 182 | 0 | 0 | 2 |
