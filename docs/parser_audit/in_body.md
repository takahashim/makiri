# "in body" insertion mode の監査（HTML Standard de953a7 対 移植版 tree builder）

- 仕様：`spec/source`（commit `de953a7cf0523f5873a1904ba2ce010c689dfb66`）の 147629–149077 行（"in body"、close a p element、adoption agency algorithm）。関連して 142947 行（Noah's Ark）、142970 行（reconstruct）、142825 行（scope）、142773 行（special）、149413 行（"in table" の anything else）を読んだ。
- 実装：`ext/makiri/rust/src/html/tree/modes/in_body.rs`（以下 `in_body.rs`）、`html/tree/actions.rs`、`html/tree/stacks.rs`、`html/tree/mod.rs`、`html/tree/select.rs`、`compat.rs`（今は `src/compat.rs`）。行番号は現在の作業ツリーのもの。パスは `ext/makiri/rust/src/html/tree/` からの相対で書く。
- `in_body.rs` 自身には `compat::fixed(Fix::...)` の分岐はない。"in body" に関わる修正は P2（`mod.rs:361` の `form_in_template`）だけで、form の開始タグと終了タグが使う。
- テスト番号は `spec/conformance/html5lib_runner.rb` と同じ数え方（ファイルごとに 1 始まり）。scripting-on のテスト（14 件）は数えていない。
- 確認の方法：(1) コードを Lexbor（`vendor/lexbor/source/lexbor/html/tree/insertion_mode/in_body.c`）と仕様の両方に照らして読んだ。(2) WPT の `.dat` を入力と期待値で検索し、step ごとに代表のテストを選んだ。(3) 組み込み済みの Ruby 拡張に、手で作った入力と乱択の入力（約 10 万件）を与え、parse5 8.0.1（scripting 無効）の出力と比べた。parse5 は古い `<select>` の規則、`search` 要素、P2、foreign content の一部で固定版の仕様と食い違うので、食い違いはすべて手で仕様をたどって判定した。adoption agency の食い違いは、parse5 の bookmark を移植版と同じ「添字の bookmark」に書き換えた複製が移植版と一致することで、原因を確かめた（後述 F1）。

状態の内訳（表の行数、111 行）：**一致 100、意図した差 5、未実装 0、逸脱 6**。逸脱の 6 行は F1〜F5 の 5 件に当たる（F1 は adoption agency の 2 行にまたがる）。

## 1. step ごとの表

| step（分岐と手順） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| **振り分け**：token の種類と tag 名で枝を選ぶ | `in_body.rs:926-1106` | 一致 | 全般 | 開始タグと終了タグの tag 名の一覧は、下の各行で仕様と突き合わせた |
| 文字 U+0000：parse error、無視 | `in_body.rs:38-49` | 一致 | plain-text-unsafe.dat#9、tests19.dat#50 | Lexbor は文字をまとめて一つの token にするので、NUL を取り除いて残りを処理する。全部 NUL のときは再構築もしない（仕様も無視するだけ） |
| 空白文字：reconstruct、文字を挿入 | `in_body.rs:64-81` | 一致 | tests19.dat#52、tests19.dat#83 | |
| ほかの文字：reconstruct、挿入、frameset-ok を "not ok" | `in_body.rs:64-81`（73-75） | 一致 | tests19.dat#47、#48、#49、#82 | 空白でない byte が一つでもあれば "not ok"。U+FFFD も含む |
| comment：comment を挿入 | `in_body.rs:988-993`、`actions.rs:372` | 一致 | comments01.dat#1 | |
| processing instruction：挿入 | `in_body.rs:994-999`、`actions.rs:394` | 一致 | processing-instructions.dat#1、#2 | |
| DOCTYPE：parse error、無視 | `in_body.rs:1000-1003` | 一致 | domjs-unsafe.dat#33 | |
| `html` 開始：parse error。stack に template があれば無視 | `in_body.rs:84-88` | 一致 | template.dat#72、#73、#74 | |
| `html` 開始：stack の先頭の要素に、まだない属性を足す | `in_body.rs:89-96`、`actions.rs:175-207` | 一致 | tests19.dat#38、noscript01.dat#2 | 既存の判定は local name と名前空間、または qualified name（Lexbor の `attr_by_local_name_ns`） |
| `html`／`body` 開始の parser sanitizer configuration の判定 | なし | 意図した差 | — | 文書と fragment の解析は parser sanitizer configuration を持たない（null）。sanitizer つきの解析（`setHTML` など）を提供していない |
| `base` `basefont` `bgsound` `link` `meta` `noframes` `script` `style` `template` `title` 開始、`</template>`：in head で処理 | `in_body.rs:100-102`、`929`、`1005-1014` | 一致 | webkit01.dat#40、template.dat#1、tests19.dat#64、#65 | 一覧は仕様の 10 個と一致 |
| `body` 開始：parse error。stack が一つだけ、二番目が body でない、template がある、のどれかなら無視 | `in_body.rs:105-115` | 一致 | template.dat#71、#103、foreign-fragment.dat#57、tests6.dat#32 | 二番目の要素の判定は tag だけを見る（名前空間を見ない）が、二番目に HTML 以外の `body` は来ない |
| `body` 開始：frameset-ok を "not ok"、body に、まだない属性を足す | `in_body.rs:116-121` | 一致 | tests19.dat#85、tests10.dat#23 | |
| `frameset` 開始：parse error。stack が一つだけ、または二番目が body でなければ無視 | `in_body.rs:125-129` | 一致 | tests_innerHTML_1.dat#5、#6、foreign-fragment.dat#52 | |
| `frameset` 開始：frameset-ok が "not ok" なら無視 | `in_body.rs:130-132` | 一致 | tests19.dat#47、#53〜#76 | |
| `frameset` 開始：二番目の要素を親から外し、html まで pop し、挿入し、"in frameset" へ | `in_body.rs:133-142` | 一致 | tests19.dat#45、#84、tests1.dat#112 | `open_pop_until_node(html, false)` は html を残す |
| EOF：template insertion mode の stack が空でなければ "in template" で処理 | `in_body.rs:146-149` | 一致 | template.dat#24、#25、#88 | |
| EOF：一覧にない要素が stack にあれば parse error。解析を止める | `in_body.rs:150-154`、`actions.rs:690-714` | 一致 | 多数 | parse error だけの注記：`check_scope_element` は「一覧の要素が一つでもあれば真」で、html は常に一覧にあるので、このエラーは報告されない（Lexbor の `lxb_html_tree_check_scope_element` と同じ反転）。木は変わらない。`</body>`、`</html>` も同じ |
| `</body>`：body が scope になければ parse error、無視 | `in_body.rs:158-165` | 一致 | tests6.dat#32、#45、tests7.dat#28 | |
| `</body>`：（parse error の判定）"after body" へ | `in_body.rs:166-170` | 一致 | tests1.dat#34、tests19.dat#21 | |
| `</html>`：同じ判定のあと "after body" へ、再処理 | `in_body.rs:174-187` | 一致 | domjs-unsafe.dat#43、tables01.dat#4、processing-instructions.dat#119 | `false` を返して再処理する |
| `address`〜`ul` の 25 個の開始タグ：p が button scope にあれば close a p element、挿入 | `in_body.rs:223-227`、`1018-1042` | 一致 | blocks.dat#1〜#40、search-element.dat#1 | 一覧（`search` を含む 25 個）は仕様と一致 |
| `h1`〜`h6` 開始：close p、current node が h1〜h6 なら parse error と pop、挿入 | `in_body.rs:230-240` | 一致 | tests1.dat#22、#95 | current node の判定は名前空間を見ないが、"in body" で処理するときの current node は HTML 要素か integration point なので答えは変わらない |
| `pre` `listing` 開始：close p、挿入 | `in_body.rs:243-247` | 一致 | blocks.dat#41、#33、#34 | |
| `pre` `listing`：次の token が LF なら無視 | `in_body.rs:15-26`、`248-249` | 一致 | tests3.dat#5、#6、plain-text-unsafe.dat#29 | 次の token の先頭の改行一つを落とす専用の mode（Lexbor と同じ）。`<pre>&#10;x` も落ちることを確かめた |
| `pre` `listing`：frameset-ok を "not ok" | `in_body.rs:250` | 一致 | tests19.dat#53、#54 | |
| `form` 開始：pointer が null でなく、parsing template contents でなければ parse error、無視 | `in_body.rs:255-260`、`mod.rs:361-370` | 一致 | template.dat#119〜#121（template 側）、本体側は未カバー | `<form><div><form>x` を手で確かめた。P2 |
| `form` 開始：close p、挿入、template でなければ pointer を設定 | `in_body.rs:261-268` | 一致 | template.dat#117、#120 | P2（`docs/parser_differences.ja.md`） |
| `li` 開始：frameset-ok、current node から上へ li を探し、implied end tags（li 以外）、pop until li | `in_body.rs:274-288`、`300-305` | 一致 | tests1.dat#103、tests3.dat#20、tests19.dat#55 | |
| `li` 開始：special で address、div、p 以外に当たれば打ち切り | `in_body.rs:289-295` | 一致 | tests1.dat#103、tests3.dat#20 | special の一覧（`res/tree.rs` の `TAG_CATS`）は仕様の 142773 行の一覧と完全に一致（HTML 82 個＋MathML 6 個＋SVG 3 個。`image` も入っているが stack に載らない） |
| `li` 開始：done で close p、挿入 | `in_body.rs:302-303` | 一致 | tests1.dat#103 | |
| `dd` `dt` 開始：同じループで dd と dt の両方を探す | `in_body.rs:308-313` | 一致 | tests2.dat#11、tests19.dat#56、#57、tricky01.dat#4 | 見つけた方の tag だけを implied end tags から除く |
| `plaintext` 開始：close p、挿入、PLAINTEXT state | `in_body.rs:316-323` | 一致 | tests18.dat#1、plain-text-unsafe.dat#10 | |
| `button` 開始：button が scope にあれば parse error、implied end tags、pop until button | `in_body.rs:326-334` | 一致 | tests20.dat#1、tests6.dat#14、tests26.dat#16 | |
| `button` 開始：reconstruct、挿入、frameset-ok を "not ok" | `in_body.rs:335-342` | 一致 | tests19.dat#58 | |
| `</address>`〜`</ul>` の 28 個：scope になければ無視。implied end tags、pop until | `in_body.rs:346-363`、`932-959` | 一致 | blocks.dat#2、adoption01.dat#3、tests1.dat#110（scope にない場合） | 一覧（`select`、`listing`、`pre`、`button` を含む 28 個）は仕様と一致 |
| `</form>`（template contents でない）：pointer を null にし、scope になければ無視、implied end tags、node を stack から取り除く | `in_body.rs:367-383` | 一致 | tests6.dat#2、tests16.dat#197、tests1.dat#110 | pointer を先に null にする順も仕様どおり |
| `</form>`（template contents）：form が scope になければ無視、implied end tags、pop until form | `in_body.rs:385-400` | 一致 | template.dat#124 | P2 |
| `</p>`：p が button scope になければ parse error、属性のない p を挿入 | `in_body.rs:404-414` | 一致 | tricky01.dat#8、tests1.dat#110 | |
| `</p>`：close a p element | `in_body.rs:415`、`actions.rs:717-727` | 一致 | adoption01.dat#1 ほか多数 | implied end tags（p 以外）、pop until p |
| `</li>`：list item scope になければ無視。implied end tags（li 以外）、pop until li | `in_body.rs:420-437` | 一致 | tests1.dat#103、#104 | |
| `</dd>` `</dt>`：scope になければ無視。implied end tags（同名以外）、pop until | `in_body.rs:440-457` | 一致 | tests1.dat#110（scope にない場合だけ） | 閉じる場合は WPT に見当たらない。`<dd><div></dd>x`、`<dl><dd>a</dd><dt>b</dt></dl>` を手で確かめた |
| `</h1>`〜`</h6>`：h1〜h6 のどれかが scope になければ無視。implied end tags、pop until h1〜h6 のどれか | `in_body.rs:460-474`、`actions.rs:661`、`stacks.rs:69` | 一致 | tests1.dat#106、tests19.dat#22、#24 | |
| `</sarcasm>`：any other end tag | `in_body.rs:983` | 一致 | webkit02.dat#4 | |
| `a` 開始：AFE の最後の marker より後に a があれば parse error、AAA、まだ残っていれば AFE と stack から取り除く | `in_body.rs:477-486`、`stacks.rs:238` | 一致 | tests1.dat#91、adoption02.dat#2、#4、tests1.dat#31 | AAA の「any other end tag」の戻り値を無視するが、この経路では formatting element が必ず見つかる |
| `a` 開始：reconstruct、挿入、AFE に push | `in_body.rs:487-495` | 一致 | adoption01.dat 全般 | |
| `b` `big` `code` `em` `font` `i` `s` `small` `strike` `strong` `tt` `u` 開始 | `in_body.rs:499-508`、`1053-1064` | 一致 | adoption01.dat#4、#7、#8 | 12 個の一覧は仕様と一致 |
| AFE への push（Noah's Ark）：最後の marker より後に、同じ名前、名前空間、属性の要素が三つあれば最も早いものを除く | `stacks.rs:250-271` | 一致 | tests23.dat#1、#2、adoption01.dat#16、#17 | marker で探索を止める |
| Noah's Ark：属性の比較は「token 上の名前、名前空間、値」 | `stacks.rs:273-319` | **逸脱** | 未カバー | F4。値のない属性（`<b x>`）と空の値（`<b x="">`）を別の属性として数える |
| `nobr` 開始：reconstruct。nobr が scope にあれば parse error、AAA、もう一度 reconstruct。挿入、push | `in_body.rs:511-539` | 一致 | tests26.dat#1〜#3、adoption02.dat#3 | AAA が「any other end tag」を返したときは、開始タグの token のまま any other end tag の手順を走らせる（仕様の文言どおり） |
| `</a>` `</b>` … `</nobr>` … `</u>`（14 個）：AAA | `in_body.rs:542-551`、`967-980` | 一致 | adoption01.dat、adoption02.dat 全般 | 一覧は仕様と一致 |
| `applet` `marquee` `object` 開始：reconstruct、挿入、marker、frameset-ok | `in_body.rs:554-566` | 一致 | tests19.dat#59、#60、#61、tests1.dat#26 | |
| `</applet>` `</marquee>` `</object>`：scope になければ無視。implied end tags、pop until、marker まで消す | `in_body.rs:569-587` | 一致 | tests10.dat#38、tests1.dat#81、#110 | |
| `table` 開始：quirks でなく p が button scope にあれば close p。挿入、frameset-ok、"in table" へ | `in_body.rs:590-601` | 一致 | quirks01.dat#1〜#4、tests3.dat#23、#24、tests20.dat#42、tests19.dat#62 | |
| `</br>`：parse error、属性を捨てて `<br>` として扱う | `in_body.rs:619-623` | 一致 | tests1.dat#110、noscript01.dat#12 | |
| `area` `br` `embed` `img` `keygen` `wbr` 開始：reconstruct、挿入、pop、self-closing を認める、frameset-ok | `in_body.rs:604-615` | 一致 | tests19.dat#63、#66、#67、#68、#70、#71 | |
| `input` 開始：fragment の context が select なら parse error、無視 | `in_body.rs:627-630` | 一致 | tests_innerHTML_1.dat#76 | |
| `input` 開始：select が scope にあれば parse error、pop until select | `in_body.rs:631-634` | 一致 | tests7.dat#17、webkit02.dat#44 | |
| `input` 開始：reconstruct、挿入、pop、self-closing | `in_body.rs:635-642` | 一致 | tests19.dat#69 | |
| `input` 開始：`type` が ASCII case-insensitive に "hidden" でなければ frameset-ok を "not ok" | `in_body.rs:643-662` | **逸脱** | webkit01.dat#51、#52、pending-spec-changes.dat#1（小文字だけ） | F3。値を byte のまま比べる |
| `param` `source` `track` 開始：挿入、pop、self-closing | `in_body.rs:667-674` | 一致 | tests25.dat#23〜#25、tests6.dat#48 | frameset-ok を変えない点も仕様どおり |
| `hr` 開始：close p。select が scope にあれば implied end tags、option か optgroup が scope にあれば parse error。挿入、pop、self-closing、frameset-ok | `in_body.rs:677-695` | 一致 | webkit02.dat#26〜#35、tests19.dat#72 | |
| `image` 開始：parse error、tag 名を img に変えて再処理 | `in_body.rs:698-702` | 一致（"in body" から呼ばれたとき） | tests1.dat#90、tests19.dat#93 | "in table" から呼ばれたときは再処理が失われる。F5 |
| `textarea` 開始：挿入、次の LF を無視、RCDATA state | `in_body.rs:705-710`、`28-35` | 一致 | tests3.dat#17、#18、#19 | |
| `textarea` 開始：original insertion mode を設定、frameset-ok、"text" へ | `in_body.rs:711-714`、`15-26` | **逸脱** | tests19.dat#73（frameset-ok の部分だけ） | F2。改行を落とす mode の後、"text" でなく元の mode（"in body" など）に戻る |
| `xmp` 開始：close p、reconstruct、frameset-ok、generic raw text | `in_body.rs:718-728`、`actions.rs:452` | 一致 | tests19.dat#74、tests16.dat#98 | |
| `iframe` 開始：frameset-ok、generic raw text | `in_body.rs:731-737` | 一致 | tests19.dat#75、tests16.dat#96 | |
| `noembed` 開始：generic raw text | `in_body.rs:740-745` | 一致 | tests16.dat#99、#194 | frameset-ok を変えない |
| `noscript` 開始（scripting が有効）：generic raw text | `in_body.rs:897-900` | 意図した差 | — | パーサは常に scripting 無効で動く（`tree.scripting` は偽）。この枝には到達しない |
| `select` 開始：fragment の context が select なら parse error、無視 | `in_body.rs:755-758` | 一致 | 未カバー | context が select の fragment で `<select>x<option>y` を確かめた（`x<option>y</option>`） |
| `select` 開始：select が scope にあれば parse error、無視、pop until select | `in_body.rs:759-763` | 一致 | tests1.dat#30、tests7.dat#18、tests2.dat#38 | |
| `select` 開始：reconstruct、挿入、frameset-ok | `in_body.rs:764-771` | 一致 | tests19.dat#76 | |
| `option` 開始：select が scope にあれば implied end tags（optgroup 以外）、option が scope にあれば parse error。なければ current node の option を pop。reconstruct、挿入 | `in_body.rs:775-798` | 一致 | tests1.dat#35、template.dat#21、webkit02.dat#19 | select の中でも、current node が option でなければ pop しない（`<select><option><b>x<option>y` は入れ子になる。仕様どおり） |
| `optgroup` 開始：select が scope にあれば implied end tags、option か optgroup が scope にあれば parse error。なければ current node の option を pop。reconstruct、挿入 | `in_body.rs:801-821` | 一致 | tests1.dat#35、tests2.dat#37、#38 | |
| option が stack から pop されたときの selectedcontent の更新 | `select.rs:84-95`（`stacks.rs` のすべての pop から） | 一致（範囲外） | webkit02.dat 系 | "in body" の token の規則ではなく option 要素の定義（58983 行）の手順。selectedness は S1 |
| `rb` `rtc` 開始：ruby が scope にあれば implied end tags。current node が ruby でなければ parse error。挿入 | `in_body.rs:824-839` | 一致 | ruby.dat#1、#2、#3 | |
| `rp` `rt` 開始：ruby が scope にあれば implied end tags（rtc 以外）。current node が rtc か ruby でなければ parse error。挿入 | `in_body.rs:842-857` | 一致 | ruby.dat#2、#4、#6、#7 | |
| `math` 開始：reconstruct、MathML 属性と foreign 属性の調整、MathML の foreign element を挿入、self-closing なら pop と認める | `in_body.rs:860-881`、`1088`、`actions.rs:277-330` | 一致 | webkit02.dat#24、#25、tests19.dat#19、#78 | frameset-ok を変えない（tests19.dat#78） |
| `svg` 開始：同上（SVG 属性） | `in_body.rs:860-881`、`1089` | 一致 | webkit02.dat#23、tests19.dat#20、#77 | |
| `caption` `col` `colgroup` `frame` `head` `tbody` `td` `tfoot` `th` `thead` `tr` 開始：parse error、無視 | `in_body.rs:1090-1103` | 一致 | tests1.dat#108、#109、template.dat#64 | 11 個の一覧は仕様と一致。仕様でコメントアウトされた終了タグの一覧は any other end tag に任せており、コードも同じ |
| any other start tag：reconstruct、挿入 | `in_body.rs:884-890`、`1104` | 一致 | webkit02.dat#4、search-element.dat#8 | `menuitem`、`isindex` などもここ |
| `noscript` 開始（scripting 無効）：any other start tag | `in_body.rs:893-896` | 一致 | 未カバー | "in body" の noscript は WPT にない（noscript01.dat は "in head"）。`<p><noscript><b>x</b></noscript>` を手で確かめた |
| any other end tag：current node から上へ、同名の HTML 要素なら implied end tags（同名以外）、pop。special に当たれば parse error、無視 | `in_body.rs:904-923` | 一致 | tests7.dat#29、search-element.dat#8、webkit02.dat#4 | 同名の判定は名前空間を見る（`node_is`）。`<math><mi><em></mi>x` は MathML の mi が special なので無視される（parse5 は誤り） |
| **reconstruct the active formatting elements** | `stacks.rs:193-235` | 一致 | adoption01.dat、tests1.dat 全般 | rewind と create を一つのループで行う |
| **generate implied end tags**（10 個、除外つき） | `actions.rs:480-500` | 一致 | 多数 | 名前空間を見ずに tag だけで pop する（Lexbor と同じ）。"in body" で処理するときの current node は HTML 要素か integration point で、一覧の名前を持つ非 HTML 要素が current node になるのは終了タグの後で pop されるときだけなので、木は変わらない（parse error の有無だけ） |
| **scope の一覧**（scope、list item、button） | `res/tree.rs`、`actions.rs:622-688` | 一致 | 全般 | scope の一覧に `select` が入っているのは固定版の仕様どおり（142843 行） |
| **AAA step 1–2**：current node が subject の HTML 要素で AFE になければ pop して終わる | `actions.rs:782-789` | 一致 | tests23.dat#1 | |
| **AAA outer loop**：outerLoopCounter が 8 以上なら終わる | `actions.rs:790-795` | 一致 | adoption01.dat#14 | |
| **AAA formattingElement**：最後の marker より後で subject の名前を持つ最後の要素。なければ any other end tag | `actions.rs:796-810` | 一致 | tests1.dat#21、#94（marker）、tests1.dat#110 | |
| **AAA**：formattingElement が stack になければ parse error、AFE から除いて終わる | `actions.rs:812-818` | 一致 | tests1.dat#91 | |
| **AAA**：scope になければ parse error、終わる | `actions.rs:820-827` | 一致 | tests1.dat#91、#26 | |
| **AAA**：current node でなければ parse error | `actions.rs:829-832` | 一致 | — | parse error だけ |
| **AAA furthestBlock**：formattingElement より下で最も上の special 要素 | `actions.rs:834-838` | 一致 | adoption01.dat#1 | 探索は formattingElement 自身から始まるが、formatting 要素は special でないので同じ |
| **AAA**：furthestBlock がなければ formattingElement まで pop し、AFE から除いて終わる | `actions.rs:840-846` | 一致 | adoption01.dat#4 | |
| **AAA commonAncestor** と bookmark の設定 | `actions.rs:848-857` | 一致 | adoption01.dat 全般 | bookmark は AFE の添字として持つ（下の F1 の原因） |
| **AAA inner loop**：node を上へ（取り除かれた node の上も正しく辿る）、formattingElement で break | `actions.rs:859-875` | 一致 | webkit02.dat#13〜#17 | 取り除いた後は添字が詰まるので、`node_idx -= 1` で「取り除く前に上にあった要素」になる |
| **AAA inner loop**：innerLoopCounter > 3 で AFE にあれば AFE から除く | `actions.rs:877-882` | 一致 | adoption01.dat#15、tests22.dat#1 | この除去で bookmark と formattingElement の添字がずれる（F1） |
| **AAA inner loop**：AFE になければ stack から除いて continue | `actions.rs:884-888` | 一致 | webkit02.dat#13 | |
| **AAA inner loop**：node の token で要素を作り、AFE と stack の entry を置き換える | `actions.rs:890-902` | 一致 | adoption01.dat#2 | |
| **AAA inner loop**：bookmarkMoved が偽なら、bookmark を新しい要素の直後へ | `actions.rs:904-907` | **逸脱** | 未カバー | F1。直後を「その時点の添字＋1」で覚え、後の AFE の除去で添字を直さない |
| **AAA inner loop**：sanitizer の nodeAction、pendingRedirects | なし | 意図した差 | — | parser sanitizer configuration は常に null なので nodeAction は常に "Keep" |
| **AAA inner loop**："Keep"：lastNode を node に append、lastNode = node | `actions.rs:909-917` | 一致 | adoption01.dat 全般 | |
| **AAA**：insertionLocation（commonAncestor による appropriate place）、lastNode を外す、条件を満たせば target の refNode の前に挿入 | `actions.rs:920-927`、`62-109` | 一致 | adoption01.dat#6（foster）、template.dat#84（template） | 新しい版の五つの条件（lastNode が target の host-including inclusive ancestor でない、など）は、script も sanitizer もない解析では常に満たされる（stack の上下と木の祖先関係が逆にならないため）。コードは条件を調べずに挿入する |
| **AAA**：formattingElement の token で newElement を作る | `actions.rs:929-938` | 一致 | adoption01.dat 全般 | |
| **AAA**："Keep"：furthestBlock の子を newElement へ移し、adjusted insertion location（furthestBlock, null）に挿入 | `actions.rs:940-950` | 一致 | adoption01.dat#1 | コードは furthestBlock に直接 append する。furthestBlock が template なら template contents に入るのが新しい規則だが、template は marker を積むので、formattingElement が marker より後にあるかぎり template が furthestBlock になることはない |
| **AAA**：formattingElement を AFE から除き、newElement を bookmark の位置に入れる | `actions.rs:952-960` | **逸脱** | 未カバー | F1。除く位置（`formatting_index`）と入れる位置（`bookmark`）の両方が、ループの前に取った添字のまま |
| **AAA**：formattingElement を stack から除き、newElement を furthestBlock の直下に入れる | `actions.rs:962-967` | 一致 | adoption01.dat 全般 | |
| **AAA**：sanitizer の "Replace with children"／"Remove"、belowNode の redirection map | なし | 意図した差 | — | sanitizer がないので不要 |
| "in table" の anything else から呼ばれた "in body" の再処理要求（`image`） | `modes/in_table.rs:184-196` | **逸脱** | 未カバー | F5。範囲は "in table" だが、"in body" の `image` の手順が失われるので載せる |
| frameset-ok を変えない `math`／`svg`、`param` などの確認 | — | 一致 | tests19.dat#77、#78 | |
| scripting 有効時の手順（noscript の終了タグなど）、custom element reactions、script の実行 | なし | 意図した差 | — | scripting 無効のパーサには関係しない（`noscript` の行と同じ理由） |

## 2. 逸脱と未実装

未実装は見つからなかった。逸脱は 5 件。どれも Lexbor から引き継いだ振る舞いで、`docs/parser_differences.ja.md` に載っていない。WPT の tree construction のテストはどれも通る（1,959 件）ので、どれもテストのない入力で起きる。

### F1 adoption agency algorithm の bookmark と formattingElement の位置を添字で持つ

- **仕様**：148821 行（bookmark を formattingElement の位置に置く）、148879 行（innerLoopCounter > 3 で AFE から除く）、148894 行（bookmark を新しい要素の直後へ）、148996 行（formattingElement を AFE から除き、newElement を bookmark の位置に入れる）。bookmark は「両隣の要素に対する位置」で、AFE から要素を除いても動かない。
- **コード**：`actions.rs:857`（`let mut bookmark = formatting_index`）、`actions.rs:904-907`（`bookmark = afe_node_idx + 1`）、`actions.rs:877-882`（`afe_remove_by_node` で添字が詰まる）、`actions.rs:952-960`（`afe_remove(formatting_index)` の後に `afe_insert(element, bookmark)`。長さで丸めるだけで、前で除いた分を引かない）。Lexbor の `lxb_html_tree_adoption_agency_algorithm` と同じ。
- **ずれ方**：(a) 最初に作り直した要素 x が formattingElement より後ろにあるとき、bookmark は除く前の添字で x の直後を指すので、formattingElement を除いた後では一つ後ろにずれる。x の後ろに AFE の要素があれば、newElement はその要素の後ろに入る。(b) inner loop が formattingElement より前の AFE の要素を除くと、`formatting_index` が一つ後ろを指したままになり、step 19 で別の要素（作り直したばかりの要素）を AFE から除く。
- **最小の再現**（token を一つずつ削って最小にしたもの）：

  ```
  <em><s><b><div><strike><tt><nobr><a><div></s></em>
  ```

  - 仕様（parse5 と手でたどった結果が一致）：`<html><head></head><body><em><s><b></b></s><b></b></em><b><div><em><s><strike><tt><nobr><a></a></nobr></tt></strike></s><tt><nobr><a></a></nobr></tt></em><tt><nobr><a><div><em><s></s></em></div></a></nobr></tt></div></b></body></html>`
  - Makiri：`... </em><nobr><a><div><em><s></s></em></div></a></nobr></div></b></body></html>`（最後の `<tt>` がない）

  AFE の中身が違うことは、`</s>` の後に再構築させるとそのまま見える：

  ```
  <em><s><b><div><strike><tt><nobr><a><div></s></div></div>x
  ```

  - 仕様：`… </div><tt><nobr><a>x</a></nobr></tt></b></em>`（`</s>` の後の AFE は em、b、tt、nobr、a）
  - Makiri：`… </div><s><nobr><a>x</a></nobr></s></b></em>`（`</s>` の後の AFE は em、b、s、nobr、a。stack にない `s` が残り、`tt` が消えた）

  手でたどると、`</s>` の一回目の outer loop で仕様の AFE は `em b' s' strike tt nobr a`、Makiri は `em b' strike s' tt nobr a`（(a)）。二回目で `strike` が AFE から除かれ（innerLoopCounter = 4）、Makiri は古い `formatting_index` = 3 で `tt'` を除く（(b)）。
- **原因の確認**：parse5 の AAA を、bookmark と formattingElement の位置を添字で覚えて丸めるように書き換えた複製を作ると、formatting 要素と block を混ぜた乱択の入力 12,000 件で Makiri と完全に一致した（書き換える前は 5 件が食い違った）。ほかに見つかった再現：`<a href=1><b><ul><tt><em><i><i y><button></a></b>`（最後の `<em>` が `<i>` の外側に再構築されない）。
- **確信度**：高い。

### F2 `textarea` の後で "text" insertion mode に入らない

- **仕様**：148472–148495 行。insert、次の LF を無視、RCDATA state、original insertion mode を current mode に、frameset-ok を "not ok"、**"text" insertion mode へ切り替える**。"text" mode の文字 token は active formatting elements を再構築せずに挿入する。
- **コード**：`in_body.rs:711-713` で mode を `InBodySkipNewLineTextarea` にし、`in_body.rs:15-26`（`skip_new_line`）が次の token で `tree.mode = tree.original_mode`、つまり "in body"（"in table" から来たときは "in table"）に戻す。`textarea` の中身はその mode で処理されるので、"in body" の文字の規則が active formatting elements を再構築し、`textarea` の**中に**要素を作る。Lexbor の `lxb_html_tree_insertion_mode_in_body_skip_new_line_textarea` と同じ。
- **最小の再現**：`<p><b></p><textarea>x</textarea>`
  - 仕様：`<html><head></head><body><p><b></b></p><textarea>x</textarea></body></html>`
  - Makiri：`<html><head></head><body><p><b></b></p><textarea><b>x</b></textarea></body></html>`
  - 先頭に改行があっても同じ（`<p><b></p><textarea>\nx`）。`<p><b></p><table><textarea>x` でも、"in table" の anything else の経由で `<textarea><b>x</b></textarea>` になる。`<p><b></p><textarea></div>` は `<textarea><b>&lt;/div&gt;</b></textarea>`。
  - `textarea` の子に要素ができるので、DOM の形が変わる（`textContent` は同じだが、`children` や直列化が変わる）。
- **確信度**：高い。

### F3 `input` の `type` を大文字小文字を区別して "hidden" と比べる

- **仕様**：148426 行。「`type` 属性がない、または値が "hidden" に ASCII case-insensitive に一致しなければ、frameset-ok を "not ok"」。
- **コード**：`in_body.rs:656-659`（`d.attr_value(x) == Some(&b"hidden"[..])`）。Lexbor の `lxb_html_tree_insertion_mode_in_body_input` も `lexbor_str_data_cmp`（区別する）で、"in table" の `input`（`lexbor_str_data_ncasecmp`）とだけ食い違う。
- **最小の再現**：`<input type=HIDDEN><frameset>`
  - 仕様：`<html><head></head><frameset></frameset></html>`
  - Makiri：`<html><head></head><body><input type="HIDDEN"></body></html>`
- **テスト**：webkit01.dat#51、#52、pending-spec-changes.dat#1 は小文字だけ。tests7.dat#226 付近の `hidDEN` は "in table" の規則。
- **確信度**：高い。

### F4 Noah's Ark の属性の比較で、値のない属性と空の値の属性を別に数える

- **仕様**：142947–142957 行。「属性は、要素を作った token 上のものとして比べる。名前、名前空間、値が同じ組に分けられれば同じ属性」。tokenizer は値のない属性（`x`）に空文字列の値を与えるので、`x` と `x=""` は同じ属性である。
- **コード**：`stacks.rs:299-319`（`attr_compare`）。`valueless` の印（`actions.rs:228-230`）が片方にだけあると `false` を返す。Lexbor の `lxb_dom_attr_compare` が値の NULL と空を区別するのを写したもの。
- **最小の再現**：`<p><b x><b x><b x=""><b x></p>x`
  - 仕様：`<p>` の後に再構築される `<b>` は三つ（四つ目を push するとき、同じ要素が三つあるので最も早いものを除く）。`… </p><b x=""><b x=""><b x="">x</b></b></b></body></html>`
  - Makiri：四つ。`… </p><b x=""><b x=""><b x=""><b x="">x</b></b></b></b></body></html>`
  - `<p><b x=""><b x=""><b x=""><b x></p>x`、`<p><b x><b x><b x><b x=""></p>x` も同じ。全部値なし、全部空の値なら一致する。
- **確信度**：高い。

### F5 "in table" の anything else が "in body" の再処理の要求を捨てる（`image` が消える）

- **仕様**：149413 行（"in table" の anything else：foster parenting を有効にして "in body" の規則で処理し、無効に戻す）と 148465 行（"in body" の `image`：tag 名を img に変えて**再処理**）。再処理は current insertion mode（"in table"）で行われ、もう一度 anything else を通って `img` が foster parent される。
- **コード**：`modes/in_table.rs:190`。`super::in_body::in_body(tree, tkz, tok);` の戻り値（`false` は「再処理せよ」）を捨てて `true` を返すので、`img` に書き換えた token は処理されずに消える。`in_body.rs:698-702` の `image` は `false` を返す唯一の開始タグの枝（`</html>` も `false` を返すが、"in table" は `</html>` を自分で処理する）。"in caption" と "in cell" は戻り値をそのまま返すので影響しない。"in table body"、"in row"、"in column group" は "in table" に渡すので同じく消える。Lexbor も同じ。
- **最小の再現**：`<table><image>`
  - 仕様：`<html><head></head><body><img><table></table></body></html>`
  - Makiri：`<html><head></head><body><table></table></body></html>`
  - `<table><tbody><image>`、`<table><tr><image>`、`<table><colgroup><image>` も `img` が消える。context が `table` の fragment で `<image>` を解析すると空になる（仕様では `<img>`）。`<table><tr><td><image>`、`<table><caption><image>` は正しい。
- **範囲**：直すべき箇所は "in table" の実装だが、失われるのは "in body" の `image` の手順なので、ここに載せる。
- **確信度**：高い。

## 3. 未カバーの step

WPT の tree construction のデータで、その step を通る入力が見つからなかったもの。（逸脱の 5 件は、すべて未カバーの step にある。）

- Noah's Ark で、値のない属性と空の値の属性を比べる場合（F4）。
- `input` の `type` が大文字を含む "hidden" の場合（"in body" の規則。F3）。
- `textarea` の後の "text" mode で、active formatting elements が再構築を待っている場合（F2）。WPT の `textarea` のテスト（tests3.dat#17〜#19、tests19.dat#73 など）は、どれも再構築される要素がない。
- "in table"、"in table body"、"in row"、"in column group" からの `image`（F5）。WPT の `image` のテストは tests1.dat#90 と tests19.dat#93 の二つだけで、どちらも "in body"。
- AAA で、inner loop が innerLoopCounter > 3 で AFE の要素を除き、そのとき formattingElement より前の要素、または bookmark より前の要素を除く場合と、最初に作り直した要素の後ろに AFE の要素が残る場合（F1）。adoption01.dat#15、tests22.dat#1 は除去を通るが、添字のずれが見える形にならない。
- `form` 開始で、form element pointer があって template の外なので無視する場合（"in body" の中）。WPT の例（tests20.dat#46、#47、tests16.dat#197）は "in table" の `form`。`<form><div><form>x` を手で確かめ、仕様どおりだった。
- `</dd>`、`</dt>` で、要素が scope にあって閉じる場合。WPT は scope にない場合（tests1.dat#110）だけ。`<dd><div></dd>x` などを手で確かめた。
- context が `select` の fragment での `select` 開始タグ。`<select>x<option>y` を手で確かめた。
- scripting 無効での "in body" の `noscript` 開始タグ（noscript01.dat は "in head" の noscript だけ）。`<p><noscript><b>x</b></noscript>` を手で確かめた。
- AAA の step 9（formattingElement が current node でない parse error）は parse error だけで、木の形は変わらないので、テストでは区別できない。
- EOF、`</body>`、`</html>` の「一覧にない要素が stack にある」parse error は、実装が報告しない（表の注記）。木は変わらないので、テストでは区別できない。
