# Makiri Design Document

## 1. 基本方針

### 達成すべきゴール

* HTML5 scraping で使われる範囲で Nokogiri 互換の API を持つ
* HTML5 parsing は WHATWG HTML Standard に沿ったパーサで高速に行う。
  Lexbor の tokenizer と tree builder を Rust に移植した自前のパーサと、
  添字で引く index アリーナの DOM を使い、上のコード (bridge、XPath、
  CSS の照合) は facade (`crate::html_dom`) を通して読む。vendored Lexbor
  は拡張に入らず、移植版を照合するテストの参照としてだけビルドする
  (`rake lexbor`)。移植の計画と現在地は `docs/rewrite_parser.ja.md` に置く
* XPath 1.0 には仕様に準拠した独自エンジンを作成し、libxml2 等を持ち込まない
* CSS selectors と stylesheet は、Lexbor の CSS 構文 tokenizer、構文パーサ、
  セレクタパーサを Rust に移植した自前のパーサで解析し、出荷するビルドは
  Lexbor の CSS パーサを使わない。照合は Lexbor の state machine を安全な Rust に
  移植した独自 matcher (`css_match`) で、HTML と XML の両方を同じトレイト
  (`css_match::dom`) 越しに照合し、selector のネストを native stack に
  載せない・照合前に selector 全体を検査する・確保はすべて fallible、を
  守る。意味はブラウザの `querySelectorAll` に合わせる (HTML 文書かどうかと
  要素の名前空間で規則を切り替える)。XML の CSS を XPath に落とす旧来の
  変換 (`crate::css`) は、XML の照合を確かめる参照としてだけ残す
* HTML の要素について source location (開始タグの行番号、1 始まり) を
  サポートする。行を特定できない node (parser が補った要素、fragment
  など) は推測せず nil を返す。column / byte offset は公開しない
* DOM mutation も基本的な機能をサポート
* XPath 評価は メモリ/DoS 耐性を持つ。文字列サイズ上限・再帰除去
  (反復トラバーサル)・スタック枯渇回避まで fail-closed で徹底する
* UTF-8 text-input 契約: HTML parse はブラウザ同様 lenient に
  decode (不正バイト → U+FFFD、parse は失敗しない)、XPath/CSS/DOM
  操作などプログラム API に渡す文字列は valid UTF-8 を要求し、違反は
  `Makiri::Error` で fail-closed する。NUL (U+0000) は二層契約:
  名前系 (要素/属性名・namespace・PI target/data・selector・XPath) と
  engine 入力は NUL 厳格、data 系 (text/comment 内容・属性値) は HTML も
  XML も DOM 準拠のため NUL を保持する。XML 1.0 の文字クラスにない文字を
  持つ木は XML のシリアライザが書く時点で拒む
* スレッドセーフ: parse は GVL を解放して並列スケール、XPath 評価は
  GVL 保持で共有 document/context のメモリ破壊を構造的に防ぐ
* 内部の不変条件違反で host process 全体を落とさない。Ruby に公開する
  method はすべて、その内側で起きた違反を rescue 可能な専用例外に変換
  する (入力が untrusted かどうかで選別しない)。method の外 (初期化など)
  では、後始末 (ensure / at_exit / 資源解放) を走らせてから当該スレッド
  だけを終わらせる。GC から呼ばれる処理だけは例外を投げられる場所では
  ないので中断 (abort) を選び、そのぶん処理を自明な範囲に保つ
* robustness fuzz と手で curate した差分 spec で検証する。パーサの移植は
  参照実装 (Lexbor) との差分 fuzz と、仕様の固定した版の conformance
  データ (WPT の tree construction、html5lib の tokenizer テスト)、
  ブラウザ、実際の Web ページで検証する

### 対応しないこと

* 完全な XML parsing (XML 1.0 のみで 1.1 は非対応)。DTD は内部サブセットを
  整形式として検査するだけで、宣言を適用しない。適用しないと木が変わる
  宣言 (属性の既定値、宣言された実体の展開など) を含む文書は、黙って
  無視せずエラーにする
* XSLT, DTD / Schema / RelaxNG validation, XPointer, XInclude
* Streaming parse — 全文 parse のみ
* SAX / pull-parser スタイル API
* libxml2 API との 100% 互換 (長い tail の corner case には追従しない)
* Nokogiri の全 method の drop-in 互換。

### 意図的に避けていること

* Lexbor の fork。Lexbor の不具合や欠落は移植版の側で扱い（差は
  `docs/parser_differences.ja.md` に記録する）、同時に上流に報告して直して
  もらう。移植版では、Lexbor の意味論と制御
  フローを保って書き、仕様適合のための振る舞いの変更は所有権や安全性の
  ための変更と分けて、独立した変更として入れる
  (`docs/parser_differences.ja.md` が既知の差分とその分類を持つ)
* libxml2 互換を売りにすること。libxml2 と HTML5/XPath spec
  が衝突するときは spec 準拠を優先し、差分は document 化する

## 2. Ruby API 表面

トップレベル:

```ruby
Makiri.HTML(source)   # → Makiri::HTML::Document
Makiri::HTML(source)  # alias
Makiri.parse(source)  # alias
Makiri.XML(source)    # → Makiri::XML::Document
Makiri::XML(source)   # alias
```

クラス階層:

```
Makiri::Node
├── Makiri::Document          (root)
├── Makiri::DocumentFragment
├── Makiri::Element
├── Makiri::Attr
├── Makiri::Text
├── Makiri::CDATASection
├── Makiri::Comment
├── Makiri::ProcessingInstruction
└── Makiri::DocumentType        (#public_id / #system_id / #external_id)

Makiri::NodeSet                (Enumerable)
Makiri::XPathContext
Makiri::XML::Builder           (block で XML document を組み立てる)
```

node class の正式名は WHATWG DOM の interface 名とし、Nokogiri 互換の
別名 (`CDATA` / `DTD`) は alias で与える。実際に生成されるのは表現ごとの
leaf class で、`Makiri::HTML::*` と `Makiri::XML::*` が上記の各 class を
継承する。HTML/XML で共通の method は基底 class 側に置き、表現固有の
振る舞いだけを leaf に持たせる。

例外:

```
Makiri::Error                        (基底 / StandardError)
├── Makiri::XPath::SyntaxError
│   └── Makiri::XPath::LimitExceeded (per-evaluate budget 超過)
├── Makiri::CSS::SyntaxError
├── Makiri::XML::SyntaxError
└── Makiri::XML::LimitExceeded
Makiri::InternalError                (Exception 直下)
```

入力起因のエラーはすべて `Makiri::Error` の下に置き、`rescue Makiri::Error`
で捕まえられるようにする。XPath の budget 超過を `XPath::SyntaxError` の
下に置くのは「式が原因で評価できなかった」という括りを一つにするため。
`InternalError` だけは `StandardError` の下に置かない。内部不変条件の違反は
引数エラーではないので `rescue => e` に混ぜてはならず、それでいて host が
1 リクエストを 500 に落とすために rescue はできる、という位置づけにする。

Node の method (scraping idiom の 9 割をカバー):

```
#name #node_name  #attributes #to_h
#[] #[]= #delete                   #key? #keys #values
#attr #get_attribute #set_attribute #attribute #has_attribute? #remove_attribute
#node_type #element? #elem? #text? #comment? #document? #fragment? #blank? ...
#classes #add_class #append_class #remove_class  (class 属性 helper)
#parent #children #element_children #traverse
#next #previous #next_element #previous_element
#first_element_child #last_element_child
#root #document #ancestors         #path
#xpath #at_xpath                   #css #at_css #matches?
#search #at                        (CSS/XPath 自動判別)
#text #inner_text #content #content=
#inner_html #inner_html= #outer_html #to_html #to_s  (#inner_html= は HTML のみ)
#add_child #<< #add_previous_sibling #before
#add_next_sibling #after
#remove #unlink                    #replace
#parse                             (要素 context での fragment parse, HTML のみ)
#line                              (source location, 1-based / nil, HTML のみ)
#== #eql? #hash                    (node の identity。wrapper は node ごとに 1 つ)
```

`#xpath` / `#at_xpath` は `namespace_matching:` option を取る
(`:strict` 既定 / `:lax`)。`Element#content_fragment` は
`<template>` の template contents を `DocumentFragment` で返す。

Document 追加 method (HTML/XML 共通):

```
#root
#create_element #create_text_node #create_comment #fragment
#internal_subset                   (DocumentType node, or nil)
```

HTML の Document のみ:

```
#title #title= #body #head
#encoding #meta_encoding #meta_encoding=
#quirks_mode                       (0 no-quirks / 1 quirks / 2 limited)
#errors                            (parse warnings, 当面は空配列)
```

NodeSet (Enumerable):

```
#[] #at #each #first #last #length #size #empty?
#map #select #reject ...           (Enumerable)
#text #to_html
#css #xpath #search #remove
#+ #- #& #|
```

XPathContext:

```
.new(node, namespace_matching: :strict)
#evaluate(expr, handler=nil)       (handler = 未知関数を捌く Ruby object)
#register_namespace(prefix, uri)   (#register_ns は alias)
#register_namespaces(hash) #register_variables(hash)
#register_variable(name, value)
#node=(node)                       (context node の再束縛, 同一 document)
```

式ごとにコンパイル済み AST を context にキャッシュし、同じ式の再評価では parse を省く。
`#evaluate` は GVL を保持して評価するため、同一 context への
並行 `evaluate` / `register_*` / `node=` や、評価中の同一 document の
mutation は GVL により直列化されメモリ破壊しない。

## 3. UTF-8 text-input 契約

Makiri が内部で扱うテキストは UTF-8 だけとする。入力 String の encoding は
尊重し、UTF-8 でなければ UTF-8 に変換してから境界を越えさせる (UTF-8 の
common case では変換も copy もしない)。そのうえで境界での挙動を 2 つに
分ける:

* HTML parse / fragment parse は lenient (ブラウザ互換)。
* プログラム API は strict。コード内の文字列は valid UTF-8 を要求し、
  違反は `Makiri::Error` を上げる。切り詰め・修復・解釈はしない。
* NUL (U+0000) は二層契約。名前系 (要素/属性名・qualified/local 名・
  namespace・PI target/data)・selector・XPath 式/変数名・engine 入力は
  NUL 厳格。data 系: text/comment
  内容 (`create_text_node`/`create_comment`/`content=`) と属性値
  (`[]=`/`set_attribute_ns`) — は HTML も XML も DOM 準拠のため NUL を
  保持する。XML 1.0 の文字クラスにない文字 (U+0000 を含む) を持つ木は、
  XML のシリアライザが書く時点で `Makiri::Error` で拒む。setter が flag を
  残す方式ではなく、書くときのデータから判定する。

handler 例外の message 取得も fail-closed する
(NUL を含んでも ArgumentError を leak しない)。

## 4. メモリモデル

* DOM node は document ごとの arena に確保し、arena 単位で一括解放する。
  木に入った node を個別に free することはしない。XML と HTML は自前の
  index アリーナ (node は `Vec` の添字、文字列は byte store の span) で、
  `unsafe` を含まない。
* index アリーナの node を Ruby 側へ渡す語は、document ごとの一意な
  stamp と node の添字を 1 語に詰めたもので、document をまたいでも衝突
  しない (Lexbor の node pointer が持っていた性質を保つ)。別の document
  の語を解決しようとすると、別の node を指すのではなく拒む。
* index アリーナの byte store は追記型で、`set_data` の置き換えが残した
  死んだ byte を数える。死んだ byte が生きた byte と node 数に応じた
  下限の両方を超えたら、編集の開始点 (索引を捨てた直後) で store を
  書き直す。span は添字なので、書き直しで handle は無効にならない。
* 木からの取り外しは detach であって destroy ではない。生存している
  Ruby wrapper が取り外し済みの node を指していても dangling しない。
* 解放順序は「索引・cache → document」に固定する。Lexbor の索引は node
  とその文字列を borrow するので、document より後に生き残ってはならない。
  index アリーナの索引は id と span を持ち borrow しないが、同じ hook で
  捨て、同じ順序で解放する。
* Ruby wrapper は自分が属する document を GC mark し、wrapper が
  生きている限り arena を keepalive する (HTML/XML 共通)。
* wrapper は node ごとに 1 つとし、document がその対応表を持つ。同じ
  node を 2 度辿れば同じ Ruby object を返すので、object に載せたもの
  (`equal?`、instance variable、singleton method、`freeze`) が失われない。
  document は表の wrapper を GC mark するため、document が生きている間は
  wrapper も生き続ける (上の keepalive の逆向き)。
* engine 内の確保は失敗を値として返せる API に一本化し、OOM は abort
  ではなく例外にする (§5)。一方 Ruby 側の記憶域 (wrapper、NodeSet の
  配列) は Ruby のアロケータで確保し、OOM は Ruby 自身の
  `NoMemoryError` に委ねる。ただしその raise は大域脱出するので、
  何も所有していない場所か、保護された呼び出しの内側でのみ行う。
* 索引や cache は「document に属し、mutation で捨てる」ようにする。
  破棄の hook は極力集約する。
  また索引は評価のたびに document から取り直す。
* 1 回の評価の中だけで有効な cache (string-value cache、XML の
  document-order index) は評価自身が所有する。入れ子の評価 (custom
  function handler 経由) が外側の評価を乱さないようにするため。
  HTML の document-order index は document が所有し、評価をまたいで使う。
  評価中は document を編集できないので、入れ子の評価とも共有できる。
  XPath engine は `Dom` の項目 (`KEEPS_ORDER` / `kept_position` /
  `build_kept_order`) を通して使い、ホストの種類では分岐しない。

検証は GC.stress + compaction 下で行う。

スレッド安全性: GVL を解放するのは HTML/XML の parse の2箇所だけ。
parse は入力を Ruby heap の外の buffer に copy してから GVL を解放する
(新規 document は未共有なので競合し得ないため並列にスケールする)。
一方 XPath 評価は GVL を保持する。engine と DOM は並行 mutation に対して
スレッドセーフでなく、GVL を保持することで「評価 vs 同一 document の mutation」
「同一 contextへの並行 evaluate / register」を直列化しメモリを破壊しないようにする。
CSS/serialize/読み取りアクセサも GVL を保持する。`spec/threading_spec.rb`
が GC.stress 下で共有 document/context を検証 (CI 必須)。

## 5. Build system

* ビルドの判断 (依存のビルド、link 引数、sanitizer などのモード切替) は
  拡張の configure 処理 1 箇所が所有する。
* 依存ライブラリのビルドは、選んだモードを stamp に記録し、モードが
  変わったら自動で作り直す。instrumented な依存が通常のビルドに混ざっては
  ならない。
* 拡張本体のビルドモードは configure の時点で決まり、configure をやり
  直すまで前回のモードが残る。instrumented なビルドを行う作業は clean から
  作り直して始め、通常のビルドへ戻すときも clean から作り直す。この戻しは
  自動ではない。
* Lexbor はパッチを当てたり fork したりせずそのまま使う。上流追従が
  submodule の update だけで済む状態を保つ。
* pin は release tag を原則とする。Makiri に必要な修正がまだタグに入って
  いない場合のみ tag 以外に pin し、その理由と「どのタグが出たら戻すか」を
  開発者向けの運用文書と変更履歴に記録する。条件が満たされたら戻す。
* Lexbor の構造体レイアウトと定数は上流のヘッダから自動生成する。
  手で転記しない。
* 拡張が export するのは Ruby が拡張の読み込みに必要とする 2 つの
  シンボル (初期化関数と ABI version) のみとする。Lexbor のシンボルは
  再 export しない。同じ library を使う別の gem が同一プロセスにいると、
  version の違う実装に bind して crash するため。
* export の抑制はソース側で行う。「export される名前を作らない」が
  機構であり、link 後に削る後処理は platform によっては原理的に効かない
  ので保険としてしか使わない。
* export と未定義シンボルは CI の gate で機械的に検証する。「余計な名前が
  出ていない」と「未解決の参照が残っていない」の両方を主張する。
* abort する確保 API は禁止する。engine の確保は失敗を値として返せる
  API に一本化し、OOM は abort ではなく例外にする。この規約は lint と、
  確保点を順に失敗させる injection テストで守る。
* 最適化フラグは platform ごとに「そのツールチェーンで実際に通るか」で
  決め、計測で得があるものだけ採用する。判断はビルド側で検出し、
  検出できない環境では黙って従来の経路に落ちる。
* 移植版のパーサが持つ静的な表 (タグ名、属性名、名前空間、文字参照の
  探索木、tree builder のタグの分類と調整表) は、Lexbor のヘッダとソース
  と WHATWG の `entities.json` から生成し、手で転記しない。生成物は
  Lexbor の表と要素ごとに一致することをテストで確かめる。
* crate のフィーチャは七つ。`ruby` (Ruby との境界、既定)、`html-dom`
  (index アリーナの HTML DOM とその上の CSS の照合、XPath の HTML 実装。
  `ruby` と `lexbor` が含む)、`lexbor` (vendored Lexbor を読む参照の層)、
  `alloc-inject` (OOM 注入)、`css-reference`、`lexbor-reference`、
  `xml-css-reference` (差分 fuzz の参照実装)。`lexbor` と三つの参照は
  出荷するビルドに入らない。

## 6. Testing

### 6.1 Unit / integration (rspec, `spec/`)

class ごとの振る舞い — parse の corner case, node 読み取り, navigation,
mutation, serialization, encoding, XPath, CSS, source location, custom
function handler。

### 6.2 Robustness fuzz (`spec/fuzz/`)

`rake fuzz` / `rake fuzz:sanitize` で実行する。
grammar-aware に XPath / CSS 式を生成 + mutation し、Makiri が
`Makiri::Error` 以外を絶対に上げない (foreign exception / crash /
hang なし) ことを assertする。

cargo-fuzz の harness (`ext/makiri/rust/fuzz/`) は Ruby なしで engine を
叩く。壊れないことを見るもの (`html`、`xml`、`xpath`、`xml_xpath`、
`html_xpath`、`css`、`html_css`) に加えて、参照実装との一致を見る差分
harness を持つ: `html_diff` (移植版の tree builder と Lexbor の木、その
直列化、深さの上限の判定)、`selector_diff` (移植版と Lexbor のセレクタ解析)、`html_css_diff` (自前の
matcher と Lexbor の matcher)、`css` (XML の matcher と旧来の XPath への
変換)。参照実装は `lexbor-reference` / `css-reference` /
`xml-css-reference` フィーチャでだけ link する。

> 当初構想にあった「Nokogiri を oracle にした diff モードを fuzz に持たせる」
> 案は採っていない。Lexbor と libxml2 は HTML5 semantics が異なり、ランダム
> 入力の構造 diff は偽陽性が多い。差分 fuzz の相手は、同じ意味論を持つはずの
> 参照実装 (移植元の Lexbor) に限り、食い違いは 0 件を期待値とする。
> 許す差は安全性のための修正だけで、差分表に列挙したものに限る。
> Nokogiri との突き合わせは、fuzz とは別の conformance 検査として持つ (§6.4)。

### 6.3 Property / 差分 (`spec/invariants/`, `spec/differential/`)

入力を振って不変条件 (namespace、木の形、索引の staleness、serialization、
text-input 契約) を検査する。あわせて、過去の実装が返していた答えを
baseline として固定し、現在のビルドと突き合わせる。baseline は再生成せず、
食い違いは調査対象として扱う。意図した変更なら振る舞いの変更と同じ commit
で baseline を書き換え、差分として review できるようにする。

### 6.4 Conformance と参照実装との差分

仕様と参照実装に対する検査は `rake conformance:*` にまとめる。

* HTML parse: WPT の tree-construction データ (html5lib のテストの現在の
  置き場) を Ruby の runner (`conformance:html5`) と crate の runner
  (`conformance:parser`) で通す。両者は同じ既知差分の一覧
  (`spec/conformance/parser_known_divergences.txt`) を読み、一覧にある
  テストが通ってしまえばそれも失敗とする。tokenizer は html5lib の
  tokenizer テストを通す。
* パーサの移植版は Lexbor を参照実装として、WPT の全入力、無作為な文書と
  fragment、実際の Web ページ (web100k、`conformance:web100k`、CI では
  回さない) で木が一致することを見る (`lexbor::tree_diff`)。移行期に
  index アリーナを Lexbor アダプタと読み取り・編集・XPath で照合していた
  検査（`lexbor::arena_tests`、`html_xpath_diff`）は、arena が唯一の実装に
  なったので退役した。
  セレクタパーサの移植版は Lexbor の解析結果と一致することを見る
  (`lexbor::selector_diff`)。
* ブラウザ (Chromium、Firefox、WebKit) は証拠として使い、合格基準には
  しない (`conformance:browsers`)。オラクル同士の食い違いは差分表に記録する。
* Nokogiri との突き合わせ: XPath / CSS / XML / Builder のそれぞれを
  Nokogiri と比較し、XML は W3C の XML Test Suite と照合する。食い違いは
  0 件を期待値とし、意図した差分は方針として明示的に列挙したものだけを
  許し、手で curate した spec と差分文書 (§7、`docs/parser_differences.ja.md`、
  `NOKOGIRI_DIFFERENCES.md`) に固定する。
* 仕様、WPT、html5lib-tests、`entities.json` の版は固定し、
  `docs/parser_differences.ja.md` に記録する。

### 6.5 メモリ安全性と資源 (sanitizer / leak / OOM)

* address sanitizer 下で spec と fuzz を通す。自前の arena は、大きな
  領域を切り出して渡す bump 方式ではなく、node と文字列を添字で引く構造に
  する。arena 内の範囲外アクセスは境界検査で必ず捕まるので、sanitizer の
  poisoning には頼らない (bump 方式では、切り出した領域どうしの間の
  overflow を sanitizer に検出させるために poisoning が要る。その必要
  自体をなくす)。
* vendored 依存の bump arena も、必要なら依存自身を instrumented で
  ビルドして検査する。ここにもパッチは当てない (ビルド flag で行う)。
* leak 検査は sanitizer と別立てで持つ。例外で脱出した経路も含めて
  漏れがないことを見る。
* OOM は injection で全確保点を順に失敗させ、どの点で失敗しても
  fail-closed (きれいに raise するか、成功時と同じ答え) になることを見る。
* Ruby に依存しない中核 (確保、buffer、UTF-8 の検証と decode) は
  形式検証を併用する。

### 6.6 Performance (`bench/`)

`rake bench` で Makiri vs Nokogiri を比較する。最適化は計測ゲート:
bench で実測の改善があり spec/fuzz/sanitizer が green なときだけ採用、
そうでなければその変更を revert する。移植版のパーサは、既定の切り替えの
前に bench の各行で種類としての退行 (Nokogiri と同等以下に落ちる行) が
ないことを確かめた (`docs/rewrite_parser.ja.md` の進捗)。

### 6.7 CI

GitHub Actions で Ruby の各バージョンと OS とで実行する。
sanitizer は Linux のみで実行する。Lexbor と比べる job (`rake rust:test`、
差分 fuzz、`conformance:parser`) だけが `rake lexbor` で参照の Lexbor を
ビルドする。

## 7. 互換性に関する考慮

Lexbor の HTML5 parsing semantics は libxml2 のそれより spec
(WHATWG) に近い。そのため Nokogiri と結果が違うケースがある。
Makiri では spec 準拠を取り、差分を curated spec で固定する:

| 項目 | Makiri | Nokogiri / browser |
| --- | --- | --- |
| 暗黙の `<head>` 挿入 | 常にあり (HTML5) | HTML4 (libxml2) は head に入る要素があるときだけ作る。HTML5 は常にあり |
| name test の namespace 解決 | strict 既定 / `:lax` opt-in | HTML5 は strict。HTML4 は namespace を持たず `:lax` 相当 |
| XPath namespace axis | 未実装 (明示エラー) | libxml2 は実装 |
| 不正バイト入力の parse | U+FFFD 置換 (ブラウザ同様、失敗しない) | HTML4 は binary 入力の encoding を推定し、UTF-8 と明示された入力の不正バイトはそのまま保持。HTML5 は U+FFFD 置換 |
| 名前テストの ASCII case 畳み込み | HTML の要素・属性は畳み込む (SVG/MathML は保持) | Nokogiri は畳み込まない (HTML4 / HTML5 とも)。browser は畳み込む |
| HTML 要素の namespace | XHTML namespace (属性は namespace なし) | HTML5 は namespace なし (`namespace-uri()` は空文字列) |
| HTML 中の `<?target data?>` | processing instruction node (HTML Standard の現行の定義) | HTML5 は comment にする。HTML4 は processing instruction |
| node からの CSS 検索の範囲 | 文書全体で照合し、結果を子孫に絞る (`querySelectorAll` と同じ、HTML と XML) | selector を node 起点に限定して照合する |
| XML の CSS の接頭辞のない型セレクタ | どの名前空間の要素にも当たる (ブラウザと同じ)。既定の名前空間は ns Hash の `"xmlns"` で与える | ルート要素の既定の名前空間 (ns Hash を渡すと名前空間なし) |
| 要素名の変更 (`name=`) | できない (DOM に rename はない) | できる |

Nokogiri 列の HTML4 / HTML5 は `Nokogiri::HTML4` / `Nokogiri::HTML5` を指す
(Nokogiri 1.19 / libxml2 2.13 で確認)。

HTML パーサが仕様の固定した版から逸脱している点 (既定の Lexbor と移植版の
両方)、移植版が安全性のために Lexbor と違えた点、オラクル同士の食い違いは
`docs/parser_differences.ja.md` に番号を付けて置く。

HTML名前テストとHTML属性ではASCII caseを畳み込み、SVG/MathMLの名称は
保持する。HTML の XPath 属性軸からは、foreign 要素 (SVG/MathML) の
`xmlns` / `xmlns:*` 属性 (XMLNS namespace の属性) を除外する。HTML 要素に
書かれた `xmlns` は普通の属性として見える。Nokogiri は foreign 要素の
`xmlns` も属性として返す。


## 8. 設計上の決定メモ

状況が変われば変更することもある。

* `HTML()` が定義の実体、`parse()` は alias。Nokogiri に揃える。
* HTML パーサは Lexbor を Rust に移植する。html5ever は採らない: 確保が
  失敗を返せず (fail-closed の規約と合わない)、atom の表がプロセス全体で
  共有され、`unsafe` を含む tendril が `falloc` と噛み合わないため。移植は
  Lexbor の状態関数と insertion mode を一対一で写し、Lexbor との一致を
  差分 fuzz で見られる形にする。仕様への適合は一致を確かめた後に、独立
  した変更として入れる。
* 移植版の DOM は最初から添字の index アリーナにし、ポインタを保つ DOM を
  経由しない。node あたりの byte は Lexbor の DOM の半分ほどで、範囲外
  アクセスは境界検査で捕まる。bridge は node を「document への参照」
  (`RawNode`) として扱い、document を `&mut` で借りた閉包の中でだけ編集
  する形に先に直し、二つの backend を一つの facade で差し替えられるように
  した。bridge が両者で違う箇所は `bridge::dom` の一つのモジュールに置く。
* セレクタの構文解析も Lexbor のパーサを移植して自前にした。Lexbor の
  共有アリーナの寿命に縛られずに解析結果をキャッシュで所有できる。
  stylesheet の読み取り (`parse_stylesheet`) も移植版で解析する。規則の
  構造と誤りからの回復は Lexbor のままで、宣言の名前と値はプロパティごと
  の正規化をやめ、どのプロパティにも同じ規則 (名前は小文字、値は空白の
  連続を詰めた原文) で返す (`docs/rewrite_parser.ja.md` の中間案)。
* Node の wrapper は node ごとに 1 つとし、document が対応表で cache する
  (§4)。当初は wrapper を cache せず node の identity で比較していたが、
  同じ node を辿るたびに別の object が返ると、`equal?`・instance variable・
  singleton method・`freeze` といった object 側の状態が黙って失われる。
  NodeSet / Node の比較は引き続き node の identity で行う。
* XPath 評価は GVL を解放しない。当初は handler なし評価で GVL を
  解放しスレッド間で XPath をスケール (~3.3×) させていたが、GVL 解放中の
  walk は「共有 document の mutation」「同一 context への並行 evaluate /
  register」と競合し use-after-move / メモリ破壊を起こす。これを lock で
  守る案 (per-document/per-context mutex) は検討したが、恒久的な不変条件
  維持コスト (新しい engine 変更ごとに "GVL 解放区間は Ruby-free かつ
  施錠済み state だけ触る" を守る) が高い。代わりに GVL を保持して構造的
  に安全にした: GVL が全 Ruby スレッドの native code を直列化するので競合
  自体が起きず lock 不要。単一スレッド性能は不変、parse は依然 GVL 解放で
  並列。失うのは「単一の共有 document への XPath の並列実行」だけ。
  GVL 解放する XPath 経路を再導入しない (document/context lock の整備が
  済むまで)。
* 入力は lenient parse / strict API。HTML parse はブラウザ同様
  decode して常に成功 (不正→U+FFFD、HTML5 適合・スクレイピング堅牢・
  U+FFFD 置換は smuggling 防御)。一方プログラム API はバイト列ではなく
  制御文字列なので valid UTF-8 / NUL なしを要求し fail-closed。

## 9. Nokogiri からの移行 (非公式)

以下のような既存の Nokogiri のコードがある場合:

```ruby
require 'nokogiri'
doc = Nokogiri::HTML(source)
doc.css('.foo').map(&:text)
```

以下のように書き換えれば実行できる。

```ruby
require 'makiri'
doc = Makiri::HTML(source)
doc.css('.foo').map(&:text)
```

HTML scraping idiom の ~90% は `Nokogiri` → `Makiri` の sed
replace で動くようにする。残り ~10% は divergence
(主に HTML5 conformance、Makiri の方が strict)。
