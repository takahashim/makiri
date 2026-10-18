# HTML パーサ監査：tree construction の中核（parse state、dispatcher、ノードの作成と挿入、initial 〜 after head）

- 仕様：HTML Standard `source` `de953a7cf0523f5873a1904ba2ce010c689dfb66`（行番号はこの `source` のもの）
- 実装：`ext/makiri/rust/src/html/tree/`（パスはこのディレクトリからの相対。`res/` は `ext/makiri/rust/src/html/res/`）
- テスト：WPT `html/syntax/parsing/resources/*.dat`（`file#n` は 1 始まりの通し番号。runner の番号と同じ）
- 既知の差（`docs/parser_differences.ja.md`）：P1、P2、P3、L1、S1 は新規として扱わない。
- 状態：一致／意図した差／未実装／逸脱。parse error の有無や種類だけの違いは備考に書き、逸脱にしない。
- 確認は、ビルド済みの拡張（`Makiri::HTML`、`Makiri::DocumentFragment.parse`）を html5lib 形式で dump して行った（`spec/conformance/html5lib_dump.rb`）。

表の生成物との照合（機械的に比較したもの）：

- special カテゴリ：`res/tree.rs` の `TAG_CATS` を復号し、仕様 142773 行の一覧と比較。差は HTML `image` が実装側にだけあること（仕様はコメントアウトしている。`<image>` は `img` になるので stack に載らず、観測できない）。
- scope 系の一覧（in scope／list item／button／table）と formatting：完全一致（`select` が in scope にあることも含む）。
- SVG 属性の調整表 58 件、foreign 属性の調整表 11 件：完全一致。
- quirks の public identifier の前方一致 55 件、完全一致 3 件、system 1 件、system 欠落時の 2 件、limited-quirks の 2+2 件：完全一致。

---

## 1. Parse state（142581–143116）

### 1.1 insertion mode と reset the insertion mode appropriately（142583–142728）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| insertion mode の一覧、original insertion mode、stack of template insertion modes | `mod.rs:72-96`（`Mode`）、`mod.rs:276-283`、`stacks.rs:138` | 一致 | 全般 | Lexbor の補助 mode `InBodySkipNewLine*` は in body の改行 1 つ読み飛ばしの実装手段で、mode としては in body と同じ扱い。"in select" 系は仕様からも消えており、実装にもない。 |
| reset step 1–3（last、node、fragment なら context に置き換え） | `actions.rs:536-547` | 一致 | `tests_innerHTML_1.dat` 全般 | |
| td/th かつ last でない → in cell | `actions.rs:557-561` | 一致 | `tests_innerHTML_1.dat`（ctx=td は last なので in body 側） | |
| tr → in row | `actions.rs:563-566` | 一致 | `tests_innerHTML_1.dat`（ctx=tr） | |
| tbody/thead/tfoot → in table body | `actions.rs:567-570` | 一致 | `tests_innerHTML_1.dat` | |
| caption → in caption | `actions.rs:571-574` | 一致 | `tests_innerHTML_1.dat` | |
| colgroup → in column group | `actions.rs:575-578` | 一致 | `tests_innerHTML_1.dat` | |
| table → in table | `actions.rs:579-582` | 一致 | `tests_innerHTML_1.dat`、`tables01.dat` | |
| template → current template insertion mode | `actions.rs:583-589` | 一致 | `template.dat`（#5 など） | template mode の stack が空なら mode を変えない（Lexbor の assert 相当。到達しない）。 |
| head かつ last → in body | `actions.rs:590-595` + `614-616` | 一致 | `tests_innerHTML_1.dat`（ctx=head） | |
| head かつ last でない → in head | `actions.rs:590-594` | 一致 | `template.dat#5`（`</template>` 後の reset） | |
| body → in body | `actions.rs:596-599` | 一致 | `tests_innerHTML_1.dat` | |
| frameset → in frameset | `actions.rs:600-603` | 一致 | `tests_innerHTML_1.dat`（ctx=frameset） | |
| html → head pointer で before head／after head | `actions.rs:604-611` | 一致 | `tests_innerHTML_1.dat`（ctx=html） | |
| last → in body | `actions.rs:614-617` | 一致 | `tests_innerHTML_1.dat` | |
| HTML 以外の node（仕様は「td element」などを HTML 要素の意味で使う） | `actions.rs:549-555` | 一致 | `foreign-fragment.dat` | foreign の node は飛ばし、last なら in body。 |

### 1.2 stack of open elements（142730–142921）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| current node、adjusted current node | `mod.rs:374-387` | 一致 | `foreign-fragment.dat` | |
| current node を stack から外すときの process internal resource links | `stacks.rs:23-27`（`pop_steps` は option の selectedcontent だけ） | 意図した差 | ― | 資源の取得をしないパーサなので対象外。 |
| special カテゴリ | `res/tree.rs` `TAG_CATS`、`actions.rs:32-37` | 一致 | `tests1.dat#103` など | 名前空間ごとの表。HTML `image` が余分にあるが観測不能（上記）。 |
| formatting カテゴリ | `res/tree.rs` `TAG_CATS` | 一致 | `adoption01.dat` | a, b, big, code, em, font, i, nobr, s, small, strike, strong, tt, u。 |
| has an element in the specific scope（target なら一致、一覧の要素なら失敗） | `actions.rs:622-658` | 一致 | 全般 | target の判定を一覧より先に行う。 |
| in scope の一覧（applet, caption, html, table, td, th, marquee, object, select, template, MathML 6 種, SVG 3 種） | `CAT_SCOPE`（`res/tree.rs`） | 一致 | `tests1.dat`、`tests10.dat` | |
| list item scope（+ HTML ol, ul） | `CAT_SCOPE_LIST_ITEM` | 一致 | `tests1.dat#103` | |
| button scope（+ HTML button） | `CAT_SCOPE_BUTTON` | 一致 | `tests1.dat` | |
| table scope（HTML html, table, template） | `CAT_SCOPE_TABLE` | 一致 | `tables01.dat`、`template.dat` | |

### 1.3 list of active formatting elements（142923–143034）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| marker（applet, object, marquee, template, td, th, caption で入れる） | `stacks.rs:153-156`（marker は `None`）、`modes/in_head.rs:115` ほか | 一致 | `tests1.dat#20`、`template.dat` | 本スライスでは template の marker を確認。 |
| push：Noah's Ark（最後の marker 以降に同名・同名前空間・同属性が 3 つあれば最古を除く） | `stacks.rs:250-315` | **逸脱** | `tests23.dat#1-4`、`adoption01.dat#17`（値のない属性はない） | 値のない属性（`<b x>`）と空の値の属性（`<b x="">`）を別の属性として比べる。仕様では両方とも値は ""。下の「逸脱と未実装」1。 |
| reconstruct step 1–2（空、最後が marker か stack 上なら終わり） | `stacks.rs:197-209` | 一致 | `adoption01.dat`、`tests1.dat` | |
| reconstruct rewind／advance／create | `stacks.rs:210-234` | 一致 | `adoption01.dat#17`、`tests23.dat` | create は「その token のために作られた要素」の属性を、元の要素から写す（`Tok::base`）。スクリプトがないので元の token の属性と同じ。 |
| clear up to the last marker | `stacks.rs:172-178` | 一致 | `template.dat`、`tests1.dat` | |

### 1.4 element pointers（143036–143052）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| head element pointer | `mod.rs:271`、`modes/before_head.rs:70-73` | 一致 | `tests3.dat#1-2`、`tests7.dat#3` | |
| form element pointer、parsing template contents | `mod.rs:270`、`mod.rs:361-372` | 一致 | `template.dat#117,#120,#124` | P2（移植版で修正済み）。 |

### 1.5 other parsing state flags（143054–143114）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| fragment context element | `mod.rs:268` | 一致 | `tests_innerHTML_1.dat` | |
| allow declarative shadow roots | なし | 意図した差 | ―（WPT pin に `shadowrootmode` のテストなし） | 常に false として振る舞う（DOMParser と同じ）。shadow root を表す DOM がない。 |
| scripting mode（Normal／Disabled／Inert／Fragment） | `mod.rs:281`（`scripting`、常に false） | 意図した差 | `noscript01.dat`（script-off のみ実行） | 文書も fragment も Disabled で解析する。仕様の fragment の既定は Inert（scripting 有効扱い）だが、非スクリプトのパーサとして Disabled に固定。 |
| frameset-ok flag（暗黙の body で "ok" に戻す） | `mod.rs:280`、`modes/after_head.rs:105` | 一致 | `template.dat#44-46` | P1（移植版で修正済み）。 |
| parser sanitizer configuration | なし | 意図した差 | ― | sanitizer を持たない（常に null）。null のときの分岐はすべて「何もしない」。 |
| insertion target redirection map | なし | 意図した差 | ― | sanitizer の replace-with-children 用と、fragment の root html を DocumentFragment へ向けるためのもの。前者は対象外、後者は fragment アルゴリズム（本スライス外）で html の子を取り出すのと結果が同じ。 |

---

## 2. Tree construction の dispatcher と integration point（145820–145908）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| stack が空 → insertion mode | `mod.rs:415-417` | 一致 | 全般（initial、before html） | |
| adjusted current node が HTML → insertion mode | `mod.rs:418-420` | 一致 | 全般 | |
| MathML text integration point かつ mglyph／malignmark 以外の start tag → insertion mode | `mod.rs:421-425` | 一致 | `tests10.dat#42-43`（mglyph／malignmark は foreign 側） | 実装は「close でない token」全体を通すので、comment／DOCTYPE／PI も insertion mode に行く。仕様では foreign content に行くが、foreign content と行き先の mode（in body、in table、in cell、in caption など）の扱いが同じ（comment と PI はその場所に挿入、DOCTYPE は無視）なので木は変わらない。 |
| MathML text integration point かつ character → insertion mode | `mod.rs:421-428` | 一致 | `plain-text-unsafe.dat#34,#38` | |
| annotation-xml かつ svg start tag → insertion mode | `mod.rs:430-436` | 一致 | `tests10.dat#52-53` | |
| HTML integration point かつ start tag → insertion mode | `mod.rs:437-441` | 一致 | `tables01.dat#17`、`tests20.dat#55-56`、`html5test-com.dat#14` | comment／DOCTYPE／PI の経路は上と同じで観測できない。 |
| HTML integration point かつ character → insertion mode | `mod.rs:437-441` | 一致 | `plain-text-unsafe.dat#42` | |
| EOF → insertion mode | `mod.rs:442-444` | 一致 | `tests10.dat` など | |
| それ以外 → foreign content | `mod.rs:445` | 一致 | `tests9.dat`〜`tests12.dat` | |
| MathML text integration point の定義（mi, mo, mn, ms, mtext） | `actions.rs:739-745` | 一致 | `plain-text-unsafe.dat#34,#38` | |
| HTML integration point の定義（annotation-xml の encoding が text/html か application/xhtml+xml、大小文字無視。SVG foreignObject, desc, title） | `actions.rs:748-775` | 一致 | `tests20.dat#54-56`、`plain-text-unsafe.dat#42` | fragment の context 要素は要素の属性を読む。fragment アルゴリズムの偽 token は context の属性を持つので同じ。 |

---

## 3. Creating and inserting nodes（145910–146578）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| appropriate place step 1–2（overrideTarget か current node） | `actions.rs:62-70` | 一致 | `adoption01.dat` | |
| foster parenting：lastTemplateOrTable が null → (html, null) | `actions.rs:87-89` | 一致 | `tests_innerHTML_1.dat`（ctx=table 系） | |
| foster parenting：lastTemplateOrTable が template → targetParent を template に | `actions.rs:82-86` | 一致 | `template.dat#91,#99` | |
| foster parenting：table の親が非 null → (親, table) | `actions.rs:90-93` | 一致 | `adoption01.dat#12`、`tables01.dat#19` | |
| foster parenting：table の親が null → stack で table の 1 つ上 | `actions.rs:94-96` | 一致 | 未カバー | スクリプトで table を外したときだけ起きる。非スクリプトのパーサでは到達しない。 |
| targetParent が template でなければ返す／template なら template contents | `actions.rs:101-107` | 一致 | `template.dat` | |
| template の insertion target（`for` による content patching）と insertion end marker | なし | **未実装** | 未カバー（WPT pin にテストなし） | 下の「逸脱と未実装」2。 |
| create an element：speculative mock element、custom element registry、definition、willExecuteScript、reactions | `actions.rs:157-172` | 意図した差 | ― | 非スクリプト・custom element 定義なし・投機的解析なし。 |
| create an element：sanitizer action と属性の sanitize | なし | 意図した差 | ― | sanitizer なし。 |
| create an element：token の属性を要素に追加 | `actions.rs:175-206` | 一致 | `tests10.dat`、`tests11.dat` | 重複属性は tokenizer が除く。調整後の名前で衝突しても残す点も仕様どおり。 |
| create an element：xmlns／xmlns:xlink の値が違えば parse error | なし | 一致 | ― | parse error を報告しないだけ。木は変わらない。 |
| create an element：resettable element の reset algorithm | なし | 意図した差 | ― | フォーム部品の value／checkedness を DOM に持たない。 |
| create an element：form element pointer への関連付け | なし | 意図した差 | ― | form owner を DOM に持たない（木の形は変わらない）。 |
| adjusted insertion location（redirection map がなければ location そのまま） | `actions.rs:62-108` | 一致 | 全般 | map は常に空。 |
| insert an element at the adjusted location：親あり／祖先／Document に要素の子があれば何もしない | なし | 一致 | 未カバー | スクリプトなしでは三条件とも成り立たない。 |
| insert an element：sanitizer の Replace with children／Remove | なし | 意図した差 | ― | sanitizer なし。 |
| insert an element：custom element reactions の queue | なし | 意図した差 | ― | custom element なし。 |
| insert a foreign element | `actions.rs:120-134` | 一致 | `tests10.dat`、`svg.dat`、`math.dat` | 場所を要素の作成前に一度だけ求める。スクリプトがないので作成後に求め直すのと同じ。 |
| insert an HTML element | `actions.rs:138-144` | 一致 | 全般 | |
| adjust MathML attributes（definitionurl → definitionURL） | `actions.rs:286-295` | 一致 | `tests19.dat#1`、`webkit02.dat#24-25` | |
| adjust SVG attributes（58 件） | `actions.rs:298-310`、`res/tree.rs` `SVG_ATTR_ADJUST` | 一致 | `tests11.dat#1-3` | 表は機械照合で一致。 |
| adjust foreign attributes（11 件） | `actions.rs:313-338`、`res/tree.rs` `FOREIGN_ATTR_ADJUST` | 一致 | `tests10.dat#23-26`、`webkit02.dat#23-24` | `xmlns` は接頭辞なし、名前空間だけ XMLNS。 |
| insert a character（Document なら捨てる、直前が Text なら連結、なければ Text を作る） | `actions.rs:341-368` | 一致 | `tables01.dat`、`adoption01.dat#12`、`tests1.dat` | Document が targetParent になる経路は非スクリプトでは到達しない。 |
| insert a comment（位置の指定があればそこ、なければ appropriate place） | `actions.rs:372-390` | 一致 | `comments01.dat`、`tests3.dat#3-4` | sanitizer の分岐は対象外。 |
| insert a processing instruction | `actions.rs:394-406` | 一致 | `processing-instructions.dat`（#115-117、#123 など） | 位置の指定を override target として appropriate place に通す。指定先は Document か html だけで、どちらも foster parenting の対象でないので結果は同じ。 |

---

## 4. Parsing elements that contain only text／Closing elements that have implied end tags（146580–146631）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| generic raw text／RCDATA（挿入、tokenizer の状態、original mode、text mode） | `actions.rs:452-477` | 一致 | `tests16.dat#90`、`html5test-com.dat#23`、`tests1.dat#55` | |
| generate implied end tags（dd, dt, li, optgroup, option, p, rb, rp, rt, rtc。除外指定あり） | `actions.rs:480-501` | 一致 | `tests1.dat#103`、`ruby.dat` | 一覧は仕様と同じ。実装はタグ名だけを見て名前空間を見ない（Lexbor どおり）。SVG の `option` などが current node のままこの手順に入るのは、foreign content の end tag が insertion mode に委ねる経路だけで、その後で必ず対象の HTML 要素まで pop するので木は変わらない（`<table><td><svg><option></td>x` で確認）。 |
| generate all implied end tags thoroughly（上記 + caption, colgroup, tbody, td, tfoot, th, thead, tr） | `actions.rs:504-533` | 一致 | `template.dat`（`</template>`） | 同上。 |

---

## 5. Insertion modes

### 5.1 initial（146636–146788）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白文字 → 無視 | `modes/initial.rs:122-127` | 一致 | `tests2.dat#51`（DOCTYPE 後は before html） ／ 先頭の空白は未カバー | token の先頭の空白を飛ばし、残りを anything else で再処理。1 文字ずつ処理するのと同じ。 |
| comment → (Document, null) | `modes/initial.rs:101-108` | 一致 | `comments01.dat` | |
| processing instruction → (Document, null) | `modes/initial.rs:109-116` | 一致 | `comments01.dat#12-13`、`processing-instructions.dat` | |
| DOCTYPE：parse error の条件 | `modes/initial.rs:147-150` | 一致 | `doctype01.dat` | parse error だけの差：public identifier が空文字列（欠落ではない）のときと、system identifier が 19 文字以外で `about:legacy-compat` でないときに報告しない。木は変わらない。 |
| DOCTYPE：doctype ノードの作成と追加 | `modes/initial.rs:139-153`、`actions.rs:412-449` | 一致 | `doctype01.dat`、`domjs-unsafe.dat#27` | 「Document に DocumentType か要素の子がなければ」の条件はない。initial では DOCTYPE を 1 つしか処理しないので成り立つ。 |
| quirks：force-quirks、name が html でない | `modes/initial.rs:161-162` | 一致 | `doctype01.dat`、`quirks01.dat` | `Makiri::HTML(...).quirks_mode` で確認（`<!DOCTYPE htmlx>` → 1）。 |
| quirks：public identifier が 3 件のどれか（大小文字無視） | `modes/initial.rs:14-18,176-179` | 一致 | `doctype01.dat` | `"HtMl"` → 1、`-//w3o//dtd w3 html strict 3.0//en//` → 1、末尾に文字が足されたもの → 0 を確認。 |
| quirks：system identifier が IBM の 1 件 | `modes/initial.rs:21,182-184` | 一致 | 未カバー | 大文字混じりで 1 を確認。 |
| quirks：public identifier の前方一致 55 件 | `modes/initial.rs:24-80` | 一致 | `doctype01.dat`（一部） | 機械照合で一致。 |
| quirks：system が欠落か空 + HTML 4.01 Frameset／Transitional | `modes/initial.rs:83-86,165` | 一致 | `doctype01.dat` | system `""` で 1 を確認。 |
| limited-quirks：XHTML 1.0 Frameset／Transitional、system あり + HTML 4.01 | `modes/initial.rs:89-92,170-195` | 一致 | `doctype01.dat` | 2 を確認。 |
| iframe srcdoc 文書と parser cannot change the mode flag | `modes/initial.rs:128`（コメント） | 意図した差 | ― | srcdoc 文書を解析しないので常に偽。 |
| before html へ | `modes/initial.rs:118` | 一致 | 全般 | |
| anything else（parse error、quirks、before html、再処理） | `modes/initial.rs:121-133` | 一致 | `tests1.dat#1` など多数 | `x` → quirks_mode 1 を確認。 |

### 5.2 before html（146790–146858）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| DOCTYPE → parse error、無視 | `modes/before_html.rs:19` | 一致 | `domjs-unsafe.dat#27` | |
| comment → (Document, null) | `modes/before_html.rs:20-27` | 一致 | `tests2.dat#35,#42` | |
| processing instruction → (Document, null) | `modes/before_html.rs:28-35` | 一致 | 未カバー（DOCTYPE 直後の PI） | `<!DOCTYPE html> <?y?>` で Document の子になることを確認。 |
| 空白文字 → 無視 | `modes/before_html.rs:45-50` | 一致 | `tests2.dat#51` | |
| start tag html | `modes/before_html.rs:36-43`、`80-84` | 一致 | `tests1.dat#9-10` など | push と追加の順序は逆だが結果は同じ。 |
| end tag head, body, html, br → anything else | `modes/before_html.rs:60` | 一致 | `tests1.dat#17-18` | |
| any other end tag → parse error、無視 | `modes/before_html.rs:61-64` | 一致 | `menuitem-element.dat#11` | |
| anything else（html を作って before head、再処理） | `modes/before_html.rs:69-77` | 一致 | `tests1.dat` 多数 | |

### 5.3 before head（146860–146936）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白文字 → 無視 | `modes/before_head.rs:36-41` | 一致 | `tests2.dat#51` | |
| comment → 挿入 | `modes/before_head.rs:18-22` | 一致 | `comments01.dat#16` | |
| processing instruction → 挿入 | `modes/before_head.rs:23-27` | 一致 | `processing-instructions.dat#116` | |
| DOCTYPE → parse error、無視 | `modes/before_head.rs:28` | 一致 | `domjs-unsafe.dat#28` | |
| start tag html → in body の規則 | `modes/before_head.rs:29` | 一致 | `tests14.dat#3-4` | |
| start tag head（挿入、head pointer、in head） | `modes/before_head.rs:30-35`、`70-73` | 一致 | `tests1.dat#86,#101` | |
| end tag head, body, html, br → anything else | `modes/before_head.rs:51` | 一致 | `tests1.dat#17-18` | |
| any other end tag → parse error、無視 | `modes/before_head.rs:52-55` | 一致 | `tests1.dat#110` | |
| anything else（属性なしの head を挿入、in head、再処理） | `modes/before_head.rs:60-67` | 一致 | `tests1.dat#1` など | |

### 5.4 in head（146938–147446）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白文字 → 挿入 | `modes/in_head.rs:63-71` | 一致 | `tests3.dat#3`、`webkit01.dat#36` | 先頭の空白だけを挿入し、残りを anything else へ。 |
| comment → 挿入 | `modes/in_head.rs:18-22` | 一致 | `tests19.dat#91` | |
| processing instruction → 挿入 | `modes/in_head.rs:23-27` | 一致 | `processing-instructions.dat#115` | |
| DOCTYPE → parse error、無視 | `modes/in_head.rs:28` | 一致 | `domjs-unsafe.dat#29` | |
| start tag html → in body の規則 | `modes/in_head.rs:29` | 一致 | `tests2.dat#55` | |
| base, basefont, bgsound, link（挿入、pop、self-closing の確認） | `modes/in_head.rs:30-36` | 一致 | `tests1.dat#86`、`tests19.dat#64` | |
| meta（挿入、pop、self-closing の確認） | `modes/in_head.rs:30-36` | 一致 | `tests19.dat#89` | |
| meta：charset／http-equiv による encoding の変更 | なし | 意図した差 | `tests19.dat#89`（木だけ） | 入力は Ruby String の encoding で解析前に UTF-8 に決まる（confidence は確定）。`Document#meta_encoding` は読むだけ。 |
| title → generic RCDATA | `modes/in_head.rs:37-41` | 一致 | `tests1.dat#55,#101` | |
| noscript（scripting が Disabled でない）→ generic raw text | `modes/in_head.rs:43-44` | 意図した差 | `noscript01.dat`（script-on は未実行） | 分岐はあるが、`scripting` は常に false。 |
| noframes, style → generic raw text | `modes/in_head.rs:54-58` | 一致 | `tests16.dat#90`、`html5test-com.dat#23`、`tests3.dat#1` | |
| noscript（scripting が Disabled）→ 挿入、in head noscript | `modes/in_head.rs:45-49` | 一致 | `noscript01.dat#1-18` | |
| script：要素の作成、挿入、push、script data state、original mode、text | `modes/in_head.rs:92-108` | 一致 | `tests3.dat#2`、`domjs-unsafe.dat#4` ほか多数 | |
| script：parser document、force async、Inert での already started、document.write の扱い | なし | 意図した差 | ― | スクリプトを実行しない。 |
| end tag head（pop、after head） | `modes/in_head.rs:80-83` | 一致 | `tests1.dat#86` ほか | |
| end tag body, html, br → anything else | `modes/in_head.rs:84` | 一致 | `tests1.dat#12,#16` | |
| start tag template：marker、frameset-ok、in template、template mode の push、要素の挿入 | `modes/in_head.rs:111-124` | 一致 | `template.dat#5,#44,#111-113` | 要素の挿入を先に行うが、スクリプトがないので順序の違いは観測できない。 |
| start tag template：shadowrootmode による declarative shadow root | なし | 意図した差 | ―（WPT pin にテストなし） | allow declarative shadow roots が常に false なので、仕様でも通常の template として挿入する。 |
| start tag template：sanitizer の Remove／Replace with children | なし | 意図した差 | ― | sanitizer なし。 |
| start tag template：`for` 属性による content patching（prepare content patching、find markers、template を挿入しない） | なし | **未実装** | 未カバー（WPT pin にテストなし） | 下の「逸脱と未実装」2。 |
| end tag template：stack に template がない → parse error、無視 | `modes/in_head.rs:128-134` | 一致 | `template.dat` | |
| end tag template：implied end tags thoroughly、pop until template、clear to marker、template mode の pop、reset | `modes/in_head.rs:135-146` | 一致 | `template.dat#5`、`template.dat#112` | |
| end tag template：template の insertion target があれば start／end marker を外して null に戻す | なし | **未実装** | 未カバー | 「逸脱と未実装」2 と同じ機能の後半。 |
| start tag head／any other end tag → parse error、無視 | `modes/in_head.rs:61,86` | 一致 | `tests3.dat#14`、`menuitem-element.dat#13` | |
| anything else（head を pop、after head、再処理） | `modes/in_head.rs:150-154` | 一致 | 多数 | |

### 5.5 in head noscript（147448–147519）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| DOCTYPE → parse error、無視 | `modes/in_head_noscript.rs:23` | 一致 | `noscript01.dat#1` | |
| start tag html → in body の規則 | `modes/in_head_noscript.rs:24` | 一致 | `noscript01.dat#2` | |
| end tag noscript（pop、in head） | `modes/in_head_noscript.rs:55-59` | 一致 | `noscript01.dat#3-5` | |
| 空白文字、comment、PI、basefont, bgsound, link, meta, noframes, style → in head の規則 | `modes/in_head_noscript.rs:25-32,36-44` | 一致 | `noscript01.dat#4-10`、`processing-instructions.dat#123` | 空白は in head と同じく直接挿入する。 |
| end tag br → anything else | `modes/in_head_noscript.rs:52-54` | 一致 | `noscript01.dat#12` | |
| start tag head, noscript／any other end tag → parse error、無視 | `modes/in_head_noscript.rs:33,60-61` | 一致 | `noscript01.dat#13-15` | |
| anything else（parse error、noscript を pop、in head、再処理） | `modes/in_head_noscript.rs:65-70` | 一致 | `noscript01.dat#16-18` | |

### 5.6 after head（147521–147627）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 空白文字 → 挿入 | `modes/after_head.rs:69-77` | 一致 | `tests6.dat#1`、`webkit01.dat#36` | |
| comment → 挿入 | `modes/after_head.rs:19-23` | 一致 | `tests3.dat#3-4` | |
| processing instruction → 挿入 | `modes/after_head.rs:24-28` | 一致 | `processing-instructions.dat#117` | |
| DOCTYPE → parse error、無視 | `modes/after_head.rs:29` | 一致 | `domjs-unsafe.dat#30` | |
| start tag html → in body の規則 | `modes/after_head.rs:30` | 一致 | `tests19.dat#92` | |
| start tag body（挿入、frameset-ok を not ok、in body） | `modes/after_head.rs:31-37` | 一致 | `tests1.dat#9-10` | |
| start tag frameset（挿入、in frameset） | `modes/after_head.rs:38-43` | 一致 | `domjs-unsafe.dat#40-41` | |
| base, basefont, bgsound, link, meta, noframes, script, style, template, title（head を push、in head の規則、head を stack から除く） | `modes/after_head.rs:44-67` | 一致 | `tests3.dat#1-2`、`tests7.dat#3`、`template.dat#46` | in head の戻り値を見ないのは Lexbor どおりで、これらのタグはどれも in head が消費する。 |
| end tag template → in head の規則 | `modes/after_head.rs:87` | 一致 | 未カバー（`</head></template>` の形はない） | |
| end tag body, html, br → anything else | `modes/after_head.rs:88` | 一致 | `tests15.dat#3-4` | |
| start tag head／any other end tag → parse error、無視 | `modes/after_head.rs:68,89-92` | 一致 | `tests6.dat#1`、`tests19.dat#3` | |
| anything else（属性なしの body、frameset-ok を ok、in body、再処理） | `modes/after_head.rs:100-110` | 一致 | `template.dat#44-46`、多数 | frameset-ok を戻す step は P1。 |

---

## 集計

| 状態 | 件数 |
|---|---|
| 一致 | 131 |
| 意図した差 | 17 |
| 未実装 | 3（同じ機能：template の `for` による content patching） |
| 逸脱 | 1 |

---

## 逸脱と未実装

### 1. 逸脱：Noah's Ark で、値のない属性と空の値の属性を別のものとして比べる

- **仕様**：142940–142962 行（push onto the list of active formatting elements）。属性は token のとおりに比べ、名前、名前空間、値がすべて等しければ同じ。tokenizer の規則で、値を書かない属性（`<b x>`）の値は空文字列なので、`<b x>` と `<b x="">` は同じ属性を持つ。
- **コード**：`stacks.rs:299-315`（`attr_compare`）。`valueless`（`mod.rs` の module doc と `actions.rs:208-230` の `make_attr`）で値を書かなかった属性を覚え、片方だけが値なしなら不一致とする。Lexbor の `lxb_dom_attr_compare` が NULL の値と空の値を区別するのを、そのまま移した。差分表（`docs/parser_differences.ja.md`）には載っていない。
- **再現**：`<p><b x><b x><b x=""><b x=""></p>x`
  - 仕様：4 つ目の push の時点で、最後の marker 以降に同じ `b` が 3 つあるので最古を除く。`</p>` の後の `x` で再構築されるのは 3 つ。

    ```
    | <html>
    |   <head>
    |   <body>
    |     <p>
    |       <b> x=""
    |         <b> x=""
    |           <b> x=""
    |             <b> x=""
    |     <b> x=""
    |       <b> x=""
    |         <b> x=""
    |           "x"
    ```

  - 実装：値なしの 2 つと空の値の 2 つが別の組になり、どちらも 3 つに達しないので、4 つとも再構築される（body の直下に `<b>` が 4 段、最内に `"x"`）。
  - 対照：`<p><b x=""><b x=""><b x=""><b x=""></p>x` と `<p><b x><b x><b x><b x></p>x` は、どちらも仕様どおり 3 段になる。
- **テスト**：WPT の Noah's Ark のテスト（`tests23.dat#1-4`、`adoption01.dat#16-17`）に、値のない属性を使うものはない。
- **確信度**：高い（コードの経路を追い、入力で確認した）。

### 2. 未実装：template の `for` 属性による content patching

- **仕様**：
  - in head の「A start tag whose tag name is "template"」の Keep 分岐（147315 行付近）：shadowrootmode が None で `for` 属性があれば、scope（adjusted insertion location の target parent。body なら html に置き換える）の中から `<?marker name=...?>` か `<?start name=...?>`（と対応する `<?end?>`）を探し（find markers、70048 行）、prepare content patching（69982 行）で template の insertion target を marker の親にし、start と end の間のノードを除く。成功したら template 要素そのものは挿入しない。
  - appropriate place for inserting a node（145965–145985 行）：targetParent が template で insertion target が非 null なら、(insertion target, insertion end marker) か (insertion target, null) を返す。
  - in head の「An end tag whose tag name is "template"」（147387 行）：insertion target が非 null なら start／end marker を外し、insertion target と marker を null に戻す。
  - いずれも scripting、allow declarative shadow roots、sanitizer によらず実行される。
- **コード**：`modes/in_head.rs:111-124`（`template`）、`modes/in_head.rs:127-147`（`template_closed`）、`actions.rs:101-107`（`appropriate_place` の template の分岐）。どこにも `for` 属性、insertion target、marker の扱いがない（Lexbor にもない機能）。
- **再現 1**：`<div><?marker name="x"?></div><template for="x"><p>new</p></template>`
  - 仕様：

    ```
    | <html>
    |   <head>
    |   <body>
    |     <div>
    |       <p>
    |         "new"
    ```

  - 実装：

    ```
    | <html>
    |   <head>
    |   <body>
    |     <div>
    |       <?marker name="x"?>
    |     <template>
    |       for="x"
    |       content
    |         <p>
    |           "new"
    ```

- **再現 2**：`<div><?start name="x"?>old<?end?></div><template for="x">new</template>`
  - 仕様：`| <div>` の子が `"new"` だけ（`old` は prepare content patching で除かれ、二つの marker は `</template>` で外れる）。template 要素はない。
  - 実装：`<div>` に `<?start name="x"?>`、`"old"`、`<?end ?>` が残り、body の子に `<template for="x">`（content に `"new"`）ができる。
- **テスト**：WPT pin（`5ffee20e`）の tree construction データにはない（`template for` の入力は 0 件）。
- **確信度**：中〜高。仕様の規定と実装に機能がないことは確実。PI の `name` の読み取りは DOM 仕様の「get a processing instruction attribute」に従い、`name="x"` は仕様の例と同じ書き方なので、期待する木もこの形になると判断した。processing-instruction token とともに新しく入った機能で、ブラウザの対応は確認していない。

---

## 未カバーの step

WPT の tree construction データ（pin `5ffee20e`）に、その step を通る入力が見つからなかったもの。

- foster parenting で、lastTemplateOrTable の table の親が null のとき（stack で table の 1 つ上を使う）：スクリプトで table を外したときだけ起きる。非スクリプトのパーサでは到達しない。
- insert an element at the adjusted insertion location の三つの早期 return（要素に親がある、要素が targetParent の祖先、Document に要素の子がある）：スクリプトなしでは到達しない。
- template の insertion target（`for` 属性）に関わる三つの step：未実装（上記 2）。
- initial の先頭の空白文字だけの token（`\A\s+` で始まる入力がない。`<!DOCTYPE html>` の後の空白は before html で `tests2.dat#51` がある）。
- initial の quirks 判定のうち、system identifier が IBM の 1 件と一致する分岐（手で確認した：`<!DOCTYPE html SYSTEM "http://www.ibm.com/data/dtd/v11/IBMXHTML1-transitional.dtd">` → quirks）。
- before html の processing instruction（DOCTYPE の直後の PI）：手で確認した（Document の子になる）。
- after head の `</template>`（in head の規則に委ねる）。
- Noah's Ark で値のない属性と空の値の属性を比べる場合（上記 1、逸脱）。
- declarative shadow root（`shadowrootmode`）：WPT pin にテストがない。実装は allow declarative shadow roots が false のときの仕様の動作と同じ。
- scripting が有効なときの分岐（noscript の raw text、script の already started など）：WPT の script-on のテストは実行しない（意図した差）。
