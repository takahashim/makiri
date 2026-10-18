# 監査: foreign content / the end / fragment の解析 / fragment の直列化

- 仕様: HTML Standard `source` @ `de953a7cf0523f5873a1904ba2ce010c689dfb66`（行番号はこの版の `source`）
- 実装: `/home/user/makiri` の作業木（出荷ビルド = `arena-dom`。`tmp/*/Makefile` の `RB_SYS_CARGO_FEATURES ?= arena-dom` で確認）
- 既知の差: `docs/parser_differences.ja.md`（P1–P3, L1, S1–S4, C1–C3, M1–M5, B1–B3）を先に読み、再掲していない。本節に関わるのは P3 のみ。
- 再現はすべて `PATH=/opt/rbenv/versions/3.3.6/bin:$PATH LANG=C.UTF-8 bundle exec ruby -Ilib -rmakiri -e '...'` で、ビルド済みの拡張に対して確かめた。
- WPT の番号は `spec/conformance/data/wpt/html/syntax/parsing/resources/<file>.dat` の中で `#data` を 1 から数えた順番。`[ctx=...]` は `#document-fragment`。
- コードのパスは `ext/makiri/rust/src/` からの相対パス。

## 1. The rules for parsing tokens in foreign content（source 150399–150614）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 振り分け（終了タグ: `script` / `p`,`br` / その他。開始タグ側: text, comment, PI, DOCTYPE, ブレイクアウトの一覧, その他） | `html/tree/modes/foreign_content.rs:183-259` | 一致（一覧の中身は下の行を参照） | 下の各行 | dispatcher（`html/tree/mod.rs:413-445`）は本節の範囲外。 |
| U+0000 の文字 token: parse error、U+FFFD を挿入 | `foreign_content.rs:98-117,148` | 一致 | `plain-text-unsafe.dat#11`〜`#24` | Lexbor は文字 token をまとめて扱う。置換後の文字列で挿入する。 |
| 空白の文字（TAB, LF, FF, CR, SPACE）: 挿入するだけ | `foreign_content.rs:121-127,148` | 一致 | `plain-text-unsafe.dat#24` ほか | frameset-ok を変えない。 |
| その他の文字: 挿入し、frameset-ok を "not ok" にする | `foreign_content.rs:121-127` | 意図した差（P3 として記録済み。出荷ビルドは仕様どおり） | `tests19.dat#51`, `plain-text-unsafe.dat#24` | `compat::fixed(Fix::P3)` の分岐。置換前の文字列で判定する。 |
| comment token: comment を挿入 | `foreign_content.rs:197-202`, `html/tree/actions.rs:373-391` | 一致 | `tests21.dat#21`, `#23` | |
| processing instruction token: PI を挿入 | `foreign_content.rs:203-208`, `actions.rs:395-407` | 一致 | 未カバー（WPT に foreign content 内の `<?` がない） | `<svg><?pi x?>` が `<svg><?pi x?></svg>` になることを確かめた。 |
| DOCTYPE token: parse error、無視 | `foreign_content.rs:209-212` | 一致 | `domjs-unsafe.dat#44` | |
| ブレイクアウトの開始タグ一覧（b, big, blockquote, body, br, center, code, dd, div, dl, dt, em, embed, h1–h6, head, hr, i, img, li, listing, menu, meta, nobr, ol, p, pre, ruby, s, small, span, strong, strike, sub, **sup**, table, tt, u, ul, var） | `foreign_content.rs:213-256` | **逸脱**（`sup` がない。F1） | 一覧の多く: `tests10.dat#12`〜`#15`, `tests9.dat`, `tests26.dat` など。`sup`: 未カバー | Lexbor の `foreign_content.c` は、コメントの一覧には `sup` を書いているが、`switch` に `LXB_TAG_SUP` がない。移植版はその `switch` をそのまま写している。 |
| `font` に `color`/`face`/`size` 属性があればブレイクアウト | `foreign_content.rs:157-168` | 一致 | `domjs-unsafe.dat#48`（属性あり）, `#45`（属性なし） | `<svg><FONT SIZE=1>` も抜けることを確かめた。属性なしの `font` は "any other start tag" へ進む。 |
| 終了タグ `br`, `p` をブレイクアウトとして扱う | `foreign_content.rs:191` | 一致 | `tests26.dat#11`〜`#20`, `foreign-fragment.dat#60`, `#61` [ctx=div] | |
| ブレイクアウト: parse error。current node が MathML text integration point、HTML integration point、HTML 要素のどれかになるまで pop | `foreign_content.rs:169-178`, `actions.rs:739-776` | 一致 | `tests9.dat`, `tests10.dat`, `tests26.dat` | annotation-xml の判定は token ではなく要素の `encoding` 属性を読むが、解析中は同じ値になる。 |
| ブレイクアウト: 現在の insertion mode で再処理 | `foreign_content.rs:179` | 一致 | 同上 | |
| any other start tag: adjusted current node が MathML なら adjust MathML attributes（`definitionurl` → `definitionURL`） | `foreign_content.rs:60-61`, `actions.rs:196-198,277-296` | 一致 | `tests19.dat#1`, `webkit02.dat#23`〜`#25` | SVG 上と HTML 上の `definitionurl` は変えないことを確かめた。 |
| any other start tag: SVG なら要素名を表で直す（37 項目） | `foreign_content.rs:70-80`, `html/res/tree.rs:221-`（`SVG_FIXNAME`） | 一致 | `tests11.dat#1`〜`#12`, `foreign-fragment.dat` ほか。`feDropShadow` など一部は未カバー | 仕様の表の 37 項目すべてを入力して確かめた。MathML の下では変えない（表の名前を `<math>` に入れると小文字のまま）。仕様が注釈にしている `solidcolor` は直さない。 |
| any other start tag: SVG なら adjust SVG attributes（58 項目） | `foreign_content.rs:62-63`, `actions.rs:299-310`, `res/tree.rs:425-484` | 一致 | `tests11.dat#1`〜`#4` | 表を 1 項目ずつ照合し、大文字で書いた 58 属性すべてを入力して確かめた。MathML と HTML の要素では変えない。 |
| any other start tag: adjust foreign attributes（11 項目） | `actions.rs:277-283,313-336`, `res/tree.rs:491-503` | adjusted current node が SVG か MathML のときは一致。それ以外の名前空間では**逸脱**（F5b） | `tests9.dat#24`〜`#27`, `tests10.dat#23`〜`#26`, `webkit02.dat#23`。`xmlns:xlink`, `xlink:arcrole` などは未カバー | 表を 1 項目ずつ照合した（xlink 7 項目、xml 2 項目、xmlns 2 項目。名前空間 id 5/6/7）。属性の名前空間 URI、localName、qualified name を XPath で確かめた。調整は `before_append_attr` を通してしか走らず、それは SVG と MathML でだけ設定される（`foreign_content.rs:60-64`）。 |
| insert a foreign element（adjusted current node の名前空間で） | `foreign_content.rs:65`, `actions.rs:120-133` | 一致 | WPT の foreign 系全般 | |
| self-closing で、tag が `script` かつ新しい current node が SVG: 認めたうえで script 終了タグの手順へ | `foreign_content.rs:82-90` | 一致 | 未カバー | `<svg><script/>a</svg>` が `<svg><script></script>a</svg>` になる。`<math><script/>` も pop される。 |
| self-closing のその他: pop して認める | `foreign_content.rs:91` | 一致 | `tests10.dat#25`, `#26`, `tests11.dat#9`〜`#13`, `tests19.dat#19`, `#20` | self-closing の「認めた」ことによる parse error の有無は確かめていない（出力に影響しない）。 |
| 終了タグ `script` で current node が SVG `script`: pop | `foreign_content.rs:43-52,190` | 一致 | `tests10.dat#40` | |
| 同上: insertion point、script nesting level、parser pause、SVG script の処理 | なし | 意図した差（scripting も document.write もない） | — | |
| any other end tag 1–2: node = current node。小文字化した名前が違えば parse error | `foreign_content.rs:24-27` | 一致（parse error の条件だけ小さな差） | `foreign-fragment.dat#2`〜`#9` [ctx=svg …] | tag id で比べる（id は小文字化した名前ごとに振られる）。スタックに要素が一つだけのときは parse error を報告しない。出力は変わらない。 |
| any other end tag 3（loop）: node がスタックの先頭なら return（fragment case） | `foreign_content.rs:28-39` | 一致（同じ結果） | `foreign-fragment.dat` [ctx=svg path] など | 実装は先頭の要素に達すると insertion mode に渡す。先頭は常に HTML の `html` 要素なので、"in body" などの規則も同じく無視する（`</svg>`, `</body>`, `</html>`, `</template>`, `</form>` を確かめた）。 |
| any other end tag 4: 名前が一致すれば、node まで pop して return | `foreign_content.rs:30-33`, `stacks.rs:89` | 一致 | `tests10.dat`, `tests11.dat` | |
| any other end tag 5–6: 一つ前へ進み、HTML 要素でなければ loop へ | `foreign_content.rs:34-37` | 一致 | 同上 | |
| any other end tag 7: HTML 要素に達したら、現在の insertion mode で処理 | `foreign_content.rs:39` | 一致 | `tests10.dat#34` など | |

## 2. The end（source 150619–150780）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 1. active speculative HTML parser があれば止めて return | なし | 意図した差（speculative parser がない） | — | |
| 2. insertion point を undefined にする | なし | 意図した差（`document.write` がない） | — | |
| 3. current document readiness を "interactive" にする | なし | 意図した差（readiness がない） | — | |
| 4. スタックのノードをすべて pop する（文書の解析） | `html/tree/parse.rs:87-89`, `html/tree/stacks.rs:105-109`, `html/tree/select.rs:88-96` | 一致 | `webkit02.dat#45`〜`#47`（EOF で開いたままの option が selectedcontent に写る） | pop ごとに option の pop steps（selectedcontent の更新）が走る。 |
| 4'. 同じ pop を fragment の解析で行う | `html/tree/fragment.rs:72-92`（pop がない） | **逸脱**（F2） | 未カバー | Lexbor の `lxb_html_parse_fragment_chunk_end` も pop しない。 |
| 5. 解析後に実行する script の一覧を処理する | なし | 意図した差（scripting がない） | — | |
| 6. DOMContentLoaded の task（load timing、イベント、ServiceWorker、WebDriver BiDi） | なし | 意図した差（イベントも browsing context もない） | — | |
| 7–8. ASAP script を待ち、load event を遅らせるものを待つ | なし | 意図した差 | — | |
| 9. load の task（readiness "complete"、load、pageshow、navigation timing） | なし | 意図した差 | — | readiness も Window もない。 |
| 10–11. print when loaded、post-load tasks | なし | 意図した差 | — | |
| abort a parser（入力を捨て、pop し、readiness を変える） | `html/tree/mod.rs:389-402`（`process_abort`） | 意図した差 | — | 仕様の abort は `document.open()` や navigation から呼ばれ、Makiri には呼び出し元がない。`process_abort` は Lexbor 由来で、内部の失敗（OOM）でスタックを空にし、解析を失敗として返す（`parse.rs:90-92`）。仕様の abort とは別物。 |

## 3. Parsing HTML fragments（source 151752–151913）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| 1. scriptingMode が Inert か Fragment であることを assert | なし | 意図した差（scripting がない） | — | |
| 2–3. context は target（Element）か、target の host。null でないこと | `bridge/fragment.rs:42-83`, `lexbor/arena_fragment.rs:117-149` | 意図した差（API） | `spec/fragment_context_spec.rb` | context は要素ノードか名前の String で受ける（`"svg"`/`"math"` は foreign の根、それ以外は HTML 名前空間）。nil は `<body>`（Nokogiri 互換）。shadow root がないので host はない。 |
| 4. type が "html" の新しい Document | `html/tree/fragment.rs:46-48` | 一致 | `tree_port_tests::the_port_parses_fragments_as_lexbor_does` | |
| 5. contextDocument の quirks / limited-quirks を新しい文書に写す | `html/tree/fragment.rs:47`（常に `Document::new(0)` = no-quirks） | **逸脱**（F3） | 未カバー（WPT の fragment テストは context の文書の mode を持たない） | |
| 6. contextDocument の scripting が無効なら scriptingMode を Disabled にする | `fragment.rs:77`（`set_state_by_tag(false, …)`） | 一致 | — | 文書は常に scripting 無効。 |
| 7. fragment context element を持つ parser を作る | `fragment.rs:54,56-57,63` | context は tag と名前空間だけで作られる。属性がない点は**逸脱**（F4）。静的でない名前空間の点は**逸脱**（F5） | `foreign-fragment.dat#1`〜`#61` | sanitizer の設定と allowDeclarativeShadowRoots はない（意図した差）。 |
| 8. tokenizer の状態: title, textarea → RCDATA | `html/tokenizer/mod.rs:537-540`, `fragment.rs:77-78` | 一致 | `plain-text-unsafe.dat#47`, `#48`, `tests4.dat#2`, `#3`, `#8` | `tmp_tag_id` を消すので、`</title>` は appropriate end tag にならない（確かめた）。 |
| 8. style, xmp, iframe, noembed, noframes → RAWTEXT | `tokenizer/mod.rs:541-548` | 一致 | `plain-text-unsafe.dat#49`, `#50`, `tests4.dat#4`。iframe, noembed, noframes は未カバー | 五つすべてを入力して確かめた。 |
| 8. script → script data | `tokenizer/mod.rs:549-552` | 一致 | `plain-text-unsafe.dat#51`, `tests4.dat#9` | |
| 8. noscript → scripting mode が Disabled でなければ RAWTEXT、Disabled なら data | `tokenizer/mod.rs:553-560` | 一致（常に Disabled なので data） | 未カバー | `<b>` が要素になることを確かめた。 |
| 8. plaintext → PLAINTEXT | `tokenizer/mod.rs:561` | 一致 | `plain-text-unsafe.dat#52`, `tests4.dat#5` | |
| 8. その他の要素（HTML 以外の名前空間の context を含む） → data | `tokenizer/mod.rs:532-535,562` | 一致 | `foreign-fragment.dat` | |
| 9. 要素 `html` を作る（custom element registry を引く） | `fragment.rs:50-52` | 一致（registry はない） | 全 fragment テスト | |
| 10. root を document に append | `fragment.rs:53` | 一致 | 同上 | |
| 11. スタックを root だけにする | `fragment.rs:58` | 一致 | 同上 | |
| 12. fragment を作る（configuration があれば inert な document に、なければ target の node document に） | `bridge/fragment.rs:147-157,202-233` | 一致 | `spec/fragment_context_spec.rb` | configuration（sanitizer）はない。 |
| 13. insertion target redirection map[root] = (fragment, null) | `lexbor/arena_fragment.rs:213-230` | 一致（実装の方式が違う） | 全 fragment テスト | root の下に解析し、終わってから root の子を fragment に写す。木の形は同じになる。 |
| 14. context が template なら "in template" を template insertion mode のスタックに push | `fragment.rs:59-62` | 一致 | `template.dat#116`, `#118`, `#120`, `#124` [ctx=template] | HTML 名前空間の template だけ。 |
| 15. context の start tag token（名前と属性）を作り、integration point の判定などに使う | `fragment.rs:54`（属性なしで作る） | **逸脱**（F4） | `foreign-fragment.dat#13` [ctx=math annotation-xml]（属性のない場合だけ） | |
| 16. insertion mode を適切にリセットする（fragment case では context を使う） | `fragment.rs:63-64`, `actions.rs:536-620` | 一致 | `tests_innerHTML_1.dat`（[ctx=td/tr/table/caption/colgroup/tbody/select/html/frameset/head/body]）, `tests6.dat`, `math.dat`, `svg.dat` | |
| 17. form element pointer: context 自身か祖先の最も近い form | `fragment.rs:65-70`（context が form のときだけ） | context 自身なら一致。祖先なら**逸脱**（F6） | 未カバー（WPT に [ctx=form] がない） | `<form>` が context のとき `<form><input></form>x` が `<input>x` になることを確かめた（一致）。 |
| 18. 入力を input stream に置く（confidence は無関係） | `fragment.rs:42-43,74` | 一致 | — | 不正な UTF-8 は先に U+FFFD にする（`utf8_input`、本節の外）。 |
| 19. 入力を使い切るまで parser を動かす | `fragment.rs:80-83` | 一致（"the end" の pop は F2） | 全 fragment テスト | |
| 20. fragment を target の node document に adopt | `lexbor/arena_fragment.rs:213-230,236-255` | 一致 | `spec/cross_import_spec.rb`, `spec/fragment_context_spec.rb` | ノードを写す（template の contents を含む）。名前空間を与えなかった要素には、import のときに context の名前空間を与える。 |
| 21. fragment を返す | `bridge/fragment.rs:202-233`（`Document#fragment`, `DocumentFragment.parse`）, `:147-197`（`inner_html=`, `outer_html=`, template） | 一致 | `spec/html_standard_steps_spec.rb`, `spec/mutation_spec.rb` | |
| foreign の context（svg, math, foreignObject, desc, title, annotation-xml, mi/mo/mn/ms/mtext, malignmark） | `fragment.rs:54,63`, `html/tree/mod.rs:382-387`（adjusted current node） | 一致（F4、F5 を除く） | `foreign-fragment.dat#1`〜`#61` | 名前の String で選べる foreign の context は svg と math だけ。ほかは要素ノードで渡す。 |

## 4. Serializing HTML fragments（source 151359–151750）

| step（節と分岐） | コード | 状態 | テスト | 備考 |
|---|---|---|---|---|
| serializes as void の定義（void 要素 + basefont, bgsound, frame, keygen, param） | `html/dom/serialize.rs:143-172` | 一致 | `lexbor::tests::serialize_walk::ours_write_what_lexbor_writes`, `lexbor::arena_tests`（Lexbor と同じ出力になることだけを確かめる）。5 つの追加要素は未カバー | HTML 名前空間で、18 の名前。 |
| 1. node が serializes as void なら空文字列を返す | `serialize.rs:320-345`（`write`、`deep` のとき） | **逸脱**（F7） | 未カバー | |
| 2. s を空にする | — | 一致 | — | |
| 3. node が template なら、代わりに template contents を使う | `serialize.rs:336-339,354` | 一致 | `spec/template_contents_contract_spec.rb`, `spec/html_standard_steps_spec.rb` | template 自身の子は書かない（確かめた）。 |
| 4. node が shadow host なら、宣言的 shadow root の `<template shadowrootmode=…>` を書く（serializable, delegatesfocus, slotassignment, clonable, customelementregistry） | なし | 意図した差（shadow DOM がない） | — | 到達しない。 |
| 5. 子ごと / Element: HTML, MathML, SVG の名前空間なら tagname = local name、ほかは qualified name | `serialize.rs:228-236,305-315`（常に `qualified_name()`） | 接頭辞がなければ一致。接頭辞があれば**逸脱**（F8） | `spec/serialize_spec.rb:34`（urn:x の名前空間） | |
| 5. Element: `<` + tagname | `serialize.rs:229-230` | 一致 | `spec/serialize_spec.rb` | |
| 5. Element: is value があり、`is` 属性がなければ ` is="…"` を書く | なし | 意図した差（is value がない） | — | `create_element` に `is` の option がなく、parser は `is` を属性として持つ。到達しない。 |
| 5. Element: 属性ごとに ` 名前="値（attribute mode で escape）"` | `serialize.rs:200-225,231-234` | 一致 | `lexbor::arena_tests`, `spec/serialize_spec.rb` | |
| 5. 属性の serialized name: 名前空間なし → local name / XML → `xml:`+local / XMLNS かつ local が `xmlns` → `xmlns` / XMLNS → `xmlns:`+local / XLink → `xlink:`+local / ほか → qualified name | `serialize.rs:201-221` | 一致 | 未カバー（Lexbor との比較だけ） | `set_attribute_ns` で、XML（接頭辞 `foo:`）、XMLNS、XLink（接頭辞 `zz:`）、`urn:o`、HTML 名前空間の属性を試し、`xml:lang`, `xmlns:a`, `xlink:href`, `o:at`, `h:at` を確かめた。 |
| 5. 属性の順序は安定 | 属性リストの順（`serialize.rs:231`） | 一致 | — | |
| 5. `>` を書き、void なら次の子へ | `serialize.rs:235,352-356,309-311` | 一致 | `spec/serialize_spec.rb` | void の子は書かない（`br.to_html` は `<br>`）。 |
| 5. 子を再帰して書き、`</tagname>` | `serialize.rs:348-376`（反復） | 一致 | `spec/serialize_spec.rb`（round trip） | |
| Text: 親が style, script, xmp, iframe, noembed, noframes, plaintext か、scripting 有効の noscript なら data をそのまま書く | `serialize.rs:176-195,244-245` | 一致 | `spec/serialize_spec.rb:34`（HTML 以外の script は escape する）, `spec/mutation_spec.rb` | 親が HTML 名前空間のときだけ。noscript は scripting が常に無効なので escape する（確かめた）。 |
| Text: ほかは escape する | `serialize.rs:247` | 一致 | `spec/serialize_spec.rb:29` | |
| CDATASection（Text の一種） | `serialize.rs:268`（`Err(Stopped)`） | 一致（到達しない） | — | HTML の文書に CDATA は作れず、XML のノードも HTML の木に入れられない（`TypeError` を確かめた）。 |
| Comment: `<!--` + data + `-->` | `serialize.rs:250-254` | 一致 | `lexbor::arena_tests` | data は escape しない（`c--x` を確かめた）。 |
| ProcessingInstruction: `<?` + target + 空白 + data + `?>` | `serialize.rs:255-261` | 一致 | `spec/mutation_spec.rb:109` | |
| DocumentType: `<!DOCTYPE ` + name + `>` | `serialize.rs:262-266` | 一致 | `spec/document_spec.rb` など | public id と system id は書かない（確かめた）。 |
| Escaping: `&` → `&amp;`, U+00A0 → `&nbsp;`, `<` → `&lt;`, `>` → `&gt;`、attribute mode では `"` → `&quot;` | `serialize.rs:58-78` | 一致 | `spec/serialize_spec.rb:29,34` | attribute mode でも `<` と `>` を escape する。この版の仕様のとおりで、Lexbor 05b5d37 も同じ（`serialize_walk` と一致）。 |
| （拡張）`pretty: true` の書式 | `serialize.rs:80-131,272-301,379-431` | 意図した差（Lexbor の拡張で、仕様にない） | `spec/serialize_spec.rb:50-62` | |

## 逸脱と未実装

未実装として独立に挙げる項目はない。仕様の手順のうち scripting、browsing context、shadow DOM、is value、sanitizer に関わるものは、Makiri がその概念を持たないので「意図した差」とした。以下の 8 件は、すべて入力で確かめた。

### F1 foreign content のブレイクアウト一覧に `sup` がない

- **仕様**: source 150446–150456。一覧に `"sub", "sup"`。
- **コード**: `html/tree/modes/foreign_content.rs:213-256`。`TAG_SUB` の次が `TAG_TABLE`。Lexbor の `insertion_mode/foreign_content.c` も同じで、コメントの一覧には `sup` を書いているが、`switch` にない。
- **再現**: `Makiri::HTML("<svg><sup>x</sup></svg>")`
  - 仕様: `<svg></svg><sup>x</sup>`（`sup` は HTML 名前空間）
  - 実装: `<svg><sup>x</sup></svg>`（`namespace-uri()` は `http://www.w3.org/2000/svg`）
  - `<math><sup>` も同じ。
- **影響**: SVG や MathML の中の `<sup>` が foreign の要素のまま残り、後続の HTML も中に閉じ込められる。WPT にこの入力はない。
- **確信度**: 高。

### F2 fragment の解析が "the end" の「スタックのノードをすべて pop する」を行わない

- **仕様**: source 150636（The end の step 4）。EOF の "stop parsing" から fragment の parser でも走る。pop steps は source 58983–58986（option が pop されると、子孫の selectedcontent を更新する）。
- **コード**: `html/tree/fragment.rs:72-92`。`tkz.end()` の後に `open_pop_all()` がない。文書の解析は `parse.rs:89` で pop する。Lexbor の `lxb_html_parse_fragment_chunk_end` も pop しない。
- **再現**: `Makiri::DocumentFragment.parse("<select><button><selectedcontent></selectedcontent></button><option>A").to_html`
  - 仕様: `<select><button><selectedcontent>A</selectedcontent></button><option>A</option></select>`（文書として解析したときと同じ）
  - 実装: `<select><button><selectedcontent></selectedcontent></button><option>A</option></select>`
  - `</option></select>` まで書けば `A` が写る。`inner_html=` でも同じ。
- **確信度**: 中〜高。fragment の parser でも EOF の "stop parsing" から the end が走ると読んだ。観測できる差は option の pop steps だけ。

### F3 fragment の文書が context の文書の quirks mode を受け継がない

- **仕様**: source 151788–151795。contextDocument が quirks なら quirks、limited-quirks なら limited-quirks。
- **コード**: `html/tree/fragment.rs:46-47`（常に `Document::new(0)`）。`parse_fragment` は context の文書の mode を受け取らない（`lexbor/arena_fragment.rs:169-202`）。
- **再現**:
  ```ruby
  d = Makiri::HTML("<p>quirks</p><div id=a></div>")   # doctype がない → quirks
  a = d.at_css("#a"); a.inner_html = "<p><table></table>"; a.inner_html
  ```
  - 仕様: `<p><table></table></p>`（quirks mode では `<table>` が `p` を閉じない。同じ文書の本文でも `<p><table>` は入れ子になる）
  - 実装: `<p></p><table></table>`
  - `Makiri::HTML("<p>x").fragment("<p><table></table>")` も同じ。`DocumentFragment.parse` の裏の文書（`bridge/fragment.rs:237-245` の `<html><body></body></html>`）は doctype がないので quirks であり、直すとこの API の答えも変わる点に注意。
- **確信度**: 高。

### F4 fragment の context が属性を持たない（annotation-xml の `encoding`）

- **仕様**: source 151878–151886。context の local name と属性から start tag token を作り、HTML integration point の判定などに使う。
- **コード**: `html/tree/fragment.rs:54`。context を tag と名前空間だけで作る。`html_integration_point`（`actions.rs:748-770`）は要素の `encoding` 属性を読むので、context の annotation-xml は integration point にならない。
- **再現**:
  ```ruby
  d = Makiri::HTML('<!doctype html><math><annotation-xml encoding="text/html"></annotation-xml></math>')
  ax = d.at_xpath('//*[local-name()="annotation-xml"]')
  ax.inner_html = "<foo></foo>"
  ax.children.map { |c| c.xpath("namespace-uri()") }
  ```
  - 仕様: `foo` は HTML 名前空間（context が HTML integration point なので、開始タグを HTML の規則で処理する）
  - 実装: `http://www.w3.org/1998/Math/MathML`
  - 文書として解析した `<annotation-xml encoding="text/html"><foo>` の `foo` は HTML になり、こちらは一致する。
- **確信度**: 高。

### F5 静的でない名前空間の context: CDATA と adjust foreign attributes

- **仕様**: context が HTML 以外の名前空間なら adjusted current node はその要素なので、(a) markup declaration open state の `[CDATA[` は CDATA section になり（tokenizer の節）、(b) "any other start tag" は名前空間によらず adjust foreign attributes を行う（source 150515–150516）。
- **コード**:
  - (a) `lexbor/arena_fragment.rs:97-103`。`parse_ns` は静的でない名前空間に 0（`NS__UNDEF`）を渡す。tokenizer は `ns != NS_HTML && ns != NS__UNDEF` のときだけ CDATA を認める（`html/tokenizer/state.rs:652-653,715-716`）。モジュールの doc comment（`arena_fragment.rs:7-12`）は「名前空間が解析に影響しない」と書くが、実際には影響する。
  - (b) `foreign_content.rs:60-64`。`before_append_attr` は MathML と SVG でしか設定されず、adjust foreign attributes はその中からしか呼ばれない（`actions.rs:196-198,277-283`）。
- **再現**:
  ```ruby
  d = Makiri::HTML("<!doctype html><body>")
  f = d.create_element_ns("urn:x", "q:foo"); d.at_css("body") << f
  f.inner_html = '<bar xlink:href="a"/><![CDATA[x<y]]>'
  f.to_html
  ```
  - 仕様: `bar` の `xlink:href` は XLink 名前空間。CDATA は text `x<y` になり、直列化は `x&lt;y`。
  - 実装: `xlink:href` は名前空間なし（`namespace-uri` が `""`、local-name が `xlink:href`）。CDATA は comment `<!--[CDATA[x<y]]-->` になる。
  - `bar` が `urn:x` 名前空間になる点は一致する（import のときに与える）。
- **確信度**: 高（(b) は、静的な名前空間を渡せても `foreign_content.rs` の分岐を直さなければ残る）。

### F6 form element pointer が context の祖先の form を見ない

- **仕様**: source 151895–151900。context 自身か、祖先をたどって最も近い form 要素。
- **コード**: `html/tree/fragment.rs:65-70`。context の tag が form のときだけ。`FragmentContext::element`（`lexbor/arena_fragment.rs:119-138`）は祖先を option guard のためにしか読まない。
- **再現**:
  ```ruby
  d = Makiri::HTML("<!doctype html><form><div id=a></div></form>")
  a = d.at_css("#a"); a.inner_html = "<form><input></form>x"; a.inner_html
  ```
  - 仕様: `<input>x`（pointer が祖先の form を指すので、`<form>` 開始タグと `</form>` は無視される）
  - 実装: `<form><input></form>x`
- **確信度**: 高。

### F7 void の要素の inner_html（fragment の直列化 step 1）

- **仕様**: source 151378。node が serializes as void なら空文字列を返す。
- **コード**: `html/dom/serialize.rs:320-345`。`deep` の経路は node の種類を見ずに子を書く。subtree の中の void は `shape` で子を飛ばす。
- **再現**:
  ```ruby
  d = Makiri::HTML("<!doctype html><br>")
  br = d.at_css("br"); br << d.create_text_node("x"); br << d.create_element("i")
  br.inner_html
  ```
  - 仕様: `""`
  - 実装: `"x<i></i>"`（`br.to_html` は `<br>` で、こちらは一致）
- **確信度**: 高（DOM の操作で void の要素に子を与えたときだけ）。

### F8 接頭辞のある HTML / SVG / MathML 要素の tagname

- **仕様**: source 151476–151478。HTML、MathML、SVG の名前空間の要素は local name を書き、ほかは qualified name を書く。
- **コード**: `html/dom/serialize.rs:230,313`。常に `qualified_name()`（Lexbor と同じ）。
- **再現**:
  ```ruby
  d = Makiri::HTML("<!doctype html><p></p>")
  d.at_css("p") << d.create_element_ns("http://www.w3.org/2000/svg", "s:rect")
  d.at_css("p") << d.create_element_ns("http://www.w3.org/1999/xhtml", "h:div")
  d.at_css("p").inner_html
  ```
  - 仕様: `<rect></rect><div></div>`（MathML の `m:mi` は `<mi></mi>`、HTML の `h:br` は `<br>`）
  - 実装: `<s:rect></s:rect><h:div></h:div>`（`<m:mi></m:mi>`、`<h:br>`）
  - `urn:x` の `q:foo` が `<q:foo></q:foo>` になる点は一致する。
- **確信度**: 高（`create_element_ns` で接頭辞を付けたときだけ。parser はこの形を作らない）。

## 未カバーの step

WPT、`spec/*.rb`、Rust のテストのどれも、仕様が定める答えとして押さえていない step。（`lexbor::arena_tests` と `serialize_walk` は Lexbor と同じ出力になることしか確かめないので、Lexbor と仕様が食い違う箇所の保証にはならない。）

- foreign content
  - ブレイクアウトの `sup`（F1）
  - foreign content の中の processing instruction
  - SVG の中の self-closing `<script/>`
  - SVG の名前の表のうち WPT にないもの（`feDropShadow` など）
  - adjust foreign attributes の `xmlns:xlink`, `xlink:arcrole`/`role`/`show`/`title`/`type`/`actuate`
  - SVG と MathML 以外の名前空間での adjust foreign attributes（F5b）
- The end
  - fragment の解析での pop all（F2）
- fragment の解析
  - quirks / limited-quirks の継承（F3）
  - context の属性（F4）
  - 静的でない名前空間の context（F5）
  - form element pointer（context が form のときも、祖先が form のときも。F6）
  - iframe / noembed / noframes / noscript の context の tokenizer 状態
- fragment の直列化
  - void の node の inner_html（F7）
  - 接頭辞のある要素の tagname（F8）
  - 属性の serialized name の名前空間ごとの分岐（仕様に照らしたテストがない）
  - basefont / bgsound / frame / keygen / param を void として扱うこと
  - noscript の text を escape すること
  - `<`/`>` を attribute mode で escape すること（文書の escape は `spec/serialize_spec.rb:29` がある）

## 集計

| 節 | 一致 | 意図した差 | 逸脱 | 未実装 |
|---|---|---|---|---|
| foreign content（26 行） | 22 | 2（P3、SVG script の scripting 部分） | 2（F1、F5b） | 0 |
| The end（11 行） | 1 | 9 | 1（F2） | 0 |
| fragment の解析（26 行） | 20 | 2（scripting の assert、context の API） | 4（F3、F4/F5 の step 7、F4 の step 15、F6） | 0 |
| fragment の直列化（21 行） | 16 | 3（shadow root、is value、pretty） | 2（F7、F8） | 0 |
| 計（84 行） | 59 | 16 | 9 | 0 |

- 行の中で一部だけが逸脱するもの（adjust foreign attributes、form element pointer、tagname）は、両方の状態を書いた。上の表では逸脱として数えた。
- 異なる逸脱は 8 件（F1–F8。F5 は (a) CDATA と (b) foreign attributes の 2 点を含む）。
- F1、F2、F7、F8 は Lexbor の振る舞いをそのまま写したもの。F3、F4、F6 は、context を tag と名前空間だけで渡す Lexbor の API に由来する。F5 は Makiri の `arena_fragment` 層に由来する。
