# HTML トークナイザ監査（1）：入力の前処理と、data state から bogus comment state まで

- 対象の仕様：HTML Standard `source`（commit `de953a7cf0523f5873a1904ba2ce010c689dfb66`）
  - 「Preprocessing the input stream」（source 142527–142581）
  - 「Tokenization」の総則から「Bogus comment state」まで（source 143116–144448）
- 対象の実装：Makiri の移植版トークナイザ（Lexbor の移植）と、そのトークナイザの状態を切り替える tree builder の箇所
- 既知の差（`docs/parser_differences.ja.md`）は読んだうえで、新しい差としては数えていない。

## 読み方

### 略号

| 略号 | パス（リポジトリ相対） |
|---|---|
| `S` | `ext/makiri/rust/src/html/tokenizer/state.rs` |
| `T` | `ext/makiri/rust/src/html/tokenizer/state_text.rs` |
| `M` | `ext/makiri/rust/src/html/tokenizer/mod.rs` |
| `A` | `ext/makiri/rust/src/html/tree/actions.rs` |
| `F` | `ext/makiri/rust/src/html/tree/fragment.rs` |
| `K` | `ext/makiri/rust/src/html/tree/stacks.rs` |
| `N` | `ext/makiri/rust/src/html/names.rs` |
| `U` | `ext/makiri/rust/src/utf8_input.rs` |

### テストの欄

- `h5:` は html5lib-tests の tokenizer テスト（`spec/conformance/data/html5lib-tests/tokenizer/*.test`）。ファイル名と `description` を書く。このテストはトークナイザだけで動かす（`html/tokenizer/tests.rs` の `html5lib_tokenizer`）。このとき `keep_duplicate` は false で、parse error は比べない（`conformance/tokens.rs` の `html5lib_eq` はトークンだけを比べる）。
- `WPT:` は WPT tree construction（`spec/conformance/data/wpt/html/syntax/parsing/resources/*.dat`）。番号は `spec/conformance/html5lib_runner.rb` と同じ 1 始まりである。
- 各分岐にどのテストが当たるかは、監査のために書いた仕様どおりのトークナイザのモデル（（監査用の一時ファイル、リポジトリには含めない））で調べた。このモデルは監査範囲の状態を仕様どおりに実装し、範囲外の状態（文字参照、コメント、DOCTYPE、PI、CDATA）は近似している。WPT については、tree builder による状態の切り替えを開始タグの名前で近似している（svg と math の中では切り替えない）。結果は `cov.json` と `cov.txt` に置いた。WPT の番号は、この近似で当たると判定したものである。
- 「手動確認」は、この監査で組み込み済みの Ruby 拡張に入力を与えて確かめたことを示す。リポジトリのテストではない。

### 状態の欄

一致、意図した差、未実装、逸脱の四つを使う。parse error の報告については、出力が変わらない限り備考に書くだけにして、状態は変えない。parse error には全体に関わる注意が二つある。

- トークナイザは parse error を `Tokenizer::errors` に記録するが、`Makiri::HTML(...).errors` は HTML では常に空である（手動確認：`Makiri::HTML("<p>\0</x a>").errors` は `[]`）。外から見えないので、どの分岐についても parse error の報告は観測できない。
- 出荷するパーサは `keep_duplicate = true` で動く（`ext/makiri/rust/src/html/tree/parse.rs:72`、`F:76`）。このため duplicate-attribute のエラー（`M:440`）は出荷するパーサでは記録されない。

### 実装の構造（前提）

- **入力は一つの chunk として渡す。** EOF は「input.len() の位置に NUL が一つある」とみなす、もう一度の走査で表す（`M:255-270`、`M:276-278`）。各状態は `t.is_eof` を見て、本物の U+0000 と EOF を区別する。
- **改行の正規化は前処理の段階ではなく、各状態で行う。** テキストを集める状態は `cr_case!`（`S:30-47`）を持ち、CR を LF に変え、直後の LF を読み飛ばす。CR が chunk の最後のバイトなら、`cr` 状態（`S:544-553`）が LF を足す。タグや属性の状態は CR を空白として扱う。
- **文字トークンはまとめて送る。** 文字は buffer に貯め、次の文字以外のトークンを作る直前に、テキストトークン一つとして tree builder に送る（`M:518-527` の `emit_text_not_empty` など）。
- **重複する属性はトークナイザでは捨てない。** 出荷するパーサでは、重複した属性も tree builder に渡す。tree builder が要素を作るときに、同じ名前の属性が要素にすでにあれば飛ばす（`A:185-189`、`A:235-249`）。後の属性を捨てて最初の属性を残す、という結果は仕様と同じになる。
- **値のない属性を区別している。** `<b x>` や `<b x=>` のように値を書かない属性は、トークン上で `value_begin = None` のまま残り、tree builder は `valueless` に記録する（`A:199-202`、`A:224-228`）。後述の **F1** の原因である。

---

## 1. Preprocessing the input stream（source 142527–142581）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| input stream：バイト列を文字に復号したもの（142529） | `U:95-100`（不正な UTF-8 を U+FFFD に置き換える）。ほかのエンコーディングは `bridge/string.rs` の `ruby_to_utf8` が変換する | 一致 | `spec/utf8_sanitize_spec.rb`、`spec/utf8_input_spec.rb` | 復号の詳細（BOM やエンコーディングの判定）はこの監査の範囲外。 |
| surrogate は surrogate-in-input-stream の parse error（142532-142534） | なし（UTF-8 の復号で U+FFFD になるので、surrogate はトークナイザに届かない） | 一致 | h5: unicodeCharsProblematic.test "Invalid Unicode character U+DFFF" など（ランナーは表現できないとして飛ばす：`conformance/json.rs:4-7`） | parse error は記録しない（`ErrorId` に対応する値がない）。UTF-8 と UTF-16 の decoder は lone surrogate を U+FFFD にするので、ストリームに surrogate が現れることはなく、出力は仕様と同じ。 |
| noncharacter は noncharacter-in-input-stream の parse error（142534-142536） | なし（`ErrorId::Noininst` は定義だけで、使われていない） | 一致 | h5: test4.test "U+FDD1 in lookahead region" など 69 件（出力だけを比べる） | エラーを報告しない。出力は変わらない。 |
| ASCII whitespace と U+0000 以外の control は control-character-in-input-stream の parse error（142536-142540） | なし（`ErrorId::Cochininst` は使われていない） | 一致 | h5: test3.test "\u000B" など 119 件（出力だけを比べる） | エラーを報告しない。出力は変わらない。 |
| U+0000 の扱いは後の段階に任せる（note、142542） | 各状態の `0x00` の分岐 | 一致 | 各状態の NUL の行を参照 | |
| newline の正規化：CR LF を LF に、単独の CR を LF にする。data、タグの状態、属性の状態（unquoted）、bogus comment（142549-142554） | `S:30-47` `cr_case!`（data は `S:77`、bogus comment は `S:597`）。タグと属性の状態は `0x0D` を空白として扱う（`S:213`、`S:259`、`S:293`、`S:332`、`S:367`、`S:466`、`S:517`）。chunk の最後の CR は `S:544-553` | 一致 | h5: test4.test "CR followed by non-LF"、"CR at EOF"、"CR LF"。test3.test "<a\u000D>"、"<a \u000D>"、"<a a\u000D>"、"<a a=\u000D>"、"<a a=a\u000D>"、"<a a=''\u000D>"。domjs.test "CR in bogus comment state"、"CRLF in bogus comment state"。WPT: plain-text-unsafe.dat#24、#25、domjs-unsafe.dat#2、#3、processing-instructions.dat#12 | |
| newline の正規化：RCDATA、RAWTEXT、script data（escape の各状態を含む）、PLAINTEXT、属性値（引用符あり） | `T:53`、`T:209`、`T:340`、`S:123`、`S:424`。escape の各状態の「anything else」は、CR を `script_data_escaped` などで読み直してから正規化する | 一致 | 未カバー（html5lib にも WPT にも、これらの状態で CR を読む入力がない） | 手動確認：`<style>a\r\nb\rc\r</style>`、`<script>a\r\nb\rc<!--\r-\r--\r<script\r>\r-\r--\r</script\r>\r\n</script>`、`<plaintext>a\r\nb\rc\r`、`<p a='x\r\ny\rz\r'>`、`<title>a\r`。いずれも LF に正規化された。Lexbor との差分テスト `lexbor::conformance_tests::port_tokenizer_matches_lexbor_on_random_inputs` は CR/LF の組を含む入力を使うが、比べる相手は Lexbor で、仕様ではない。 |
| next input character、current input character、EOF の概念（142557-142577） | `M:241-270`（EOF は仮想の NUL と `is_eof`）、`M:276-278` | 一致 | 全テスト | insertion point と script-created parser は、スクリプトを実行しないので該当しない。 |

## 2. Tokenization の総則（source 143116–143222）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 状態機械は data state から始める（143118） | `M:186-213`（`state: data_before`） | 一致 | 全テスト | |
| tree construction の段階がトークナイザの状態を変える（143124、143146-143150）。対象は title と textarea（RCDATA）、style、xmp、iframe、noembed、noframes、scripting が有効なときの noscript（RAWTEXT）、script（script data）、plaintext（PLAINTEXT） | `A:451-477`（`generic_rawtext_parsing`、`generic_rcdata_parsing`）、`ext/makiri/rust/src/html/tree/modes/in_head.rs:103-104`（script）、`ext/makiri/rust/src/html/tree/modes/in_body.rs:321`（plaintext）と `:709-710`（textarea）、`F:77`（fragment の context、`M:531-564`） | 一致 | h5: contentModelFlags.test 全般（初期状態の指定で）。WPT: tests4.dat#2-#9、plain-text-unsafe.dat#47-#52（fragment の context）、ほか多数 | scripting flag は常に無効（`tree/mod.rs:309`、`F:77` は false を渡す）なので、noscript は data state のまま。scripting を無効にした UA としては仕様どおりで、`#script-on` の WPT はランナーが飛ばす。手動確認：foreign content の `<svg><title>`、`<svg><style>`、`<math><plaintext>` は状態を切り替えない。`<frameset><textarea>` は無視されて状態も変わらない。 |
| DOCTYPE、開始タグ、終了タグ、コメント、文字、EOF の各トークン。開始タグを作るときは self-closing flag をクリアし、属性のリストを空にする（143132-143143） | `M:60-100`（`Token`、`Token::clean`）、`M:500` | 一致 | 全テスト | 属性を作るときの値は F1 を参照。 |
| 送ったトークンはすぐに tree construction が処理する（143145） | `M:469-483`（`emit`）、`ext/makiri/rust/src/html/tree/mod.rs:479-489` | 一致 | 全テスト | 文字トークンはまとめて、遅れて送る。文字トークンの処理がトークナイザの状態を変えることはない。CDATA の判定に使う `current_ns` は、トークンを処理するたびに更新している（`tree/mod.rs:486`）。`<!` を読む前に、それまでの文字を送っている（`S:157-160`）。したがって結果は変わらない。 |
| 作ったトークンが送られずに捨てられることがある（EOF のとき、143152-143155） | `S:236-241`、`S:301-306` など | 一致 | h5: test4.test "EOF in tag name state "、"EOF in attribute name state" など | |
| 開始タグの self-closing flag を確認しなかったら non-void-html-element-start-tag-with-trailing-solidus（143157-143162） | `A:734` | 一致 | h5: test2.test "Non-void element containing trailing /"（トークンだけ） | エラーは記録するが、外からは見えない。 |
| 属性を持つ終了タグは end-tag-with-attributes（143164-143166） | `M:446-450`、`M:491-493` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with space)" など | エラーだけ。 |
| self-closing flag を持つ終了タグは end-tag-with-trailing-solidus（143168-143171） | `M:451-454` | 一致 | h5: test3.test "</a/>" など | エラーだけ。 |
| appropriate end tag token：最後に送った開始タグと同じ名前の終了タグ。開始タグを一つも送っていなければ、どの終了タグも当たらない（143173-143176） | `T:103-106`（`appropriate`：名前を `tag_append_lower` で intern して `tmp_tag_id` と比べる）。fragment では `F:78` が `tmp_tag_id = TAG__UNDEF` にする | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"、"End tag with incorrect name in RCDATA or RAWTEXT"。WPT: tests4.dat#9（script の context で、開始タグがないので `</script>` が当たらない） | `tmp_tag_id` は tree builder が状態を切り替えるときにだけ設定する。RCDATA、RAWTEXT、script data に入るのは開始タグを処理した直後だけなので、「最後に送った開始タグ」と同じになる。手動確認：textarea、style、script を context にした fragment の `a</textarea>b` などは、すべてテキストになる。 |
| 属性の一部として読む文字参照（return state が属性値の状態のとき、143178-143182） | `S:420-421`、`S:475-476`、`S:995-998`（`is_attribute`） | 一致 | h5: entities.test の属性値の各テスト | 文字参照の状態そのものはこの監査の範囲外。 |
| flush code points consumed as a character reference（143184-143190） | `S:1001-1276`（buffer を直接書き換える） | 一致 | h5: namedEntities.test、numericEntities.test | 範囲外の状態が使う定義なので、確認は軽くとどめた。 |
| convert the temporary buffer to a comment（143192-143197） | `S:965-973` | 一致 | h5: domjs.test "CR in bogus comment state"（`<?` の後）、WPT: processing-instructions.dat | PI の状態はこの監査の範囲外。 |
| 各段の前に parser pause flag を確かめる（143199-143202） | なし | 意図した差 | — | Makiri はスクリプトを実行せず、pause が起きない。 |
| （143151 の関連）属性名の状態を出たときの重複の判定：同じ名前の属性がすでにあれば、新しい属性を捨てる（144186-144196） | 出荷するパーサ：`A:185-189`、`A:235-249`（要素を作るときに、同じ名前の属性がすでにあれば飛ばす）。トークナイザだけで動かすとき：`M:388-397`、`M:433-443` | 一致 | h5: test1.test "Repeated attr" など 7 件（`keep_duplicate=false` の経路だけ）。WPT: 該当なし。出荷する経路は未カバー | 手動確認：`<p a=1 A=2 a=3>`、`<svg viewbox=1 viewbox=2 VIEWBOX=3>`、`<math definitionurl=1 definitionURL=2>`、`<svg xlink:href=1 xlink:href=2 xml:lang=a xml:lang=b xmlns:xlink=c xmlns:xlink=d>`、`<html a=1><html a=2 b=1 b=2>` は、どれも最初の属性だけを残した。`<svg xlink:href=1 href=2 xml:lang=a lang=b>` は両方を残した（名前が違うので正しい）。出荷するパーサは duplicate-attribute のエラーを記録しない。 |
| **属性を作るときの値は空文字列**（144134-144140、144227-144229） | `M:388-397`（`TokenAttr::default()` で `value_begin = None`）、`S:376-380`、`A:199-202`、`A:224-228`、`K:300-312` | **逸脱（F1）** | 未カバー | 値を書かない属性を、値が空文字列の属性と区別している。Noah's Ark の比較で結果が変わる。後述の F1 を参照。 |

## 3. Data state（143225-143251）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| U+0026 `&`：return state を data にして、character reference state に移る | `S:71-76` | 一致 | h5: entities.test "Ambiguous ampersand." ほか多数。WPT: entities01.dat#1 | |
| U+003C `<`：tag open state に移る | `S:65-70` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT, switching back to PCDATA"。WPT: adoption01.dat#1 | |
| U+0000：unexpected-null-character。その文字をそのまま文字トークンとして送る | `S:91-98`（NUL は buffer に残し、`null_count` を数える） | 一致 | h5: test2.test "Null Byte Replacement"。WPT: plain-text-unsafe.dat#2 | エラーは `S:98`。 |
| EOF：EOF トークンを送る | `S:79-89`（残りのテキストを送る）、`M:266-268` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: adoption01.dat#1 | |
| anything else：文字トークンを送る | `S:62`（`seek4`）、`S:100` | 一致 | h5: domjs.test "</script> in script HTML comment"。WPT: adoption01.dat#2 | `seek4`（`M:612-634`）は、最初に見つかる位置かそれより前で止まる。誤検出は上位のバイトにしか出ないので、飛ばしすぎることはない。 |

## 4. RCDATA state（143253-143279）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `&`：return state を RCDATA にして、character reference に移る | `T:47-52` | 一致 | h5: contentModelFlags.test "RCDATA w/ an entity"。WPT: tests16.dat#82 | |
| `<`：RCDATA less-than sign に移る | `T:41-46` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: comments01.dat#16 | |
| U+0000：エラー。U+FFFD を送る | `T:54-67` | 一致 | h5: domjs.test "Raw NUL replacement"。WPT: plain-text-unsafe.dat#42、#47（B1 はオラクル同士の差） | |
| EOF：EOF を送る | `T:56-64` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with EOF)"。WPT: plain-text-unsafe.dat#42 | |
| anything else | `T:69` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: comments01.dat#16 | |

## 5. RAWTEXT state（143281-143303）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `<`：RAWTEXT less-than sign に移る | `T:41-46`（`RC=false`） | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: adoption02.dat#2 | `&` の分岐は `RC` のときだけ有効なので、RAWTEXT では `&` を普通の文字として扱う（`T:47`）。 |
| U+0000：エラー。U+FFFD | `T:54-67` | 一致 | h5: domjs.test "Raw NUL replacement"。WPT: plain-text-unsafe.dat#44 | |
| EOF | `T:56-64` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with EOF)"。WPT: plain-text-unsafe.dat#44 | |
| anything else | `T:69` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: html5test-com.dat#23 | |

## 6. Script data state（143305-143327）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `<`：script data less-than sign に移る | `T:203-208` | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: domjs-unsafe.dat#4 | |
| U+0000：エラー。U+FFFD | `T:210-224` | 一致 | h5: domjs.test "Raw NUL replacement"。WPT: domjs-unsafe.dat#4 | |
| EOF | `T:212-220` | 一致 | h5: domjs.test "Raw NUL replacement"。WPT: plain-text-unsafe.dat#46 | |
| anything else | `T:225` | 一致 | h5: domjs.test "leading U+FEFF must pass through"。WPT: domjs-unsafe.dat#4 | |

## 7. PLAINTEXT state（143329-143348）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| U+0000：エラー。U+FFFD | `S:124-137` | 一致 | h5: domjs.test "Raw NUL replacement"。WPT: plain-text-unsafe.dat#10 | |
| EOF | `S:126-133` | 一致 | h5: contentModelFlags.test "PLAINTEXT content model flag"。WPT: foreign-fragment.dat#51 | |
| anything else | `S:138` | 一致 | h5: contentModelFlags.test "PLAINTEXT content model flag"。WPT: plain-text-unsafe.dat#10 | |

## 8. Tag open state（143350-143382）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| U+0021 `!`：markup declaration open に移る | `S:157-160` | 一致 | h5: domjs.test "--!NUL in comment "。WPT: blocks.dat#1 | 移る前に、それまでのテキストを送る。 |
| U+002F `/`：end tag open に移る | `S:154-156` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT, switching back to PCDATA"。WPT: adoption01.dat#1 | |
| ASCII alpha：開始タグを作り、tag name で読み直す | `S:149-153` | 一致 | h5: domjs.test "Non BMP-charref in attribute"。WPT: adoption01.dat#1 | ASCII だけを見る（`M:583-585`）。手動確認：`<ſtyle>` はテキストになる。 |
| U+003F `?`：temporary buffer を空にして、processing instruction open に移る | `S:161-165`（`token_set_begin` が buffer を空にする） | 一致 | h5: domjs.test "CR in bogus comment state"。WPT: comments01.dat#12、processing-instructions.dat | Lexbor の pin（`f441451`）で入った新しい分岐。 |
| EOF：eof-before-tag-name。`<` と EOF を送る | `S:166-172` | 一致 | h5: test3.test "<"、"<<"。WPT: tests1.dat#36 | |
| anything else：invalid-first-character-of-tag-name。`<` を送り、data で読み直す | `S:174-177` | 一致 | h5: test1.test "Empty start tag"。WPT: tests1.dat#37 | 入力中の本物の U+0000 もこの分岐に入る（`is_eof` が false なので）。 |

## 9. End tag open state（143384-143411）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| ASCII alpha：終了タグを作り、tag name で読み直す | `S:183-188` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT, switching back to PCDATA"。WPT: adoption01.dat#1 | |
| `>`：missing-end-tag-name。data state に移る | `S:189-192` | 一致 | h5: test1.test "Empty end tag"。WPT: なし | `data_state` に直接戻り、前後のテキストを一つのトークンにつなぐ。結果は同じ。 |
| EOF：eof-before-tag-name。`<`、`/`、EOF を送る | `S:193-199` | 一致 | h5: test2.test "Unescaped </"。WPT: tests1.dat#38 | |
| anything else：invalid-first-character-of-tag-name。空のコメントを作り、bogus comment で読み直す | `S:201-205` | 一致 | h5: test2.test "Illegal end tag name"。WPT: tests1.dat#39 | |

## 10. Tag name state（143413-143451）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| tab、LF、FF、（CR）、SPACE：before attribute name に移る | `S:213-219` | 一致 | h5: domjs.test "Non BMP-charref in attribute"。WPT: adoption01.dat#10 | |
| `/`：self-closing start tag に移る | `S:220-226` | 一致 | h5: test2.test "Non-void element containing trailing /"。WPT: foreign-fragment.dat#19 | |
| `>`：data state に移り、タグを送る | `S:227-234` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT, switching back to PCDATA"。WPT: adoption01.dat#1 | トークンを送る前に `state = data_before` にしているので、tree builder の切り替えが優先される。 |
| ASCII upper alpha：小文字にして足す | `M:338-350` から `N:266-273`（`tag_append_lower`、ASCII だけを小文字にする） | 一致 | h5: test3.test "</A>"。WPT: entities01.dat#10 | 手動確認：`<aÄB>` は `aÄb` になる。Kelvin 記号の `<linK>` は `link` にならない。 |
| U+0000：エラー。U+FFFD を足す | `S:242-245` | 一致 | h5: test3.test "<a\u0000>"。WPT: なし | 手動確認：`<aB\0c>` は `ab�c` になる。 |
| EOF：eof-in-tag。EOF を送る | `S:236-241` | 一致 | h5: test4.test "EOF in tag name state "。WPT: webkit01.dat#4 | |
| anything else：そのまま足す | `S:247` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT, switching back to PCDATA"。WPT: adoption01.dat#1 | |

## 11. RCDATA less-than sign、end tag open、end tag name（143453-143535）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| less-than sign：`/` → temporary buffer を空にして end tag open に移る | `T:79-81`（buffer の位置は end tag open で `entity_start` に記録） | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: comments01.dat#16 | |
| less-than sign：anything else → `<` を送り、RCDATA で読み直す | `T:83-84`（`<` は `T:42` で足し済み） | 一致 | h5: contentModelFlags.test "End tag not closing RCDATA or RAWTEXT (ending with left-angle-bracket)"。WPT: html5test-com.dat#21 | |
| end tag open：ASCII alpha → 終了タグを作り、end tag name で読み直す | `T:89-92` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: comments01.dat#16 | |
| end tag open：anything else → `</` を送り、RCDATA で読み直す | `T:93-96` | 一致 | h5: domjs.test "bad endtag (space before name)"。WPT: なし | |
| end tag name：空白、appropriate → before attribute name に移る | `T:143-166`（`begin_at_space` は位置だけに影響） | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with space)"。WPT: なし | |
| end tag name：`/`、appropriate → self-closing start tag に移る | `T:159-166` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with slash)"。WPT: なし | |
| end tag name：`>`、appropriate → data state に移り、タグを送る | `T:153-158`（先にテキストを送り、次に終了タグを送る） | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: comments01.dat#16 | |
| end tag name：空白、`/`、`>` で appropriate でない → anything else として扱う | `T:147-150` | 一致 | h5: domjs.test "bad endtag (without close bracket)"、"bad endtag (trailing solidus)"、contentModelFlags.test "End tag with incorrect name in RCDATA or RAWTEXT"。WPT: tests16.dat#83 | 判定のために名前を intern するので、名前の表が大きくなる。出力には影響しない。 |
| end tag name：ASCII upper alpha → 小文字にして tag name に、そのまま temporary buffer に足す | `T:168-174`（文字は進めるだけ。小文字にするのは `T:104` の intern のとき） | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (case-insensitivity)"。WPT: なし | |
| end tag name：ASCII lower alpha | 同上 | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: comments01.dat#16 | |
| end tag name：anything else → `</` と temporary buffer を送り、RCDATA で読み直す | `T:169-173` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with EOF)"。WPT: tests16.dat#83 | 読み直す文字が `&` なら、文字参照として読む（手動確認では扱っていないが、`T:47` に戻る経路をたどって確かめた）。 |

## 12. RAWTEXT less-than sign、end tag open、end tag name（143537-143619）

RCDATA と同じ関数を `RC=false` で使う（`T:78-98`、`T:132-185`）。

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| less-than sign：`/` | `T:79-81` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: adoption02.dat#2 | |
| less-than sign：anything else | `T:83-84` | 一致 | h5: contentModelFlags.test "End tag not closing RCDATA or RAWTEXT (ending with left-angle-bracket)"。WPT: html5test-com.dat#23 | |
| end tag open：ASCII alpha | `T:89-92` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: adoption02.dat#2 | |
| end tag open：anything else | `T:93-96` | 一致 | h5: domjs.test "bad endtag (space before name)"。WPT: tests5.dat#10 | |
| end tag name：空白、appropriate | `T:143-166` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with space)" | |
| end tag name：`/`、appropriate | `T:159-166` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with slash)" | |
| end tag name：`>`、appropriate | `T:153-158` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: adoption02.dat#2 | |
| end tag name：appropriate でない | `T:147-150` | 一致 | h5: domjs.test "bad endtag (without close bracket)"、"bad endtag (trailing solidus)"。WPT: noscript01.dat#10 | |
| end tag name：ASCII upper alpha | `T:168-174` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (case-insensitivity)" | |
| end tag name：ASCII lower alpha | `T:168-174` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT"。WPT: adoption02.dat#2 | |
| end tag name：anything else | `T:169-173` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with EOF)"。WPT: noscript01.dat#10 | |

## 13. Script data：less-than sign、end tag open、end tag name、escape start（143621-143740）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| less-than sign：`/` → temporary buffer を空にして end tag open に移る | `T:236-239` | 一致 | h5: domjs.test "</script> in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#4 | |
| less-than sign：`!` → script data escape start に移り、`<!` を送る | `T:240-243`（`!` は escape start で足す） | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: domjs-unsafe.dat#5 | |
| less-than sign：anything else → `<` を送り、script data で読み直す | `T:244-247` | 一致 | h5: domjs.test "HTML tag in script data"。WPT: scriptdata01.dat#8 | |
| end tag open：ASCII alpha | `T:253-256` | 一致 | h5: domjs.test "</script> in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#4 | |
| end tag open：anything else → `</` を送り、script data で読み直す | `T:257-260` | 一致 | h5: domjs.test "bad endtag (space before name)"。WPT: tests16.dat#4 | |
| end tag name：空白、appropriate | `T:265-267` から `T:143-166` | 一致 | h5: なし。WPT: scriptdata01.dat#3、#7、tests16.dat#10 | |
| end tag name：`/`、appropriate | 同上 | 一致 | h5: なし。WPT: scriptdata01.dat#4、#5、tests16.dat#59 | |
| end tag name：`>`、appropriate | 同上 `T:153-158` | 一致 | h5: domjs.test "</script> in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#4 | |
| end tag name：appropriate でない（空白、`/`、`>`） | `T:147-150` | 一致 | h5: domjs.test "bad endtag (without close bracket)"、"bad endtag (trailing solidus)"、"bad endtag (not matching last start tag)"。WPT: scriptdata01.dat#6 | |
| end tag name：ASCII upper alpha | `T:168-174` | 一致 | h5: domjs.test "lowercase endtags"。WPT: tests16.dat#5 | |
| end tag name：ASCII lower alpha | `T:168-174` | 一致 | h5: domjs.test "</script> in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#4 | |
| end tag name：anything else | `T:169-173` | 一致 | h5: domjs.test "bad endtag (not matching last start tag)"。WPT: domjs-unsafe.dat#22 | |
| escape start：`-` → escape start dash に移り、`-` を送る | `T:271-274`（`-` は次の状態でまとめて足す） | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: domjs-unsafe.dat#5 | |
| escape start：anything else → script data で読み直す | `T:275-277`（`!` を足す） | 一致 | h5: test1.test "<! in script data"。WPT: scriptdata01.dat#9 | |
| escape start dash：`-` → escaped dash dash に移り、`-` を送る | `T:282-286`（`!--` を足す） | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: domjs-unsafe.dat#5 | |
| escape start dash：anything else → script data で読み直す | `T:287-289`（`!-` を足す） | 一致 | h5: test1.test "<!- in script data"。WPT: scriptdata01.dat#10 | |

## 14. Script data escaped、escaped dash、escaped dash dash（143742-143837）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| escaped：`-` → escaped dash に移り、`-` を送る | `T:319-327` | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: domjs-unsafe.dat#7 | |
| escaped：`<` → escaped less-than sign に移る | `T:328-339` | 一致 | h5: domjs.test "</script> in script HTML comment"。WPT: domjs-unsafe.dat#5 | |
| escaped：U+0000 → エラー。U+FFFD | `T:341-349` | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: domjs-unsafe.dat#6 | |
| escaped：EOF → eof-in-script-html-comment-like-text。EOF を送る | `T:343-345`、`T:293-301` | 一致 | h5: domjs.test "EOF in script HTML comment"。WPT: domjs-unsafe.dat#11 | |
| escaped：anything else | `T:350` | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: domjs-unsafe.dat#6 | |
| escaped dash：`-` → escaped dash dash に移り、`-` を送る | `T:366-370` | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: domjs-unsafe.dat#8 | |
| escaped dash：`<` → escaped less-than sign に移る | `T:371-376` | 一致 | h5: domjs.test "Dash less-than in script HTML comment"。WPT: domjs-unsafe.dat#10 | |
| escaped dash：U+0000 → エラー。escaped に移り、U+FFFD を送る | `T:377-385` | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: domjs-unsafe.dat#7 | |
| escaped dash：EOF | `T:378-380` | 一致 | h5: domjs.test "EOF in script HTML comment after dash"。WPT: domjs-unsafe.dat#9 | |
| escaped dash：anything else → escaped に移り、その文字を送る | `T:386-389`（escaped で読み直す。結果は同じ） | 一致 | h5: domjs.test "Dash in script HTML comment"。WPT: tests16.dat#67 | |
| escaped dash dash：`-` → `-` を送る | `T:396-399` | 一致 | h5: domjs.test "Dash at end of script HTML comment"。WPT: scriptdata01.dat#12 | |
| escaped dash dash：`<` → escaped less-than sign | `T:400-405` | 一致 | h5: domjs.test "NUL in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#13 | |
| escaped dash dash：`>` → script data に移り、`>` を送る | `T:406-409`（script data で読み直す） | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: scriptdata01.dat#13 | |
| escaped dash dash：U+0000 → エラー。escaped に移り、U+FFFD | `T:410-413`（escaped で読み直し、`T:341-349` が処理する） | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: domjs-unsafe.dat#5 | |
| escaped dash dash：EOF | `T:410-413` から `T:343-345` | 一致 | h5: domjs.test "EOF in script HTML comment after dash dash"。WPT: tests16.dat#23 | |
| escaped dash dash：anything else | `T:410-413` | 一致 | h5: domjs.test "NUL in script HTML comment"。WPT: domjs-unsafe.dat#6 | |

## 15. Script data escaped less-than sign、escaped end tag open、escaped end tag name（143839-143923）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| less-than sign：`/` → temporary buffer を空にして escaped end tag open に移る | `T:424-427` | 一致 | h5: domjs.test "</script> in script HTML comment"。WPT: domjs-unsafe.dat#5 | |
| less-than sign：ASCII alpha → temporary buffer を空にし、`<` を送り、double escape start で読み直す | `T:428-432`（`entity_start` を `<` の後に置く） | 一致 | h5: domjs.test "NUL in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#11 | |
| less-than sign：anything else → `<` を送り、escaped で読み直す | `T:433-434` | 一致 | h5: domjs.test "Dash less-than in script HTML comment"。WPT: domjs-unsafe.dat#10 | |
| end tag open：ASCII alpha | `T:439-442` | 一致 | h5: domjs.test "</script> in script HTML comment"。WPT: domjs-unsafe.dat#5 | |
| end tag open：anything else → `</` を送り、escaped で読み直す | `T:443-446` | 一致 | h5: test1.test "</ in script HTML comment"。WPT: tests16.dat#27 | |
| end tag name：空白、appropriate | `T:451-453` から `T:143-166` | 一致 | h5: なし。WPT: tests16.dat#29、#46 | |
| end tag name：`/`、appropriate | 同上 | 一致 | h5: なし。WPT: tests16.dat#47、#146 | |
| end tag name：`>`、appropriate | 同上 | 一致 | h5: domjs.test "</script> in script HTML comment"。WPT: domjs-unsafe.dat#5 | |
| end tag name：appropriate でない | `T:147-150`（戻る先は escaped） | 一致 | h5: test1.test "End tag in script HTML comment"（`>`）。WPT: domjs-unsafe.dat#24（`>`）、#25（空白）、#26（`/`） | |
| end tag name：ASCII upper alpha | `T:168-174` | 一致 | h5: なし。WPT: domjs-unsafe.dat#12 | |
| end tag name：ASCII lower alpha | 同上 | 一致 | h5: domjs.test "</script> in script HTML comment"。WPT: domjs-unsafe.dat#5 | |
| end tag name：anything else | `T:169-173` | 一致 | h5: test1.test "End tag in script HTML comment"。WPT: domjs-unsafe.dat#24 | |

## 16. Script data double escape start、double escaped と dash、dash dash、less-than sign、double escape end（143925-144110）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| double escape start：空白、`/`、`>` → temporary buffer が "script" なら double escaped、そうでなければ escaped に移り、その文字を送る | `T:494-502` から `T:469-478`（その文字は移った先で読み直す） | 一致 | h5: domjs.test "NUL in script HTML comment - double escaped"（script）、"Incomplete start tag in script HTML comment double escaped"（それ以外）。WPT: domjs-unsafe.dat#13、#14 | どちらの状態でも、空白、`/`、`>` は普通の文字になる。CR は読み直した先で正規化する。 |
| double escape start：ASCII upper alpha → 小文字にして temporary buffer に足し、文字を送る | `T:479-485`（比べるのは `T:472` の `eq_ignore_ascii_case`） | 一致 | h5: なし。WPT: domjs-unsafe.dat#11、scriptdata01.dat#15 | |
| double escape start：ASCII lower alpha | 同上 | 一致 | h5: domjs.test "NUL in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#13 | |
| double escape start：anything else → escaped で読み直す | `T:480-484` | 一致 | h5: domjs.test "Unclosed start tag in script HTML comment double escaped"。WPT: domjs-unsafe.dat#11 | |
| double escaped：`-` → double escaped dash、`-` を送る | `T:319-327`（`DOUBLE`） | 一致 | h5: domjs.test "NUL in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#16 | |
| double escaped：`<` → double escaped less-than sign、`<` を送る | `T:328-339` | 一致 | h5: domjs.test "</script> in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#14 | |
| double escaped：U+0000 → エラー。U+FFFD | `T:341-349` | 一致 | h5: domjs.test "NUL in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#15 | |
| double escaped：EOF | `T:343-345` | 一致 | h5: domjs.test "EOF in script HTML comment - double escaped"。WPT: tests16.dat#32 | |
| double escaped：anything else | `T:350` | 一致 | h5: domjs.test "</script> in script HTML comment - double escaped with nested <script>"。WPT: scriptdata01.dat#16 | |
| double escaped dash：`-` → dash dash、`-` を送る | `T:512-516` | 一致 | h5: domjs.test "NUL in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#17 | |
| double escaped dash：`<` → double escaped less-than sign、`<` を送る | `T:517-521` | 一致 | h5: test1.test "- in script HTML comment double escaped"。WPT: tests16.dat#51 | |
| double escaped dash：U+0000 → エラー。double escaped に移り、U+FFFD | `T:522-530` | 一致 | h5: domjs.test "NUL in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#16 | |
| double escaped dash：EOF | `T:523-525` | 一致 | h5: domjs.test "EOF in script HTML comment double escaped after dash"。WPT: tests16.dat#49 | |
| double escaped dash：anything else | `T:531-534` | 一致 | h5: test1.test "- spaced in script HTML comment double escaped"。WPT: scriptdata01.dat#17 | |
| double escaped dash dash：`-` | `T:545-548` | 一致 | h5: test1.test "--- in script HTML comment double escaped"。WPT: domjs-unsafe.dat#18 | |
| double escaped dash dash：`<` | `T:549-553` | 一致 | h5: test1.test "-- in script HTML comment double escaped"。WPT: domjs-unsafe.dat#18 | |
| double escaped dash dash：`>` → script data に移り、`>` を送る | `T:554-558` | 一致 | h5: domjs.test "NUL in script HTML comment - double escaped"。WPT: scriptdata01.dat#19 | |
| double escaped dash dash：U+0000 | `T:559-567` | 一致 | h5: domjs.test "NUL in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#17 | |
| double escaped dash dash：EOF | `T:560-562` | 一致 | h5: domjs.test "EOF in script HTML comment double escaped after dash dash"。WPT: tests16.dat#52 | |
| double escaped dash dash：anything else | `T:568-571` | 一致 | h5: test1.test "-- spaced in script HTML comment double escaped"。WPT: scriptdata01.dat#18 | |
| double escaped less-than sign：`/` → temporary buffer を空にし、double escape end に移り、`/` を送る | `T:581-584` と、Lexbor にだけある中間の状態 `T:590-603`（alpha が来れば `/` を足して double escape end に移る。それ以外なら `/` を足して double escaped で読み直す） | 一致 | h5: domjs.test "</script> in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#14 | `</` の後が英字でない場合：仕様では double escape end の空白、`/`、`>`（buffer は空なので "script" ではない）または anything else になり、どちらも double escaped に戻って文字を送る。中間の状態の経路と結果は同じ。 |
| double escaped less-than sign：anything else → double escaped で読み直す | `T:585-586` | 一致 | h5: domjs.test "</script> in script HTML comment - double escaped with nested <script>"。WPT: tests16.dat#33 | |
| double escape end：空白、`/`、`>` → buffer が "script" なら escaped、そうでなければ double escaped に移り、文字を送る | `T:606-614` から `T:469-478` | 一致 | h5: domjs.test "</script> in script HTML comment - double escaped"（script）、"Incomplete end tag in script HTML comment double escaped"（それ以外）。WPT: domjs-unsafe.dat#14、#19 | |
| double escape end：ASCII upper alpha | `T:479-485` | 一致 | h5: なし。WPT: domjs-unsafe.dat#19、#20 | |
| double escape end：ASCII lower alpha | 同上 | 一致 | h5: domjs.test "</script> in script HTML comment - double escaped"。WPT: domjs-unsafe.dat#14 | |
| double escape end：anything else → double escaped で読み直す | `T:480-484` | 一致 | h5: domjs.test "Unclosed end tag in script HTML comment double escaped"。WPT: tests16.dat#35 | |

## 17. Before attribute name state（144112-144143）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白：無視する | `S:259` | 一致 | h5: test3.test "<a \u0009>"。WPT: scriptdata01.dat#5 | |
| `/`、`>`、EOF：after attribute name で読み直す | `S:268-271`、`S:273-276` | 一致 | h5: test3.test "<a />"、test2.test "Start tag with no attributes but space before the greater-than sign"、contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with space)"（EOF）。WPT: tests10.dat#25、scriptdata01.dat#3、tests16.dat#11 | |
| `=`：unexpected-equals-sign-before-attribute-name。名前を `=`、値を空文字列にした属性を作り、attribute name に移る | `S:260-267` | 一致（値は F1 を参照） | h5: test3.test "<a =>"、"<a a=''=>"、"<a/=>"。WPT: なし | |
| anything else：名前と値を空文字列にした属性を作り、attribute name で読み直す | `S:277-280` | 一致（値は F1 を参照） | h5: domjs.test "Non BMP-charref in attribute"。WPT: adoption01.dat#10 | 入力中の本物の U+0000 もここに入り、attribute name で U+FFFD になる。 |

## 18. Attribute name state（144145-144197）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白、`/`、`>`、EOF：after attribute name で読み直す | `S:293-299`（名前を確定する）、`S:301-306`（EOF） | 一致 | h5: test2.test "Empty attribute followed by uppercase attribute"（空白）、test3.test "<a a/>"（`/`）、test1.test "Unquoted attribute ending in ampersand"（`>`）、test4.test "EOF in attribute name state"（EOF）。WPT: tests2.dat#18、tests2.dat#59、foreign-fragment.dat#2、tests2.dat#17 | |
| `=`：before attribute value に移る | `S:312-318` | 一致 | h5: domjs.test "Non BMP-charref in attribute"。WPT: adoption01.dat#10 | |
| ASCII upper alpha：小文字にして足す | `M:354-369` から `N:278-284`（`attr_local_name_append` が ASCII だけを小文字にする） | 一致 | h5: test2.test "Empty attribute followed by uppercase attribute"。WPT: tests1.dat#32 | 手動確認：`Data-Ä` は `data-Ä` になり、`ſtyle` は `style` と別の名前になる。 |
| U+0000：エラー。U+FFFD を足す | `S:307-310` | 一致 | h5: test3.test "<a \u0000>"、"<a a\u0000>"。WPT: なし | 手動確認：`<a d\0E=1>` は `d�e` になる。 |
| `"`、`'`、`<`：unexpected-character-in-attribute-name。anything else として扱う | `S:319` | 一致 | h5: test2.test "Double-quote after attribute name"。WPT: html5test-com.dat#2 | |
| anything else：そのまま足す | `S:320` | 一致 | h5: domjs.test "Non BMP-charref in attribute"。WPT: adoption01.dat#10 | |
| この状態を出るときの重複の判定 | 2 章の該当行を参照 | 一致 | h5: test1.test "Repeated attr"（トークナイザだけの経路） | 出荷する経路は未カバー。 |

## 19. After attribute name state（144199-144231）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白：無視する | `S:332` | 一致 | h5: test2.test "Empty attribute followed by uppercase attribute"。WPT: tests2.dat#18 | |
| `/`：self-closing start tag に移る | `S:333-336` | 一致 | h5: test3.test "<a />"。WPT: tests10.dat#25 | |
| `=`：before attribute value に移る | `S:337-340` | 一致 | h5: test3.test "<a a =>"。WPT: なし | |
| `>`：data state に移り、タグを送る | `S:341-345` | 一致 | h5: test1.test "Unquoted attribute ending in ampersand"。WPT: foreign-fragment.dat#2 | |
| EOF：eof-in-tag。EOF を送る | `S:347-351` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with space)"。WPT: tests16.dat#11 | |
| anything else：名前と値を空文字列にした属性を作り、attribute name で読み直す | `S:352-355` | 一致（値は F1 を参照） | h5: test2.test "Empty attribute followed by uppercase attribute"。WPT: tests2.dat#18 | |

## 20. Before attribute value state（144233-144261）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白：無視する | `S:367` | 一致 | h5: test3.test "<a a=\u0009>"。WPT: なし | |
| `"`：attribute value (double-quoted) に移る | `S:368-371` | 一致 | h5: domjs.test "Non BMP-charref in attribute"。WPT: adoption01.dat#10 | |
| `'`：attribute value (single-quoted) に移る | `S:372-375` | 一致 | h5: entities.test の単一引用符の各テスト。WPT: entities02.dat#3 | |
| `>`：missing-attribute-value。data state に移り、タグを送る | `S:376-380` | 一致（値は F1 を参照） | h5: test3.test "<a a =>"。WPT: なし | この分岐を通った属性は、値を持たない属性として残る（F1 の再現 `<b x=>`）。 |
| anything else（EOF を含む）：attribute value (unquoted) で読み直す | `S:382-385` | 一致 | h5: entities.test の引用符なしの各テスト、test4.test "EOF in before attribute value state"（EOF）。WPT: domjs-unsafe.dat#46 | 仕様にこの状態の EOF の行はなく、anything else から unquoted の EOF に進む。実装も同じ経路をたどる。 |

## 21. Attribute value (double-quoted) と (single-quoted)（144263-144321）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 引用符（`"` または `'`）：after attribute value (quoted) に移る | `S:411-417` | 一致 | h5: domjs.test "Non BMP-charref in attribute"（`"`）、entities.test の単一引用符の各テスト（`'`）。WPT: adoption01.dat#10、entities02.dat#3 | |
| `&`：return state をこの状態にして、character reference に移る | `S:418-423` | 一致 | h5: domjs.test "Non BMP-charref in attribute"、entities.test。WPT: entities02.dat#2、#3 | |
| U+0000：エラー。U+FFFD を足す | `S:425-438`（EOF でないとき `S:435-438`） | 一致 | h5: test3.test "<a a=\"\u0000\">"、"<a a='\u0000'>"。WPT: なし | |
| EOF：eof-in-tag。EOF を送る | `S:426-434` | 一致 | h5: test4.test "EOF in attribute value (double quoted) state"、"EOF in attribute value (single quoted) state"。WPT: webkit02.dat#5（`"` だけ） | |
| anything else：そのまま足す | `S:440` | 一致 | h5: entities.test の各テスト。WPT: adoption01.dat#10、entities02.dat#3 | 値の開始時に `value_begin` を記録する（`S:402-404`）。属性名の後で一度だけ buffer を空にする。`seek4` を使うのは double-quoted だけで、Lexbor と同じ最適化である。 |

## 22. Attribute value (unquoted)（144323-144367）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白：before attribute name に移る | `S:466-472` | 一致 | h5: test1.test "Unquoted attribute ending in ampersand"。WPT: tests10.dat#24 | |
| `&`：return state を unquoted にして、character reference に移る | `S:473-478` | 一致 | h5: entities.test の引用符なしの各テスト。WPT: entities02.dat#4 | |
| `>`：data state に移り、タグを送る | `S:479-486` | 一致 | h5: entities.test の引用符なしの各テスト。WPT: domjs-unsafe.dat#46 | |
| U+0000：エラー。U+FFFD | `S:497-500` | 一致 | h5: test3.test "<a a=\u0000>"。WPT: なし | |
| `"`、`'`、`<`、`=`、`` ` ``：unexpected-character-in-unquoted-attribute-value。anything else として扱う | `S:502-505` | 一致 | h5: entities.test "Entity name requiring semicolon instead followed by the equals sign in an unquoted attribute value."。WPT: html5test-com.dat#3 | エラーの位置にタグの位置（`token.end`）を使っており、文字の位置ではない。軽微。 |
| EOF：eof-in-tag。EOF を送る | `S:488-496` | 一致 | h5: test4.test "EOF in attribute value (unquoted) state"。WPT: なし | |
| anything else | `S:506` | 一致 | h5: test1.test "Start Tag w/attribute no quotes"。WPT: domjs-unsafe.dat#46 | |

## 23. After attribute value (quoted)（144369-144400）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白：before attribute name に移る | `S:517-520` | 一致 | h5: test1.test "Multiple atts"。WPT: isindex.dat#2 | |
| `/`：self-closing start tag に移る | `S:521-524` | 一致 | h5: test2.test "Void element with permitted slash (with attribute)"。WPT: webkit01.dat#46 | |
| `>`：data state に移り、タグを送る | `S:525-529` | 一致 | h5: domjs.test "Non BMP-charref in attribute"。WPT: adoption01.dat#10 | |
| EOF：eof-in-tag。EOF を送る | `S:531-535` | 一致 | h5: test4.test "EOF in after attribute value state"。WPT: なし | |
| anything else：missing-whitespace-between-attributes。before attribute name で読み直す | `S:536-538` | 一致 | h5: test1.test "Multiple atts no space"。WPT: なし | |

## 24. Self-closing start tag state（144402-144423）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `>`：self-closing flag を立て、data state に移り、タグを送る | `S:558-563` | 一致 | h5: test2.test "Non-void element containing trailing /"。WPT: foreign-fragment.dat#19 | |
| EOF：eof-in-tag。EOF を送る | `S:565-569` | 一致 | h5: contentModelFlags.test "End tag closing RCDATA or RAWTEXT (ending with slash)"。WPT: tests16.dat#47 | |
| anything else：unexpected-solidus-in-tag。before attribute name で読み直す | `S:570-572` | 一致 | h5: test2.test "StartTag containing /"。WPT: scriptdata01.dat#5 | |

## 25. Bogus comment state（144425-144447）

| step（状態と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| `>`：data state に移り、コメントを送る | `S:589-596` | 一致 | h5: domjs.test "CDATA in HTML content"。WPT: comments01.dat#12 | |
| EOF：コメントを送り、EOF を送る | `S:600-607` | 一致 | h5: domjs.test "CR in bogus comment state"。WPT: comments01.dat#14 | |
| U+0000：エラー。U+FFFD を足す | `S:608-610` | 一致 | h5: test3.test "<!\u0000"。WPT: plain-text-unsafe.dat#12 | |
| anything else：そのまま足す | `S:612`、CR は `S:597` | 一致 | h5: domjs.test "CR in bogus comment state"。WPT: comments01.dat#12 | 手動確認：`<!a\r\nb\r>` はコメント `a\nb\n` になる。 |

---

## 集計

| 状態 | 件数 |
|---|---|
| 一致 | 197（このうち 4 行は「一致（値は F1 を参照）」：属性を作る分岐そのものは一致し、作った属性の値の表し方が F1 に当たる） |
| 意図した差 | 1（parser pause flag：スクリプトを実行しないので該当しない） |
| 未実装 | 0 |
| 逸脱 | 1（F1） |

一致の行のうち、parse error だけが違うもの（出力は変わらない）は次のとおり。

- 入力ストリームの surrogate、noncharacter、control の parse error を記録しない。
- 出荷するパーサは duplicate-attribute を記録しない（`keep_duplicate = true` で重複の判定を tree builder に任せており、tree builder は黙って飛ばす）。
- unquoted 属性値の unexpected-character の位置がタグの位置になる。
- HTML の `Document#errors` は常に空で、トークナイザが記録した parse error は外から見えない。

---

## 逸脱と未実装

### F1 値を書かない属性を、空文字列の値の属性と区別している（Noah's Ark の比較が変わる）

- **仕様**：
  - before attribute name state の anything else（source 144139-144140）：「Start a new attribute in the current tag token. Set that attribute name and value to the empty string.」同じ文が `=` の分岐（144134-144136、値は空文字列）と after attribute name state の anything else（144227-144229）にある。トークンの上では、`<b x>`、`<b x=>`、`<b x="">` の属性 `x` はどれも値が空文字列で、区別がない。
  - list of active formatting elements への push（source 142941-142950）：「two elements have the same attributes if all their parsed attributes can be paired such that the two attributes in each pair have identical names, namespaces, and values」。
- **コード**：
  - トークナイザは属性を `TokenAttr::default()`（`value_begin = None`、`value = None`）で作る（`M:388-397`）。値の状態に入ったときだけ `value_begin` を設定する（`S:402-404`、`S:460-462`）。before attribute value state の `>`（`S:376-380`）や、`=` のない属性では `None` のまま残る。
  - tree builder は `value_begin` が `None` の属性を `valueless` に記録する（`A:199-202`、`A:224-228`）。設計は `ext/makiri/rust/src/html/tree/mod.rs:25-28` に「Lexbor keeps a NULL value for one (`<b x>`), which the Noah's Ark clause tells apart from an empty one (`<b x="">`)」と書かれている。
  - Noah's Ark の比較 `attr_compare`（`K:300-312`）は、片方だけが `valueless` なら別の属性とみなす。
  - `valueless` を読む箇所はほかにもある（`select.rs:158` の size、`actions.rs:759` の annotation-xml の encoding、複製の `actions.rs:268` と `select.rs:376`）。ただし手動確認では、Noah's Ark の比較のほかに出力が変わるものはなかった（`<svg><font color>`、`<table><input type>`、`<math><annotation-xml encoding>` は仕様どおり）。
- **最小の再現**：`<p><b x><b x><b x><b x=""></p><p>t`
  - **仕様の結果**：四つの `b` は同じ属性（`x=""`）を持つ。四つ目を push するとき一つ目が active formatting elements から外れる。二つ目の `<p>` では三つの `b` を再構築する（`<p><b x=""><b x=""><b x="">t</b></b></b></p>`）。
  - **実装の結果**：二つ目の `<p>` の中に `b` が四つできる（手動確認：`d.css("p")[1].css("b").size` は 4）。

    ```
    <p><b x=""><b x=""><b x=""><b x="">t</b></b></b></b></p>
    ```

  - 同じ結果になる入力：`<p><b x=""><b x=""><b x=""><b x></p><p>t`、`<p><b x=><b x=><b x=><b x=""></p><p>t`（どちらも 4）。比較のために、四つとも `x=""`、または四つとも `x` にすると、どちらも 3 になる（仕様どおり）。
- **テスト**：html5lib と WPT のどちらも当たらない。html5lib の tokenizer ランナーは、値のない属性を `unwrap_or_default()` で `""` にして比べるので、この区別は見えない（`html/tokenizer/tests.rs:100`）。
- **分類の見込み**：Lexbor の振る舞いを移したもので、`docs/parser_differences.ja.md` には載っていない。分類は「参照実装の未対応」（P 項目の扱い）で、移植版で直す候補になる。直すなら `compat.rs` の `fixed` の分岐で `attr_compare` を値だけの比較にし、差分の許可は「入力に、値を書かない属性を持つ formatting element（`a`、`b`、`big`、`code`、`em`、`font`、`i`、`nobr`、`s`、`small`、`strike`、`strong`、`tt`、`u`）があるとき」とするのが自然である。
- **確信度**：高い。コードの経路を追い、Ruby 拡張で三つの入力について確かめた。仕様の文はトークンの属性の値を空文字列と定めており、解釈の余地は小さい。

### 未実装

なし。監査範囲の状態と分岐は、どれも実装されている。

---

## 未カバーの step

状態が一致で、html5lib と WPT のどちらにも当たるテストが見つからなかった step を挙げる。いずれも手動確認か、コードの経路の追跡で一致と判断した。

1. **RCDATA、RAWTEXT、script data（escape の各状態を含む）、PLAINTEXT、属性値（引用符あり）の中の CR と CR LF の正規化**（前処理、142549-142554）。CR を含むテストは、data、タグと属性の状態、bogus comment、PI にしかない。手動確認：1 章の行を参照。
2. **出荷するパーサ（`keep_duplicate = true`）での重複属性の除去**（144186-144196）。html5lib の "Repeated attr" などは、トークナイザだけの経路（`keep_duplicate = false`）でしか動かない。WPT には同じ属性を二度書くタグがない。手動確認：2 章の行を参照。tree builder 側の判定（`A:235-249`）は、名前（qualified name）か、local name と名前空間で比べる。foreign の属性の調整が絡む組み合わせでも、仕様と同じ結果になった。
3. **fragment で開始タグを一つも送っていないときに、RCDATA と RAWTEXT の終了タグが appropriate にならないこと**（143173-143176）。WPT では、script の context の tests4.dat#9（escaped の中の `</script>`）だけが当たる。textarea、title、style の context で同じ名前の終了タグを書いたテストはない。手動確認：`DocumentFragment.parse("a</textarea>b<p>", context: "textarea")` などは、すべてテキストになった。
4. **入力ストリームの surrogate、noncharacter、control の parse error**（142531-142540）。出力についてはテストがあるが、parse error はどのランナーも比べず、実装も記録しない。出力に影響しないので一致とした。
5. **non-void-html-element-start-tag-with-trailing-solidus、end-tag-with-attributes、end-tag-with-trailing-solidus の parse error**（143157-143171）。トークンや木を比べるテストには当たるが、parse error そのものを確かめるテストはない（`html/tokenizer/tests.rs` の traced 版は Lexbor との一致だけを見る）。

（参考）html5lib には当たらず、WPT にだけ当たる分岐：script data end tag name の空白と `/` が appropriate な場合、script data escaped end tag name の空白と `/`（appropriate かどうかの両方）と ASCII upper alpha、double escape start と double escape end の ASCII upper alpha。いずれも WPT の tree construction で当たる（表の番号を参照）。
