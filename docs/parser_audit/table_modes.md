# HTML Standard 監査：text / table 系 / template / after body / frameset 系の insertion mode

- 仕様：whatwg/html `source` @ `de953a7cf0523f5873a1904ba2ce010c689dfb66`（行番号はこのファイルの行）
- 実装：Makiri の移植版 tree builder（Lexbor `05b5d37` の移植、`compat::fixed` は出荷ビルドで常に真）
- 略記：`M/` = `ext/makiri/rust/src/html/tree/modes/`、`T/` = `ext/makiri/rust/src/html/tree/`（いずれもリポジトリ相対）
- テスト：WPT `html/syntax/parsing/resources/*.dat`。`file#N` の N は `spec/conformance/html5lib_runner.rb` と同じ 1 始まりの通し番号。scripting-on のテストは数えていない。
- 確認方法：`Makiri::HTML(...)` / `Makiri::DocumentFragment.parse(..., context:)` の木を出力して照合した（再ビルドなし）。
- 既知の差（`docs/parser_differences.ja.md` の P1〜P3 等）は新規として扱っていない。P2 は "in table" の form に関係するので、該当行で引用した。

状態の凡例：一致／意図した差／未実装／逸脱。「一致（等価）」は、手順の書き方は仕様と異なるが、出力（木）が常に同じになることを追跡で確かめたもの。

---

## 1. "text" insertion mode（仕様 149077〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| A character token：Insert the token's character | `M/text.rs:11-15` | 一致 | tests16.dat#2、tests1.dat#101、tests16.dat#93 | U+0000 はトークナイザが U+FFFD に置き換えるので来ない |
| EOF：Parse error | `M/text.rs:17` | 一致 | tests16.dat#1、#100、tests1.dat#50、tests18.dat#31 | |
| EOF：current node が script なら already started を真に | なし | 意図した差 | — | script を実行しないパーサ（scripting 無効）では観測されない |
| EOF：Pop the current node | `M/text.rs:18` | 一致 | tests16.dat#2 | |
| EOF：original insertion mode に戻して再処理 | `M/text.rs:19-20` | 一致 | tests16.dat#2 | `false` を返すと再処理になる |
| `</script>`（149108）：microtask checkpoint、insertion point の保存、script nesting level、prepare the script element、pending parsing-blocking script の実行ループ | なし | 意図した差 | — | script の実行と document.write は対象外（scripting 無効） |
| `</script>`：Pop the current node、original insertion mode に切り替え | `M/text.rs:22-26` | 一致 | domjs-unsafe.dat#4 ほか（`</script>` を含む 97 件） | 「Any other end tag」と同じ腕で処理する。scripting 無効では結果は同じ |
| Any other end tag（149217）：Pop、original insertion mode に切り替え | `M/text.rs:22-26` | 一致 | tests1.dat#101（`</title>`）、tests16.dat#92（`</textarea>`） | text mode には文字、適切な終了タグ、EOF しか来ないので、`_` の腕は終了タグだけを受ける |

## 2. "in table" insertion mode（仕様 149227〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| A character token、current node が table/tbody/**template**/tfoot/thead/tr（149235）：pending table character tokens を空に、original mode を保存、"in table text" に切り替えて再処理 | `M/in_table.rs:25-43`（判定は 29-33） | **逸脱** | table 等：adoption01.dat#12、tests16.dat#195、template.dat#13、tables01.dat#19。template：未カバー | 要素の列挙に **template がない**（Lexbor `in_table.c` の `text_open` と同じ）。current node が template のときは「Anything else」（foster parenting 付きの "in body"）に行き、空白だけの文字でも active formatting elements を再構築する。下の F1 |
| （上の分岐の対象外の文字）→ Anything else | `M/in_table.rs:42` | 一致 | tests1.dat#33（`<div>` の中の文字はそもそも in body） | |
| A comment token：Insert a comment | `M/in_table.rs:223-228` | 一致 | tests19.dat#26（`<table>  <!--foo-->`、in table text 経由）、tests1.dat#33 | |
| A processing instruction token：Insert a processing instruction | `M/in_table.rs:229-234` | 一致 | processing-instructions.dat#110 | |
| A DOCTYPE token：Parse error、無視 | `M/in_table.rs:235-238` | 一致 | domjs-unsafe.dat#32、tables01.dat#19 | |
| start caption：clear the stack back to a table context | `M/in_table.rs:63` | 一致 | tests6.dat#16、tests2.dat#36 | |
| start caption：marker を挿入 | `M/in_table.rs:64-66` | 一致 | tests10.dat#13 | |
| start caption：要素を挿入、"in caption" へ | `M/in_table.rs:67-71` | 一致 | tests6.dat#16 | |
| start colgroup：clear back、挿入、"in column group" へ | `M/in_table.rs:75-82` | 一致 | tests1.dat#107、domjs-unsafe.dat#35 | |
| start col：clear back、属性なしの colgroup を挿入、"in column group" へ、再処理 | `M/in_table.rs:85-92` | 一致 | tests1.dat#108 | 暗黙の colgroup は `Tok::fake` で、ソース位置を持たない（L1） |
| start tbody/tfoot/thead：clear back、挿入、"in table body" へ | `M/in_table.rs:95-102` | 一致 | template.dat#14、#17、#18 | |
| start td/th/tr：clear back、属性なしの tbody を挿入、"in table body" へ、再処理 | `M/in_table.rs:105-112` | 一致 | tables01.dat#1、#2、tests1.dat#108 | |
| start table：Parse error | `M/in_table.rs:116` | 一致 | tests6.dat#42 | |
| start table：table が table scope になければ無視 | `M/in_table.rs:117-120` | 一致 | 未カバー | template の中（`<template><caption></caption><table>`）でだけ起きる。手で確かめた：`<template><tbody></tbody><table>x</template>` は仕様どおり table を無視する |
| start table：table が pop されるまで pop、reset the insertion mode、再処理 | `M/in_table.rs:121-123` | 一致 | tests6.dat#42、tests8.dat#8 | |
| end table：table scope になければ parse error、無視 | `M/in_table.rs:128-132` | 一致 | template.dat#63（`<template><thead></thead></table>`） | |
| end table：table まで pop、reset the insertion mode | `M/in_table.rs:133-135` | 一致 | adoption01.dat#11 ほか多数 | |
| end body/caption/col/colgroup/html/tbody/td/tfoot/th/thead/tr：Parse error、無視 | `M/in_table.rs:203-216` | 一致 | tables01.dat#6、tests6.dat#43 | 要素の列挙は仕様と同じ 11 個 |
| start style/script/template、end template：in head の規則 | `M/in_table.rs:217`、`245-247` | 一致 | tests7.dat#12（style）、template.dat#114（script）、template.dat#9（template と `</template>`） | |
| start input：type 属性がない、または "hidden" と ASCII 大文字小文字無視で一致しない → Anything else | `M/in_table.rs:140-149` | 一致 | webkit02.dat#11、tests7.dat#23（`type=" hidden"`） | 重複属性は最初のものが有効（`<table><input type=text type=hidden>` は foster される。手で確認） |
| start input（hidden）：Parse error、挿入、pop、self-closing を acknowledge | `M/in_table.rs:150-156` | 一致 | tests7.dat#19、#21、#22、html5test-com.dat#26 | |
| start form：Parse error | `M/in_table.rs:161` | 一致 | tests20.dat#46 | |
| start form：form element pointer が null でなく、parsing template contents でもなければ無視 | `M/in_table.rs:162-165` | 一致 | tests20.dat#46、#47 | |
| start form：挿入、parsing template contents でなければ form element pointer を設定、pop | `M/in_table.rs:166-179` | 意図した差（P2 で直した手順。出荷ビルドは仕様どおり） | template.dat#117、#118、tests20.dat#48 | `compat::fixed(Fix::P2)`。判定は `T/mod.rs:361`（`form_in_template`） |
| EOF：in body の規則 | `M/in_table.rs:250` | 一致 | tests16.dat#195、template.dat#91 | |
| Anything else：Parse error、foster parenting を有効に、in body で処理、無効に | `M/in_table.rs:184-196` | 一致 | adoption01.dat#6、#11、tests1.dat#78、template.dat#115 | 失敗時は abort（foster parenting は戻さないが構文解析も終わる） |
| clear the stack back to a table context（149422）：current node が table/template/html でない間 pop | `M/in_table.rs:11-22` | 一致 | tests1.dat#108、tests6.dat#37（ctx=tbody）、tests_innerHTML_1.dat#12（ctx=table） | HTML 名前空間で判定 |
| （補助）foster parenting の appropriate place | `T/actions.rs:62-104` | 一致 | adoption01.dat#12、template.dat#115 | last template と last table の比較、parent の有無、直前要素の 3 分岐とも仕様どおり |

## 3. "in table text" insertion mode（仕様 149436〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| U+0000：Parse error、無視 | `M/in_table_text.rs:16-30` | 一致 | pending-spec-changes-plain-text-unsafe.dat#1 | NUL を除いて保存する。Lexbor と同じく parse error を 2 回記録する（出力には影響しない） |
| Any other character token：pending list に追加 | `M/in_table_text.rs:19-45` | 一致 | adoption01.dat#12、tests15.dat#10 | 空白でない文字の有無を `have_non_ws` に持つ（`is_ws` は ASCII whitespace の 5 文字、`T/mod.rs:223`） |
| Anything else：空白でない文字があれば parse error、pending の各文字を "in table" の Anything else で再処理 | `M/in_table_text.rs:51-62` | 一致 | adoption01.dat#12、tests1.dat#33、tests15.dat#10 | `in_body::text_append`（`M/in_body.rs:64-81`）が in body の文字の手順（AFE の再構築、frameset-ok、挿入）を foster parenting 付きで行う |
| Anything else：そうでなければ pending の文字をそのまま挿入 | `M/in_table_text.rs:63-71` | 一致 | tests16.dat#195、template.dat#13、tests19.dat#26 | |
| Anything else：original insertion mode に戻して再処理 | `M/in_table_text.rs:76-77` | 一致 | 上と同じ | |

## 4. "in caption" insertion mode（仕様 149487〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| end caption：caption が table scope になければ parse error、無視 | `M/in_caption.rs:12-18` | 一致 | tests6.dat#18（ctx=caption）、tests_innerHTML_1.dat#23 | |
| end caption：generate implied end tags | `M/in_caption.rs:19` | 一致 | tests6.dat#19 | 補助の `generate_implied_end_tags`（`T/actions.rs:480`）は名前空間を見ないが、直後に caption まで pop するので出力は変わらない |
| end caption：current node が caption でなければ parse error | `M/in_caption.rs:20-25` | 一致 | tests6.dat#19 | |
| end caption：caption まで pop、AFE を marker までクリア、"in table" へ | `M/in_caption.rs:26-29` | 一致 | tests2.dat#36、template.dat#56 | |
| start caption/col/colgroup/tbody/td/tfoot/th/thead/tr、end table：caption がなければ無視 | `M/in_caption.rs:55-63`、`37` | 一致 | tests6.dat#21（ctx=caption）、tests_innerHTML_1.dat#20 | 列挙は 9 個で仕様と同じ |
| 同：caption を閉じて（上と同じ手順）再処理 | `M/in_caption.rs:11-29`（`consumed=false`） | 一致 | tests6.dat#16、#20、tests17.dat#5、pending-spec-changes.dat#2 | |
| end body/col/colgroup/html/tbody/td/tfoot/th/thead/tr：Parse error、無視 | `M/in_caption.rs:38-50` | 一致 | tests6.dat#22 | 列挙は 10 個で仕様と同じ |
| Anything else：in body の規則 | `M/in_caption.rs:51`、`64` | 一致 | tests10.dat#13、tests9.dat#14、tests6.dat#19 | |

## 5. "in column group" insertion mode（仕様 149563〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白文字：Insert the character | `M/in_column_group.rs:52-59` | 一致（current node が colgroup のとき） | domjs-unsafe.dat#37 | トークンの先頭の空白だけを挿入する（`split_ws_begin`） |
| 空白でない文字の後に続く空白文字（current node が colgroup でないとき） | `M/in_column_group.rs:57-60` → `10-17` | **逸脱** | 未カバー | 先頭の空白を除いた残りを丸ごと「Anything else」に渡す。current node が colgroup でない（template 内、または context が colgroup の fragment）と、残り全体を無視し、その中の空白も捨てる。仕様は文字ごとに処理するので、空白は挿入される。下の F3 |
| A comment token：Insert a comment | `M/in_column_group.rs:62-67` | 一致 | domjs-unsafe.dat#35 | |
| A processing instruction token | `M/in_column_group.rs:68-73` | 一致 | 未カバー | 手で確認：`<table><colgroup><?pi?></colgroup>` は PI が colgroup の子になる |
| A DOCTYPE token：Parse error、無視 | なし（`M/in_column_group.rs:85` の `_` に落ちる） | **逸脱** | domjs-unsafe.dat#34（ただし直後が `</colgroup>` なので差が出ない） | DOCTYPE の腕がなく（Lexbor `in_column_group.c` も同じ）、Anything else が colgroup を閉じて "in table" で再処理する。下の F2 |
| start html：in body の規則 | `M/in_column_group.rs:74` | 一致 | domjs-unsafe.dat#36 | |
| start col：挿入、pop、self-closing を acknowledge | `M/in_column_group.rs:75-82` | 一致 | tests1.dat#107、tests_innerHTML_1.dat#13 | |
| end colgroup：current node が colgroup でなければ parse error、無視 | `M/in_column_group.rs:31-38` | 一致 | tests_innerHTML_1.dat#36（ctx=colgroup） | |
| end colgroup：pop、"in table" へ | `M/in_column_group.rs:39-41` | 一致 | domjs-unsafe.dat#35、#36 | |
| end col：Parse error、無視 | `M/in_column_group.rs:43-46` | 一致 | tests6.dat#28、template.dat#70 | |
| start/end template：in head の規則 | `M/in_column_group.rs:47`、`83` | 一致 | template.dat#39、#70 | |
| EOF：in body の規則 | `M/in_column_group.rs:84` | 一致 | template.dat#78 | `<table><colgroup>` で終わる入力の WPT はないが、template.dat#78 が EOF をこの経路で通す |
| Anything else：current node が colgroup でなければ parse error、無視 | `M/in_column_group.rs:11-17` | 一致 | template.dat#78、#79、tests_innerHTML_1.dat#37（ctx=colgroup） | |
| Anything else：pop、"in table" へ、再処理 | `M/in_column_group.rs:18-20` | 一致 | tests6.dat#26、tables01.dat#4、tests1.dat#109 | |

## 6. "in table body" insertion mode（仕様 149657〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| start tr：clear back to table body、挿入、"in row" へ | `M/in_table_body.rs:72-79` | 一致 | tests10.dat#10、tests1.dat#107 | |
| start th/td：Parse error、clear back、属性なしの tr を挿入、"in row" へ、再処理 | `M/in_table_body.rs:80-94` | 一致 | webkit02.dat#9、template.dat#55、tests_innerHTML_1.dat#53 | 暗黙の tr はソース位置を持たない（L1） |
| end tbody/tfoot/thead：同名の要素が table scope になければ parse error、無視 | `M/in_table_body.rs:43-50` | 一致 | tests6.dat#35（ctx=tr 経由）、template.dat#15 | |
| end tbody/tfoot/thead：clear back、pop、"in table" へ | `M/in_table_body.rs:51-55` | 一致 | template.dat#14、#16、#17、#18 | |
| start caption/col/colgroup/tbody/tfoot/thead、end table：tbody/thead/tfoot が table scope になければ parse error、無視 | `M/in_table_body.rs:24-28`、`95-100`、`56` | 一致 | math.dat#3〜#8、svg.dat#3〜#8（fragment で scope にない場合） | |
| 同：clear back、pop、"in table" へ、再処理 | `M/in_table_body.rs:29-32` | 一致 | tests1.dat#108、#109、tests6.dat#37（ctx=tbody） | |
| end body/caption/col/colgroup/html/td/th/tr：Parse error、無視 | `M/in_table_body.rs:57-67` | 一致 | tests6.dat#40 | 列挙は 8 個で仕様と同じ |
| Anything else：in table の規則 | `M/in_table_body.rs:68`、`101` | 一致 | tests10.dat#9、tests7.dat#13 | |
| clear the stack back to a table body context（149746）：tbody/tfoot/thead/template/html でない間 pop | `M/in_table_body.rs:10-21` | 一致 | tests1.dat#107、template.dat#15 | |

## 7. "in row" insertion mode（仕様 149756〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| start th/td：clear back to table row、挿入、"in cell" へ、marker を挿入 | `M/in_row.rs:76-86` | 一致 | tests1.dat#87、tables01.dat#14 | |
| end tr：tr が table scope になければ parse error、無視 | `M/in_row.rs:24-30`、`41` | 一致 | tests6.dat#34、tests_innerHTML_1.dat#48（ctx=tr） | |
| end tr：clear back、pop、"in table body" へ | `M/in_row.rs:31-34` | 一致 | template.dat#53、tests1.dat#79 | |
| start caption/col/colgroup/tbody/tfoot/thead/tr、end table：tr がなければ parse error、無視 | `M/in_row.rs:87-93`、`42` | 一致 | math.dat#2、svg.dat#2（ctx=tr） | |
| 同：clear back、pop、"in table body" へ、再処理 | `M/in_row.rs:31-34`（`consumed=false`） | 一致 | tests1.dat#87、tables01.dat#13、tests17.dat#13 | |
| end tbody/tfoot/thead：同名の要素が table scope になければ parse error、無視 | `M/in_row.rs:43-50` | 一致 | tests6.dat#35（ctx=tr） | |
| end tbody/tfoot/thead：tr が table scope になければ無視 | `M/in_row.rs:51-56` | 一致 | 未カバー | 到達しない分岐と見られる（"in row" で tbody 等が scope にあるなら tr も scope にある） |
| end tbody/tfoot/thead：clear back、pop、"in table body" へ、再処理 | `M/in_row.rs:57-60` | 一致 | tests18.dat#35 | |
| end body/caption/col/colgroup/html/td/th：Parse error、無視 | `M/in_row.rs:62-71` | 一致 | tables01.dat#14 | 列挙は 7 個で仕様と同じ |
| Anything else：in table の規則 | `M/in_row.rs:72`、`94` | 一致 | tests2.dat#15、tests10.dat#10、processing-instructions.dat#111 | |
| clear the stack back to a table row context（149855）：tr/template/html でない間 pop | `M/in_row.rs:10-19` | 一致 | tests1.dat#87、template.dat#35 | |

## 8. "in cell" insertion mode（仕様 149865〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| end td/th：同名の HTML 要素が table scope になければ parse error、無視 | `M/in_cell.rs:28-34` | 一致 | tests6.dat#15（`<td></th>`） | |
| end td/th：generate implied end tags | `M/in_cell.rs:35` | 一致 | tests10.dat#12、tests9.dat#13 | |
| end td/th：current node が同名でなければ parse error | `M/in_cell.rs:36-41` | 一致 | tests10.dat#12 | |
| end td/th：同名の HTML 要素まで pop、AFE を marker までクリア、"in row" へ | `M/in_cell.rs:42-45` | 一致 | adoption01.dat#11、template.dat#37 | `</th>` が th を閉じる入力は WPT になく、手で確認した（`<table><tr><th>x</th><td>y</td>`） |
| start caption/col/colgroup/tbody/td/tfoot/th/thead/tr：Assert（td/th が table scope にある）、close the cell、再処理 | `M/in_cell.rs:66-81` | 一致 | tests1.dat#87、math.dat#1（ctx=td）、pending-spec-changes.dat#3 | 仕様は Assert だが、実装は scope にないとき parse error で無視する（Lexbor の旧来の手順）。Assert が成り立つ限り到達しない |
| end body/caption/col/colgroup/html：Parse error、無視 | `M/in_cell.rs:47-50` | 一致 | tables01.dat#11、tests6.dat#24 | |
| end table/tbody/tfoot/thead/tr：同名の要素が table scope になければ parse error、無視 | `M/in_cell.rs:51-58` | 一致 | webkit02.dat#6、tests2.dat#4 | |
| 同：close the cell、再処理 | `M/in_cell.rs:59-60` | 一致 | tables01.dat#12、tests1.dat#20、template.dat#29 | |
| Anything else：in body の規則 | `M/in_cell.rs:62`、`82` | 一致 | tests10.dat#11、namespace-sensitivity.dat#1 | |
| close the cell（149937）：generate implied end tags | `M/in_cell.rs:11` | 一致 | tests1.dat#87 | |
| close the cell：current node が td/th でなければ parse error | `M/in_cell.rs:12-17` | 一致 | tests1.dat#20 | |
| close the cell：td/th まで pop、AFE を marker までクリア、"in row" へ | `M/in_cell.rs:18-20`、`T/stacks.rs:79-86` | 一致 | tests1.dat#20、#31 | |

## 9. "in template" insertion mode（仕様 149964〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| character / comment / PI / DOCTYPE：in body の規則 | `M/in_template.rs:43-46` | 一致 | template.dat#1、#66、processing-instructions.dat#120、#124。DOCTYPE は未カバー | 手で確認：`<template><!doctype html>a</template>` は DOCTYPE を無視する |
| start base/basefont/bgsound/link/meta/noframes/script/style/template/title、end template：in head の規則 | `M/in_template.rs:36-38`、`47-56` | 一致 | template.dat#50、#67、#68、#69、#88、#101、#102 | 列挙は 10 個で仕様と同じ |
| start caption/colgroup/tbody/tfoot/thead：template mode を pop、"in table" を push、切り替えて再処理 | `M/in_template.rs:57-59`、`10-17` | 一致 | template.dat#28、#62、#92 | |
| start col：同、"in column group" | `M/in_template.rs:60` | 一致 | template.dat#73、#78 | |
| start tr：同、"in table body" | `M/in_template.rs:61` | 一致 | template.dat#32、#51 | |
| start td/th：同、"in row" | `M/in_template.rs:62` | 一致 | template.dat#27、#37 | |
| Any other start tag：同、"in body" | `M/in_template.rs:64` | 一致 | template.dat#5、#40、#90 | |
| Any other end tag：Parse error、無視 | `M/in_template.rs:39-40` | 一致 | template.dat#7、#85、#87 | |
| EOF：template がスタックになければ stop parsing | `M/in_template.rs:21-24` | 一致 | template.dat#116（ctx=template） | |
| EOF：parse error、template まで pop、AFE を marker までクリア、template mode を pop、reset、再処理 | `M/in_template.rs:25-30` | 一致 | template.dat#33、#39、#88、#91 | |
| （補助）reset the insertion mode appropriately | `T/actions.rs:536-618` | 一致 | template.dat#28、tests6.dat#42 | 仕様（142639〜）の各手順と照合した。select の手順は固定版の仕様にもない |

## 10. "after body" insertion mode（仕様 150098〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白文字：in body の規則 | `M/after_body.rs:38-43` | 一致 | webkit01.dat#27、tricky01.dat#9 | トークン全体が空白のときだけこの腕。空白と非空白が混ざると全体を in body で再処理するが、仕様の文字ごとの処理と同じ木になる（空白も非空白も in body に挿入される） |
| A comment token：html 要素に挿入 | `M/after_body.rs:12-17` | 一致 | tests19.dat#21、tests1.dat#34、tests_innerHTML_1.dat#79（ctx=html） | |
| A processing instruction token：html 要素に挿入 | `M/after_body.rs:18-23` | 一致 | processing-instructions.dat#118 | |
| A DOCTYPE token：Parse error、無視 | `M/after_body.rs:24` | 一致 | domjs-unsafe.dat#31 | |
| start html：in body の規則 | `M/after_body.rs:34` | 一致 | tests2.dat#54 | |
| end html：fragment なら parse error、無視 | `M/after_body.rs:27-30` | 一致 | tests6.dat#45（ctx=html） | |
| end html："after after body" へ | `M/after_body.rs:31-32` | 一致 | tests1.dat#11、webkit01.dat#22 | |
| EOF：stop parsing | `M/after_body.rs:36` | 一致 | doctype01.dat#33、menuitem-element.dat#17 | |
| Anything else：Parse error、"in body" へ、再処理 | `M/after_body.rs:45-47` | 一致 | tests2.dat#34、tests15.dat#4、#6 | |

## 11. "in frameset" insertion mode（仕様 150158〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白文字：Insert the character | `M/in_frameset.rs:84-88`、`12-30` | 一致 | tests2.dat#7 | 空白だけを取り出して挿入し、残り（非空白、U+0000）は無視する。文字ごとの処理と同じ結果 |
| A comment token | `M/in_frameset.rs:53-57` | 一致 | webkit01.dat#31 | |
| A processing instruction token | `M/in_frameset.rs:58-62` | 一致 | 未カバー | 手で確認：`<frameset><?x y?></frameset>` は frameset の子に PI |
| A DOCTYPE token：Parse error、無視 | `M/in_frameset.rs:63` | 一致 | tests2.dat#9 | |
| start html：in body の規則 | `M/in_frameset.rs:64` | 一致 | domjs-unsafe.dat#40 | |
| start frameset：挿入 | `M/in_frameset.rs:65-69` | 一致 | tests1.dat#105 | |
| end frameset：current node が root html なら parse error、無視 | `M/in_frameset.rs:36-39` | 一致 | tests6.dat#30、tests_innerHTML_1.dat#80（ctx=frameset） | |
| end frameset：pop、fragment でなく current node が frameset でなくなれば "after frameset" へ | `M/in_frameset.rs:40-47` | 一致 | tests1.dat#105（入れ子）、tests6.dat#8 | |
| start frame：挿入、pop、self-closing を acknowledge | `M/in_frameset.rs:70-76` | 一致 | tests1.dat#105、tests19.dat#46 | |
| start noframes：in head の規則 | `M/in_frameset.rs:77` | 一致 | webkit01.dat#31、tests15.dat#13 | |
| EOF：current node が root html でなければ parse error、stop | `M/in_frameset.rs:78-82` | 一致 | tests19.dat#46、tests2.dat#9 | |
| Anything else：Parse error、無視 | `M/in_frameset.rs:49-50`、`83-91` | 一致 | tests6.dat#29、tests19.dat#36（`</noframes>`）、tests10.dat#21 | |

## 12. "after frameset" insertion mode（仕様 150245〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白文字：Insert the character | `M/after_frameset.rs:36-41` | 一致 | tests2.dat#8、tests6.dat#8、#10 | in frameset と同じく空白だけを取り出す |
| A comment token | `M/after_frameset.rs:15-19` | 一致 | webkit01.dat#31 | |
| A processing instruction token | `M/after_frameset.rs:20-24` | 一致 | 未カバー | 手で確認：html の子に PI |
| A DOCTYPE token | `M/after_frameset.rs:25` | 一致 | domjs-unsafe.dat#42 | |
| start html：in body の規則 | `M/after_frameset.rs:31` | 一致 | domjs-unsafe.dat#41 | |
| end html："after after frameset" へ | `M/after_frameset.rs:27-29` | 一致 | tests19.dat#39、#40 | |
| start noframes：in head の規則 | `M/after_frameset.rs:33` | 一致（等価） | tests18.dat#31、#32 | 開始と終了を区別せず、`</noframes>` も in head に渡す。in head は `</noframes>` を parse error で無視する（`M/in_head.rs:78-89`）ので、仕様の「Anything else：無視」と同じ |
| EOF：stop parsing | `M/after_frameset.rs:34` | 一致 | tests18.dat#31、tests6.dat#8 | |
| Anything else：Parse error、無視 | `M/after_frameset.rs:35-43` | 一致 | tests6.dat#10、tests10.dat#22 | |

## 13. "after after body" insertion mode（仕様 150307〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| A comment token：Document に挿入 | `M/after_after_body.rs:17-24` | 一致 | webkit01.dat#22、#23、tests15.dat#5 | |
| A processing instruction token：Document に挿入 | `M/after_after_body.rs:25-32` | 一致 | processing-instructions.dat#119 | |
| DOCTYPE、空白文字、start html：in body の規則 | `M/after_after_body.rs:33`、`36-41` | 一致 | domjs-unsafe.dat#43、tests2.dat#57、webkit01.dat#28、tests19.dat#37 | 空白と非空白が混ざるトークンは全体を in body で再処理するが、after body と同じ理由で等価 |
| EOF：stop parsing | `M/after_after_body.rs:34` | 一致 | tests1.dat#11 | |
| Anything else：Parse error、"in body" へ、再処理 | `M/after_after_body.rs:43-45` | 一致（等価） | webkit01.dat#24、tests2.dat#56、tests1.dat#93、tests10.dat#19 | `</html>` は `res::TAG_HTML` の腕で in body に直接渡し、mode を "in body" に変えない。in body の `</html>` が "after body" に移して再処理し、"after after body" に戻るので、結果は同じ |

## 14. "after after frameset" insertion mode（仕様 150350〜）

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| A comment token：Document に挿入 | `M/after_after_frameset.rs:17-24` | 一致 | tests19.dat#39、tests18.dat#34 | |
| A processing instruction token：Document に挿入 | `M/after_after_frameset.rs:25-32` | 一致 | 未カバー | 手で確認：`</html><?b?>` は Document の子に PI |
| DOCTYPE、start html：in body の規則 | `M/after_after_frameset.rs:33` | 一致 | tests19.dat#44、#38 | `</html>` も in body に渡すが、body が scope にないので無視され、仕様（Anything else：無視）と同じ |
| 空白文字（トークン全体が空白）：in body の規則 | `M/after_after_frameset.rs:36-41` | 一致 | tests19.dat#40、tests6.dat#46 | |
| 空白文字（非空白と同じトークンに混ざる） | `M/after_after_frameset.rs:36-44` | **逸脱** | 未カバー（tests19.dat#41 `</html>abc` は空白を含まない） | トークンに非空白が 1 文字でもあると、空白を含めて全体を無視する（Lexbor `after_after_frameset.c` と同じ）。仕様は空白を in body の規則で挿入する。下の F4 |
| EOF：stop parsing | `M/after_after_frameset.rs:34` | 一致 | tests19.dat#39、#40 | |
| start noframes：in head の規則 | `M/after_after_frameset.rs:35` | 一致（等価） | tests18.dat#33、#34 | `</noframes>` も in head に渡すが、そこで無視されるので等価 |
| Anything else：Parse error、無視 | `M/after_after_frameset.rs:43-44` | 一致 | tests19.dat#41、#42、tests18.dat#21 | |

---

## 逸脱と未実装

未実装はない。逸脱は次の 4 件で、いずれも Lexbor（`vendor/lexbor/source/lexbor/html/tree/insertion_mode/*.c`）の同じ箇所をそのまま移植したもの。`docs/parser_differences.ja.md` には載っていない。WPT は 4 件とも検査していない。

### F1 "in table" の文字トークンで、current node が template のとき "in table text" に入らない

- **仕様**：149235「A character token, if the current node is table, tbody, **template**, tfoot, thead, or tr element」→ "in table text"。そこで空白だけなら、pending の文字をそのまま挿入する（149458〜、AFE を再構築しない）。
- **コード**：`ext/makiri/rust/src/html/tree/modes/in_table.rs:29-33`（列挙に `TAG_TEMPLATE` がない）。template のときは `anything_else`（`in_table.rs:184`）から in body の文字処理に行き、空白文字でも AFE を再構築する。Lexbor `in_table.c` の `lxb_html_tree_insertion_mode_in_table_text_open` も同じ列挙。
- **起きる条件**：template の中で "in table" に戻り（`<tbody></tbody>`、`<caption></caption>` などの後）、current node が template で、AFE の最後の marker より後に、スタックにない formatting element が残っていて、空白だけの文字が来たとき。空白以外の文字は、どちらの経路でも in body の再構築を通るので差が出ない。
- **再現**：`<template><tbody></tbody><p><b></p> </template>`
  - 仕様の木（template の内容）：
    ```
    <tbody>
    <p>
      <b>
    " "
    ```
  - 実装の木：
    ```
    <tbody>
    <p>
      <b>
    <b>
      " "
    ```
- **確信度**：高い（Ruby の拡張で出力を確認し、仕様の手順を 1 トークンずつ追った）。

### F2 "in column group" に DOCTYPE トークンの腕がない

- **仕様**：149588「A DOCTYPE token：Parse error. Ignore the token.」
- **コード**：`ext/makiri/rust/src/html/tree/modes/in_column_group.rs:51-86` の `match` に `TAG__EM_DOCTYPE` がなく、`_ => anything_else`（85 行）に落ちる。current node が colgroup なら、colgroup を pop して "in table" で再処理する（そこで DOCTYPE は無視される）。Lexbor `in_column_group.c` も同じ。
- **再現**：`<table><colgroup><!doctype html><col></table>`
  - 仕様：`<table><colgroup><col></colgroup></table>`（colgroup 一つ）
  - 実装：`<table><colgroup></colgroup><colgroup><col></colgroup></table>`（後の `<col>` が暗黙の colgroup を新たに作る）
- **WPT**：domjs-unsafe.dat#34（`<table><colgroup><!DOCTYPE html></colgroup></table>`）は DOCTYPE の後が `</colgroup>` なので、どちらでも同じ木になり、検出しない。
- **確信度**：高い（出力を確認）。

### F3 "in column group" の Anything else が、テキストトークンの残りを丸ごと無視する

- **仕様**：149572（空白文字は Insert the character）と 149637（Anything else：current node が colgroup でなければ parse error、無視）は文字ごとに適用されるので、非空白の後の空白は挿入される。
- **コード**：`ext/makiri/rust/src/html/tree/modes/in_column_group.rs:52-60`。先頭の空白だけを挿入し（`split_ws_begin`）、残りを `anything_else`（10-17 行）に渡す。current node が colgroup でないときは、残り全体（中の空白も）を無視する。current node が colgroup のときは "in table" で残りを再処理するので正しい。Lexbor `in_column_group.c` の `..._text` と同じ。
- **起きる条件**：current node が colgroup でない "in column group"。つまり template の中（`<template><col>` の後）か、context が colgroup の fragment。
- **再現 1**：`<template><col>a b</template>`
  - 仕様（template の内容）：`<col>`、`" "`
  - 実装：`<col>` だけ
- **再現 2**：`Makiri::DocumentFragment.parse("a b<col>", context: "colgroup")`
  - 仕様：`" "`、`<col>`
  - 実装：`<col>` だけ
- **WPT**：tests6.dat#27（ctx=colgroup、`foo<col>`）は空白を含まない。
- **確信度**：高い（出力を確認）。

### F4 "after after frameset" で、非空白と同じトークンの空白を捨てる

- **仕様**：150371「A character token that is … whitespace → in body の規則」、150390「Anything else：Parse error. Ignore the token.」。文字ごとなので、空白は in body の規則で挿入される（挿入先は current node の html 要素）。
- **コード**：`ext/makiri/rust/src/html/tree/modes/after_after_frameset.rs:36-44`。トークン全体が空白のときだけ in body に渡し、非空白を含むトークンは空白ごと無視する。"after frameset"（`after_frameset.rs:36-41`）と "in frameset" は空白を取り出して挿入しており正しい。Lexbor `after_after_frameset.c` の `LXB_TAG__TEXT` と同じ。
- **再現**：`<frameset></frameset></html> x `
  - 仕様：`<html><head></head><frameset></frameset>"  "</html>`（空白 2 文字が html の子のテキストになる）
  - 実装：`<html><head></head><frameset></frameset></html>`（テキストなし）
  - 比較：`<frameset></frameset> x `（"after frameset"）は実装でも `"  "` が残る。
- **WPT**：tests19.dat#40（`</html>  `、空白だけ）と #41（`</html>abc`、空白なし）はどちらも差が出ない。
- **確信度**：高い（出力を確認）。

### 参考（逸脱ではない）

- `generate_implied_end_tags`（`ext/makiri/rust/src/html/tree/actions.rs:480-500`）は、仕様の「dd, dt, li, …」を HTML 名前空間に限らず、タグ id だけで判定する（SVG の `<p>` なども pop する）。この範囲の呼び出し元（close the cell、`</caption>`、`</td>`）では、直後に caption/td/th まで pop するので、出力は変わらない。in body など別の範囲の呼び出しでは影響しうるので、その監査で確かめるべき点として挙げておく。
- "in cell" の caption 等の開始タグ（149899）で、仕様は Assert に変わったが、実装は scope を確かめて無視する旧来の手順を残している。Assert が成り立つ限り到達しない。
- "in table text" の U+0000 の parse error を 2 回記録する（`in_table_text.rs:16-18`、`25-27`）。出力には影響しない。

## 未カバーの step

WPT の入力を grep して、該当するテストが見つからなかった step。手で確認したものはそう書いた。

| insertion mode | step | 備考 |
|---|---|---|
| in table | 文字トークンで current node が template（149235） | F1。逸脱 |
| in table | start table で table が table scope にない（無視する） | template の中でだけ起きる。手で確認、一致 |
| in column group | A processing instruction token | 手で確認、一致 |
| in column group | A DOCTYPE token | F2。domjs-unsafe.dat#34 は差を検出しない |
| in column group | 非空白の後の空白（current node が colgroup でない） | F3。逸脱 |
| in row | end tbody/tfoot/thead で、同名要素は scope にあるが tr が scope にない | 到達しない分岐と見られる |
| in cell | `</th>` が th を閉じる | `</td>` と同じコード。手で確認、一致 |
| in cell | caption 等の開始タグで td/th が scope にない（実装の無視の分岐） | 仕様では Assert。到達しない |
| in template | A DOCTYPE token | 手で確認、一致 |
| in frameset | A processing instruction token | 手で確認、一致 |
| after frameset | A processing instruction token | 手で確認、一致 |
| after after frameset | A processing instruction token | 手で確認、一致 |
| after after frameset | 非空白と同じトークンの空白 | F4。逸脱 |
| text | EOF で current node が script（already started） | scripting 無効のため対象外 |
| text | `</script>` の script 実行の手順 | scripting 無効のため対象外 |
