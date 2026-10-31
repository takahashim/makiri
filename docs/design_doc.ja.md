# Makiri Design Document

## 1. 基本方針

### 目標

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
  Lexbor の CSS パーサを使わない。照合は、Lexbor の state machine を安全な
  Rust に移植した独自 matcher (`css_match`) が行い、HTML と XML の両方を
  同じトレイト (`css_match::dom`) 越しに扱う。matcher は selector のネストを
  native stack に載せず、照合の前に selector 全体を検査し、確保をすべて
  fallible にする。意味はブラウザの `querySelectorAll` に合わせ、HTML 文書
  かどうかと要素の名前空間で規則を切り替える。XML の CSS を XPath に変換
  する旧来の方式は、XML の照合をブラウザとの比較で確かめるようになって削除した
* HTML の要素について source location (開始タグの行番号、1 始まり) を
  サポートする。行を特定できない node (parser が補った要素、fragment
  など) は推測せず nil を返す。column / byte offset は公開しない
* DOM の編集 (node の追加・取り外し・置換、属性と内容の書き換え、
  `inner_html=`) をサポートする
* XPath 評価はメモリ消費と DoS に耐える。文字列の大きさに上限を置き、
  木は再帰ではなく反復で辿り、スタックの残量を検査する。上限に達したら
  結果を切り詰めずに raise する (fail-closed)
* parse も上限を持つ。HTML parse は木の深さ (`max_tree_depth:`、既定
  400) と、入力に対して作る文書の大きさ (`max_amplification:`、既定は
  入力の 256 倍で 64 MiB を下回らない) を制限し、超えたら `Makiri::Error`
  で拒む。XML の文書は arena の byte 予算 (`max_bytes:`、既定 256 MiB)
  を持ち、parse だけでなく編集も含めた文書の生涯にわたって数える
* 入力の文字列は UTF-8 として扱う (§3)。HTML parse はブラウザと同じく
  不正なバイトを U+FFFD に置き換えて必ず成功させる。XPath、CSS、DOM 操作
  などのプログラム API に渡す文字列は valid UTF-8 でなければ
  `Makiri::Error` にする。NUL (U+0000) は、名前と式では拒み、text、
  comment、属性値では DOM に合わせて保持する
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

* Lexbor の fork。Lexbor の不具合や欠落は移植版の側で扱い、同時に上流に
  報告して直してもらう。移植版は Lexbor の意味論と制御フローを保って書く。
  仕様に合わせるための振る舞いの変更は、所有権や安全性のための変更と分けて、
  独立した変更として入れる。既知の差分とその分類は
  `docs/parser_differences.ja.md` に記録する
* libxml2 互換を売りにすること。libxml2 と HTML5/XPath spec
  が衝突するときは spec 準拠を優先し、差分は document 化する。例外は
  XPath の数値の文字列化 (`string()`) で、XPath 1.0 §4.2 ではなく
  libxml2 の書き方 (指数表記、有効数字 15 桁) に合わせる。Nokogiri から
  移る呼び出し側が文字列で比べるため

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
下に置くのは、「式が原因で評価できなかった」エラーを一つの class で
捕まえられるようにするためである。`InternalError` だけは `StandardError`
の下に置かない。内部不変条件の違反は引数の誤りではないので、`rescue => e`
で入力起因のエラーと一緒に捕まえさせない。それでも host が名指しで rescue
すれば、そのリクエストだけを 500 にできる。

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
#inner_html #inner_html= #outer_html #outer_html= #to_html  (HTML のみ)
#to_s                              (XML は #to_xml / #canonicalize も)
#add_child #<< #add_previous_sibling #before
#add_next_sibling #after
#remove #unlink                    #replace
#parse                             (要素 context での fragment parse, HTML のみ)
#line                              (source location, 1-based / nil, HTML のみ)
#== #eql? #hash                    (node の identity。wrapper は node ごとに 1 つ)
```

`#xpath` / `#at_xpath` は `namespace_matching:` option を取る
(`:strict` 既定 / `:lax`)。ほかの keyword は namespace の接頭辞の束縛
(Nokogiri の `xpath("//s:p", s: uri)` と同じ) で、`XPathContext.new` も
同じ keyword を登録する。`Element#content_fragment` は
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
#evaluate(expr)                    (未知の関数は XPath::SyntaxError)
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
尊重し、UTF-8 でなければ UTF-8 に変換してから境界を越えさせる
(UTF-8 / US-ASCII / ASCII-8BIT は変換しない)。String のバイト列は、
読んでから copy し終えるまで Ruby のコードを挟まずに一度だけ copy する。
copy の後で Ruby 側が String を変えても、copy には影響しない。境界での
扱いは次のとおり:

* HTML parse / fragment parse は lenient (ブラウザ互換)。
* プログラム API は strict。コード内の文字列は valid UTF-8 を要求し、
  違反は `Makiri::Error` を上げる。切り詰め・修復・解釈はしない。
* NUL (U+0000) は入力の種類で扱いを分ける。名前 (要素名、属性名、
  qualified/local 名、namespace、PI の target と data)、selector、XPath の
  式と変数名、engine への入力は、NUL を含めば拒む。data (text と comment の
  内容 `create_text_node` / `create_comment` / `content=`、属性値 `[]=` /
  `set_attribute_ns`) は、HTML でも XML でも DOM に合わせて NUL を保持する。
  XML 1.0 の文字クラスにない文字 (U+0000 を含む) を持つ木は、XML の
  シリアライザが書く時点で `Makiri::Error` で拒む。setter が flag を残す
  のではなく、書くときのデータから判定する。

XPath の独自関数（Nokogiri の handler）は受け付けない。
評価中に Ruby のコードは走らず、XPath 1.0 にない関数の呼び出しは
`XPath::SyntaxError` になる。例外は Nokogiri の組み込みの `css-class` と
`local-name-is` で、Nokogiri の builtin namespace に束縛した接頭辞の下で
Nokogiri と同じに答える。

引数は変換しない。
文字列の引数は `String`、名前の引数（属性名、要素名、接頭辞、セレクタ、式）は `String` か `Symbol` に限り、それ以外は `TypeError` にする。
`to_s` / `to_str` は呼び出し側の Ruby のコードであり、拡張の中でそれを走らせないためである。
`to_s` で変換するのは lib の側（parse の入口の `String()`、`XML::Builder`）だけで、拡張に入る前に済む。

## 4. メモリモデル

* DOM node は document ごとの arena に確保し、arena 単位で一括解放する。
  木に入った node を個別に free することはしない。XML と HTML は自前の
  index アリーナ (node は `Vec` の添字、文字列は byte store の span) で、
  `unsafe` を含まない。
* index アリーナの node を Ruby 側へ渡す語は、document ごとの一意な
  stamp と node の添字を 1 語に詰めたもので、document をまたいでも衝突
  しない (Lexbor の node pointer が持っていた性質を保つ)。別の document
  の語を解決しようとすると、別の node を指すのではなく拒む。
* HTML の index アリーナの byte store は追記型で、`set_data` の置き換え
  が残した死んだ byte を数える。死んだ byte が生きた byte と node 数に応じた
  下限の両方を超えたら、編集の開始点 (索引を捨てた直後) で store を
  書き直す。span は添字なので、書き直しで handle は無効にならない。
  XML のアリーナは書き直さない。置き換えた値や取り外した node の byte は
  文書が消えるまで残り、`max_bytes:` に数え続ける。詰めるのは `dup` で、
  今ある中身だけを同じ予算の新しい文書に写す。
* 木からの取り外しは detach であって destroy ではない。生存している
  Ruby wrapper が取り外し済みの node を指していても dangling しない。
* index アリーナの索引は id と span を持ち、node も文字列も borrow しない
  ので、document との解放順序は問わない。守るのは、どの編集も何かを
  変える前に索引を捨てること (`HtmlParsed::edit` → `invalidate_indexes`)
  である。
* Ruby wrapper は自分が属する document を GC mark し、wrapper が
  生きている限り arena を keepalive する (HTML/XML 共通)。
* ラップしたオブジェクトが生かしておく Ruby の値は、Ruby との境界の層が
  持つスロットにだけ置く。payload (Rust の構造体) は Ruby の値を持たない。
  スロットはオブジェクトの確保の直後に payload と一緒に一度だけ書き、
  mark はスロットを全部 pinned で mark する。型ごとの trace を書かないので、
  mark の書き忘れも、オブジェクトになる前に値を入れる誤りも起きない。
  中身が揃う前のオブジェクトは、class を持つうちに hidden ivar を書いて
  から隠し、中身を埋めて揃ってから見せる。
* wrapper は node ごとに 1 つとし、document がその対応表を持つ。同じ
  node を 2 度辿れば同じ Ruby object を返すので、object に載せたもの
  (`equal?`、instance variable、singleton method、`freeze`) が失われない。
  document は表の wrapper を GC mark するため、document が生きている間は
  wrapper も生き続ける (上の keepalive の逆向き)。
* engine 内の確保は失敗を値として返せる API に一本化し、OOM は abort
  ではなく例外にする (§5)。一方 Ruby 側の記憶域 (wrapper) は Ruby の
  アロケータで確保し、OOM は Ruby 自身の
  `NoMemoryError` に委ねる。その raise は大域脱出なので、raise しうる C の
  呼び出しはすべて Ruby との境界の層 (`nupur` の `raw`) の中で
  `rb_protect` の下に置き、`Err` として戻す（Rust のフレームを飛び越え
  させない）。
* Ruby の外に持つメモリ (arena、NodeSet の配列、XPathContext の AST の
  cache) は GC に報告する。報告は GVL を持つ場所で行い、返却は境界の層の
  free の callback が、payload の申告した量をもとに行う。Makiri の `Drop`
  は Ruby の関数を呼ばない。
* 索引や cache は「document に属し、mutation で捨てる」ようにする。
  破棄の hook は極力集約する。
  また索引は評価のたびに document から取り直す。
* 1 回の評価の中だけで有効な cache (string-value cache、XML の
  document-order index) は評価自身が所有する。
  HTML の document-order index は document が所有し、評価をまたいで使う。
  評価中は Ruby が走らず、document は編集されない。
  XPath engine は `Dom` の項目 (`KEEPS_ORDER` / `kept_position` /
  `build_kept_order`) を通して使い、ホストの種類では分岐しない。

検証は GC.stress + compaction 下で行う。

スレッド安全性: GVL を解放するのは HTML/XML の parse の2箇所だけ。
parse は入力を Ruby heap の外の buffer に copy してから GVL を解放する。
作っている document はまだ誰とも共有されていないので競合せず、parse は
並列にスケールする。一方 XPath 評価は GVL を保持する。engine と DOM は
並行 mutation に対してスレッドセーフではない。GVL を保持することで、
評価と同じ document の mutation、同じ context への並行 evaluate / register
を直列化し、メモリを破壊しないようにする。
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
* export は CI の gate (`rake symbols`) で機械的に検証する。export された
  名前の全体を見て、初期化関数と ABI version のほかに名前が出ていない
  ことを主張する。
* abort する確保 API は禁止する。engine の確保は失敗を値として返せる
  API に一本化し、OOM は abort ではなく例外にする。この規約は lint と、
  確保点を順に失敗させる injection テストで守る。
* 最適化は platform によらない 1 つの release profile に固定する
  (`opt-level = 3`、`lto = true`、`codegen-units = 1`)。拡張は cdylib
  だけをビルドし、LTO が crate をまたいで効くようにする。変えるのは
  bench で計測した得があるときだけとする。
* 移植版のパーサが持つ静的な表 (タグ名、属性名、名前空間、文字参照の
  探索木、tree builder のタグの分類と調整表) は、Lexbor のヘッダとソース
  と WHATWG の `entities.json` から生成し、手で転記しない。生成物は
  Lexbor の表と要素ごとに一致することをテストで確かめる。
* Rust のクレートは五つある。参照実装を除く四つが拡張の workspace をなし、
  依存は一方向で、下のクレートは上のクレートを知らない。
  - 確保の基盤 (`falloc`): 失敗を値として返す確保と、その注入の hook。
    どのクレートにも依存しない。
  - エンジン (`makiri_engine`): HTML の tokenizer と tree builder、
    index アリーナの DOM、XML の reader、XPath、セレクタの構文解析と CSS の
    照合。依存するのは `falloc` と Rust のライブラリ (`encoding_rs`、
    `chardetng`) だけで、Ruby も Lexbor も知らない。
  - Ruby との境界の層 (`nupur`、rb-sys の上): Ruby と話すための `unsafe` を
    すべて持ち、safe な型だけを外に出す。拡張にもエンジンにも依存しない
    ので、層が拡張の都合を知ることはない。拡張が層に渡す必要のあるもの
    (内部エラーの例外クラス、raise の注入の hook) は、初期化のときに
    登録する。
  - 拡張 (`makiri_rs`): エンジンと境界の層に依存し、Ruby の表面 (`bridge`、
    `glue`、初期化) だけを持つ。`bridge` と `glue` は `unsafe` を書かない。
  - 参照実装 (`makiri_lexbor`): vendored Lexbor と、そのレイアウトの
    生成 (bindgen)、Lexbor の DOM を読む adapter、移植版を Lexbor に照らす
    差分テスト。エンジンに依存し、出荷する拡張はこれに依存しない。fuzz の
    クレートと同じく独立した workspace とし、拡張の workspace の
    Cargo.lock (source install が使うもの) には Lexbor のためのものが何も
    現れない。
  拡張と参照実装は、エンジンのモジュールをクレートの根で re-export する。
  `crate::xml` は拡張の中でも参照実装の中でもエンジンのものを指す。
* エンジンが Ruby-free であることは、規約や lint ではなく依存関係で
  成り立たせる。エンジンは rb-sys にも `nupur` にも依存しないので、Ruby の
  値や境界の層の名前を書けばコンパイルが通らない。GVL を外して動く parse の
  本体と、文書を `&mut` で借りた編集の閉包が Ruby を呼ばないことは、
  それらがエンジンの関数だけを呼ぶことに帰着する。
* エンジンは `unsafe` を書かない。クレートの根で `forbid(unsafe_code)` と
  する。安定版の Rust には、失敗を返す `Box<T>` の確保を safe に書く方法が
  なく、そこに一か所だけ `unsafe` が残る。それを持つ確保の基盤を別の
  クレート (`falloc`) にするのはこのためである。Miri が Ruby なしで検査する
  `nupur` の `utf8_copy` も、`unsafe` を許すこのクレートのテストビルドに
  含める。unwind しないことの証明に使う未定義シンボルの呼び出しは、証明の
  ためのビルドにだけ現れるので、そのビルドに限って許す。
* エンジンと拡張の境目にあるモジュールは、次のように置く。
  - コンパイル済みセレクタの cache (`css_cache`) はエンジンに置く。二つの
    DOM を知り、Ruby を知らない。スレッドごとに持つのは、クエリが GVL を
    外さないことから来る性質であり、照合の入口の約束として述べる。
  - スタック残量の検査はエンジンが問い合わせ、検査の関数は拡張が初期化の
    ときに渡す (`stack::install`)。OOM 注入の hook も同じ形で、エンジンと
    `falloc` は hook を受け取る口だけを持つ。
  - Lexbor から離れた箇所の分岐 (`compat`) はエンジンに置き、Lexbor の
    とおりに動かす切り替え (`as_lexbor`) はエンジンのフィーチャにする。
    どこで Lexbor から離れたかはエンジン自身の知識であり、切り替えを使う
    のが参照実装だけである。出荷するビルドでは分岐は定数になる。
  - 文書の本体の借用ガード (`doc_cell`) はエンジンに置く。同じ文書を
    Ruby 側の複数の参照が共有することへの備えで、使うのは拡張だけだが、
    Ruby-free であり、その約束 (借用がガードより長く生きない、ガードが
    スレッドを離れない) を `compile_fail` の doctest で Ruby なしに確かめる
    には、Ruby に依存しないクレートにある必要がある。
  - 子の位置の memo (`child_index`) はエンジンに置き、memo を無効にする
    版数 (`tree_version`) は拡張が文書ごとに持って渡す。
  - GVL を鍵にした cell (`gvl` の `GvlCell`) は参照実装に置き、出荷する
    ビルドには入らない。C の callback の中で panic を捕まえる latch
    (`caught::PanicLatch`) は、参照実装が Lexbor の callback のために自分の
    ものを持ち、出荷するビルドでは nupur のもの (`nupur::caught`) を GVL を
    外した処理と `rb_hash_foreach` の callback が使う。
  - 参照実装のテストが使うエンジンの内部 (Lexbor のとおりに動かす切り替え、
    適合テストの harness、tokenizer の追跡付きの実行など) は、エンジンの
    `lexbor-compat` フィーチャで公開する。出荷するビルドには入らない。
* エンジンの公開 API は、拡張と参照実装が呼ぶものに限る。エンジンの中の不変条件
  (編集は文書を貸す閉包の中でだけ行う、作りかけの node は木に入らない、
  など) はこれまでどおり型で守り、クレートの可視性には頼らない。依存する
  クレート (拡張、参照実装、fuzz) はどれも同じ型の規則に従う。
* GVL を持っている証拠のトークンは、Ruby が呼ぶ入口 (メソッドの trampoline
  と初期化関数) だけが作る。その補助関数は `unsafe` で、`unsafe` を書けない
  拡張のコードからは、マクロの展開を通してしか呼べない。
* エンジンと境界の層への呼び出しはクレートをまたぐが、`cdylib` の LTO の
  下でインライン化される。拡張を `rlib` として同時に作ると rustc は LTO を
  行わないので、fuzz の
  クレートは拡張ではなく、参照実装 (エンジンを re-export する) に
  依存する。
* フィーチャはクレートごとに持つ。エンジンは `html-dom` (既定。index
  アリーナの HTML DOM と、その上の CSS の照合、XPath の HTML 実装。外すと
  素のエンジンになる) と `lexbor-compat` (Lexbor のとおりに
  動かす切り替え。参照実装だけが使う) と `alloc-inject` (OOM 注入)
  を持つ。`falloc` と `nupur` も `alloc-inject` を持ち、拡張の
  `alloc-inject` はこの 3 つを有効にする。参照実装は `css-reference` と
  `lexbor-reference` (差分 fuzz の参照) を持つ。Ruby との境界を外す
  フィーチャと Lexbor を読むフィーチャはなくなり、それぞれ拡張と
  参照実装のクレートそのものになる。出荷する
  拡張のビルドは Lexbor を link せず、そのヘッダも読まない (bindgen を使う
  のは Ruby のヘッダを読む rb-sys だけになる)。

## 6. Testing

### 6.1 Unit / integration (rspec, `spec/`)

class ごとの振る舞い — parse の corner case, node 読み取り, navigation,
mutation, serialization, encoding, XPath, CSS, source location。

### 6.2 Robustness fuzz (`spec/fuzz/`)

`rake fuzz` / `rake fuzz:sanitize` で実行する。
XPath / CSS の式を文法に沿って生成・変異させ、Makiri が `Makiri::Error`
以外を上げない (foreign exception、crash、hang がない) ことを確かめる。

cargo-fuzz の harness (`ext/makiri/rust/fuzz/`) は Ruby なしで engine を
叩く。壊れないことを見るもの (`html`、`xml`、`xpath`、`xml_xpath`、
`html_xpath`、`css`、`html_css`) に加えて、参照実装との一致を見る差分
harness を持つ: `html_diff` (移植版の tree builder と Lexbor の木、その
直列化、深さの上限の判定)、`selector_diff` (移植版と Lexbor のセレクタ
解析)、`stylesheet_diff` (移植版と Lexbor の stylesheet 解析)、
`html_css_diff` (自前の matcher と Lexbor の matcher)。`css` は XML の
matcher の入口どうしの一致も見る。fuzz のクレートは拡張ではなく参照実装 (`makiri_lexbor`) に
依存し、比べる相手は参照実装の `lexbor-reference` / `css-reference`
フィーチャでだけ link する。

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
  (`lexbor::selector_diff`)。stylesheet パーサも同様に照合する
  (`lexbor::stylesheet_diff`)。
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
  する。bump 方式では、切り出した領域どうしの間の overflow を sanitizer に
  検出させるために poisoning が要る。添字で引く構造なら arena 内の範囲外
  アクセスは境界検査で捕まるので、poisoning に頼る必要がない。
* vendored 依存の bump arena も、必要なら依存自身を instrumented で
  ビルドして検査する。ここにもパッチは当てない (ビルド flag で行う)。
* leak 検査は sanitizer と別立てで持つ。例外で脱出した経路も含めて
  漏れがないことを見る。
* OOM は injection で全確保点を順に失敗させ、どの点で失敗しても
  fail-closed (きれいに raise するか、成功時と同じ答え) になることを見る。
* Ruby に依存しない中核 (確保、buffer、UTF-8 の検証と decode) は
  形式検証を併用する。

### 6.6 Performance (`bench/`)

`rake bench` で Makiri と Nokogiri を比較する。最適化は、bench で改善を
実測でき、spec、fuzz、sanitizer が green のときだけ採用し、そうでなければ
revert する。移植版のパーサは、既定の切り替えの
前に bench の各行で種類としての退行 (Nokogiri と同等以下に落ちる行) が
ないことを確かめた (`docs/rewrite_parser.ja.md` の進捗)。

### 6.7 CI

GitHub Actions で Ruby の各バージョンと OS とで実行する。
sanitizer は Linux のみで実行する。`rake lexbor` で参照の Lexbor を
ビルドするのは、参照実装を build する job だけである: 参照実装も含めた
clippy (`rake lint`)、`rake rust:test`、ASan 下の Rust のテスト (sanitize
job)、差分 fuzz、`conformance:parser`。

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

HTML の名前テストと HTML の属性では ASCII case を畳み込み、SVG/MathML の
名称は保持する。HTML の XPath 属性軸からは、foreign 要素 (SVG/MathML) の
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
  した。`bridge::dom` は、Document が `RefCell` で所有する本体と、その読み取りと
  編集のガードを置く。本体へ届く道はガードだけで、編集は読み取りが残っている間は
  実行時に拒否される。
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
* XPath 評価は GVL を解放しない。当初は GVL を解放し、XPath をスレッド間で
  並列に走らせていた (約 3.3 倍にスケールした)。しかし GVL を外した間の
  走査は、共有 document の mutation や、同じ context への並行 evaluate /
  register と競合し、use-after-move やメモリ破壊を起こす。document ごと・
  context ごとの mutex で守る案も検討したが、engine を変えるたびに「GVL を
  外した区間は Ruby に触れず、施錠した state だけを読む」ことを守り続ける
  必要があり、その保守の負担が大きい。GVL を保持すれば、GVL が全 Ruby
  スレッドの native code を直列化するので、lock なしで競合そのものが
  起きない。単一スレッドの性能は変わらず、parse は引き続き GVL を外して
  並列に走る。失うのは、一つの共有 document に対する XPath の並列実行だけ
  である。document と context の lock を整備するまでは、GVL を外す XPath
  の経路を再導入しない。
* エンジンをクレートとして分ける (§5)。分ける前からエンジンは Ruby も
  Lexbor も知らずに書かれ、Ruby なしで動いていた (`ruby` フィーチャを外した
  ビルド、Kani、Miri、Verus、cargo-fuzz、Ruby なしの計測)。ただしその境界は
  手で保守するモジュールの一覧 (lint の `ENGINE`、unsafe の検査の
  `ENGINE_DIRS`、`forbid` の根) で定義されていて、一覧どうしが食い違って
  いた (消したモジュールが残り、新しいモジュールが入っていない)。クレートに
  すると境界は依存関係になり、一覧も、それを守る lint
  (`ruby_layer_in_engine`) がエンジンに対して見るものもなくなる (lint は
  `nupur` の中の Ruby-free な一モジュール、`utf8_copy` のためだけに残る)。
  境界の層 (`nupur`) を分けたのと同じ考えであり、性能は `cdylib` 単体の
  ビルドの LTO で保つ。
* parse は lenient、API は strict にする。HTML parse はブラウザと同じく
  decode し、不正なバイトを U+FFFD に置き換えて常に成功する。HTML5 に
  適合し、壊れたページもスクレイピングでき、U+FFFD への置き換えは
  smuggling も防ぐ。一方、プログラム API に渡す文字列はデータのバイト列
  ではなく処理を指示する文字列なので、valid UTF-8 で NUL を含まないことを
  要求し、違反は raise する。

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

HTML scraping でよく使う書き方の約 9 割が、`Nokogiri` を `Makiri` に
置換するだけで動くことを目標にする。残りの約 1 割は挙動が違い、その多くは
HTML5 への適合で Makiri の方が厳格なことによる。
