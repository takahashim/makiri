# HTML パーサの Rust 移植計画

本書は、vendored Lexbor を Rust の自前実装に置き換える計画を述べる。
2026-10-18 時点の調査と、その計画への査読（同日）を反映している。
設計の前提は `docs/design_doc.ja.md` に従い、運用上の制約は `CLAUDE.md` に従う。

## 目的と制約

目的は、Makiri から C のコードをなくすことである。
現在 C で残っているのは vendored Lexbor だけで、XPath エンジン、XML リーダ、CSS マッチャ、各種 index、挿入規則、文字列境界、Ruby glue はすでに Rust である。

方針として、html5ever は採用せず、Lexbor を参考に自前で書く。
html5ever を採らない理由は三つある。
第一に、html5ever と依存クレート（tendril、string_cache）の確保は失敗時に abort し、`falloc` と `rake oom` が守っている「OOM は raise で返し、ホストを殺さない」方針をパース経路で守れない。
第二に、プロセス全体で共有する atom テーブルと tendril の unsafe が入り、`rake unsafe:boundaries` の勘定と再入しない設計に合わない。
第三に、木の深さガードや、現在の Lexbor pin が実装している processing-instruction token など、Makiri が依存している振る舞いを TreeSink の外から足すことになる。

Lexbor を参考にするのは、CSS マッチャで一度やった方法の繰り返しである。
`lexbor::css_match` は Lexbor の `lxb_selectors_*` の状態機械を安全な Rust に写し、旧実装をテスト専用のフィーチャに残して差分オラクルにした。
Lexbor は Apache-2.0 なので、制御フローの移植は `NOTICE` の記載の範囲で済む。
初期実装では、Lexbor の意味論と制御フローとの対応を可能な限り保つ。
対応が保たれていれば、新実装と Lexbor の食い違いはすべて移植の誤りとして扱えるので、差分オラクルが鋭く効く。
仕様の不変条件と所有権の不変条件は、制御フローを変えずに doc comment と assertion で明示する。
Lexbor からの変更の扱いは「変更の二つの種類」の節で定める。

制約は次の二つである。

- **速度**：パースと走査の速度を落とさない。判定は `rake bench` の各行で行い、「種類としての退行」（Nokogiri と同等以下に落ちる行）を認めない。
- **メモリ**：多少の増加は許容するが、数倍は許容しない。

速度とメモリについて本書が述べる設計上の見込み（状態関数の形、ノードの縮小）は、正しさを確立した後に測定する仮説として扱う。
見込みが外れた場合に設計を変えるのは測定の結果であって、本書の記述ではない。

## 現状の依存範囲

Lexbor への依存は `ext/makiri/rust/src/lexbor/` 配下に閉じている。
テスト以外で呼んでいる Lexbor 関数は 89 個、呼び出し元のファイルは 15 個である。

ただし、依存は adapter の内側だけでは終わらない。
glue、XPath、CSS マッチャ、text index、dom index は `lexbor::adapter::html` の型付きハンドル（`HtmlNode`、`HtmlElement`、`HtmlAttr`、`HtmlDoc`）と、`Dom` トレイトおよび `dom_rules::Tree` トレイトを通して DOM を読む。
一方で bridge は、HTML ノードの identity を Lexbor のノードポインタとして保存し、復元する。
`bridge::wrapper::NodeWord` は「HTML 文書では Lexbor のポインタ、XML 文書ではアリーナの `NodeId`」を 1 ワードに収める型で、ラッパの TypedData と NodeSet のバッファがこの形で持っている。
シリアライザも、走査は Rust が駆動するが、ノード単位の出力は Lexbor のポインタを `lxb_html_serialize_cb` に渡している。
したがって「adapter を差し替えれば glue は無変更」は成り立たず、切り替えの単位には bridge のノード identity と NodeSet、シリアライザが含まれる。

Lexbor が担っている仕事は五つに分かれる。

- **HTML5 パーサ**：tokenizer と tree builder。chunk API と token hook を通して、木の深さガードとソース位置の記録を行っている。
- **DOM 本体**：ノード構造体、mraw アリーナ、名前テーブル、挿入と削除、`import_node`、属性操作。
- **CSS セレクタパース**：セレクタパーサと、CSS 構文トークナイザ（`contains_guard` が使う）。
- **CSS stylesheet パース**：`Makiri::Lexbor::CSS.parse_stylesheet` の規則、at-rule、declaration、値の正規化。
- **シリアライザ**：ノード単位の `lxb_html_serialize_cb` と、`pretty:` の Lexbor 独自レイアウト。

## 出発点の実測

合成文書を現行の Lexbor でパースし、GC に報告しているアリーナ量を読んだ。
文書は 570 KB で、ノード 25,005 個（うち text 10,000）と属性 20,000 個を持つ。

| 項目 | 値 |
|---|---|
| 報告バイト数 | 9.1 MB |
| ノードまたは属性 1 個あたり | 202 B |
| 入力に対する倍率 | 16 倍 |

この量は構造体の大きさから説明できる。
`lxb_dom_node_t` が 96 B で、element はそれに 80 B、attr は 48 B を足す。
text は `lexbor_str_t` の 16 B に加えて mraw の sub-allocation を持つ。
さらに名前テーブル四つ（tag、attr、ns、prefix）と pool の slack が乗る。

u32 の index で引くアリーナにすると、node は約 48 B、attr は約 24 B、文字データは文書ごとの byte アリーナ 1 本に収まる見込みである。
同じ文書で見積もると 3 MB 前後で、Vec の倍増による slack を見ても現行を下回る。
この見積もりは実装後に `bytes/node` の bench 行で測る対象であり、「数倍になる」危険が設計上小さいことを示す根拠として置く。

## 固定するもの

実装に入る前に、何を保つかを文書として固定する。
固定していない契約は、移植の途中で「実装が大変だから」という理由で暗黙に変わるからである。

**既存 API**。
`lib/makiri/` の公開メソッドと `spec/` の全例を保つ。
`Makiri::Lexbor::CSS.parse_stylesheet` は名前に Lexbor を含むので、互換維持ならエイリアスを残し、名前を変えるなら利用側（dommy）の移行を完了条件に含める。

**意図した仕様差**。
`NOKOGIRI_DIFFERENCES.md` と同じ形式で、Lexbor との差分表を新たに置く。
`CLAUDE.md` にすでに記録がある差分（form state は Standard、link は Lexbor、SVG 要素名の case folding、属性名の大文字小文字）を最初の項目にする。
移植中に見つかる食い違いは、後述の分類に従ってこの表に追記する。

**対象とする仕様の版**。
HTML Standard は living standard なので、commit を一つ固定し、差分表（`docs/parser_differences.ja.md`）にその hash を書く。
WPT と html5lib-tests の pin も同時に固定する。
版を進めるときは、三つを同時に進め、変更点を差分表に反映する。
固定した版は差分表の冒頭にある。

**ソース位置の規則**。
`Node#line` は Integer か nil で、間違った行を返さない。
nil になるのは、パーサが生成した要素（暗黙の html、head、body、tbody、colgroup、adoption agency や再構築が作り直す formatting element）、table から foster された void 要素、text、comment、属性、fragment の全ノードである。
現行の実装は `source_loc::stamp_created` の条件で近似しているが、移植後は tree builder が要素を作る時点で「token から作った」か「パーサが補った」かを区別し、前者だけに token の開始位置を書く。
token の開始位置をそのまま書くだけでは再構築された formatting element に元の tag の位置が付くので、生成理由の区別は省けない。
ただし、この規則への切り替えは振る舞いの変更に当たる。
初期実装は生成理由を記録するだけで、判定は現行の `stamp_created` の条件を再現する。
生成理由による規則は、移植の完了後に独立した変更として入れる。

## 変更の二つの種類

Lexbor からの変更は、次の二つに分け、一つのコミットに混ぜない。

- **所有権と安全性のための変更**：観測できる振る舞いを変えない変更。ポインタから `NodeId` へ、mraw から Vec と byte アリーナへ、callback から直接の呼び出しへ、abort する確保から `falloc` へ、chunk 入力と `encoding` の除去、token hook から tree builder の内部への深さガードの移動、性能のための制御フローの変更が該当する。
- **仕様適合のための振る舞いの変更**：Ruby から観測できる結果を変える変更。Lexbor が仕様から逸れている箇所を仕様に寄せるもの、ソース位置の規則の切り替え、`pretty:` と stylesheet の方針で生じる差が該当する。

前者の検証は、参照実装との完全一致である。
構造ダンプ、直列化、`rake diff` の baseline のどれかに差が出たら、移植の誤りとして直す。

後者は、一件ずつ独立したコミットとして入れる。
各コミットは、差分表の一項目、仕様の該当 step、変化を示す最小再現の spec、差分 fuzz の許可リストの一項目をそろえて持つ。
許可リストの項目は「この形の入力でだけ、この差が出る」と述べる述語として書き、差分 fuzz はリストで説明できない差を失敗とする。
こうしておくと、ある振る舞いの変更が別の箇所の振る舞いを巻き添えで変えたときに検出できる。

初期実装の途中で仕様からの逸脱を見つけても、その場では直さない。
差分表に「参照実装の未対応」として記録し、移植が完了してから後者の変更として入れる。
例外は逸脱が安全性に関わる場合（範囲外の読み書き、停止しないループ、資源の上限の欠如）で、その修正は初期実装の中で入れ、参照実装との差を許可リストに載せる。

## 検証基盤

実装の正しさは、次の順で用意する基盤で判定する。
基盤が先で、移植が後である。

### 比較の主対象は DOM の構造

直列化した文字列の一致では、パーサの正しさを判定できない。
同じ HTML に直列化されても、要素の名前空間、隣接する text node の分割、document の quirks mode、template contents の所属は異なり得る。
これらは XPath、CSS、DOM API の結果を変え、しかもパーサの誤りとシリアライザの誤りが相殺して文字列が一致することもある。

比較器は、次を含む構造ダンプを出す。

- node の種別
- 要素と属性の namespace URI、local name、prefix
- 属性の並びと値
- text、comment、PI の文字データ（隣接 text の分割を保ったまま）
- doctype の name、public id、system id
- document の quirks mode
- template contents（子ではなく contents として）

`spec/conformance/html5lib_dump.rb` の html5lib 形式のダンプがこの大半を持っているので、quirks mode を足した形を比較器の出力形式とする。
quirks mode は `Document#quirks_mode` として Ruby の表面に出ており、CSS の class と id の照合の大文字小文字も変えるので、比較対象から外せない。
直列化のバイト一致は、この構造比較とは別のテストとして保つ。

document のパースと、context 要素付きの fragment パースは分けて扱う。
fragment は tree builder の初期状態（insertion mode、open elements、form 要素）が context で変わるので、document の一致は fragment の一致を含意しない。

### 参照実装の保持

Lexbor は `lexbor-reference` フィーチャとしてテスト専用に残す。
`css-reference` がすでにその形で、出荷するビルドには含まれず、差分テストと fuzz だけが使う。

参照実装を残す以上、submodule と cmake の経路は捨てない。
捨てるのは「出荷ビルドが Lexbor を必要とする」状態であり、「検証用に Lexbor を取得しビルドできる」状態ではない。
両者は別の CI ジョブで確認する。
通常ビルドは Lexbor なしで compile と spec を通し、参照ジョブは `lexbor-reference` を有効にして差分テストと fuzz を回す。
参照実装の commit は現在の pin に固定し、Makiri の都合で動かさない。

### オラクル

**HTML Standard** の固定した版を一次の根拠にする。
tokenizer と tree builder の関数は Lexbor の関数と 1 対 1 に対応させ、doc comment には Lexbor の関数名と、仕様の節と step 番号の両方を書く。
仕様は、移植後の振る舞いの変更と監査の根拠になる。

**WPT の `html/syntax/parsing/resources`** から tree construction のテストを取る。
html5lib-tests は 2026-06 に tree construction のテストを WPT に移し、以後の仕様変更（processing-instruction token、form element pointer、frameset-ok）はそちらにしか入っていない。
`html5lib_runner.rb` はすでにこの pin を取得して document と fragment の両方を走らせ、固定した仕様で分類した既知の差を `KNOWN_DIVERGENCES` として差分表の id で持つ。

**html5lib-tests** からは tokenizer のテストを取る。
tokenizer を単体で検証できないと、tree builder の失敗がどちらの層のものか切り分けられない。
tokenizer には Ruby の入り口がないので、runner は Rust の側に置き、Lexbor の tokenizer と新しい tokenizer の両方を同じ runner で走らせる。

**WHATWG の `entities.json`** から entity テーブルを生成する。
Lexbor の `res.h` を写すのではなく、JSON を正とし、生成物が JSON と一致することをテストで確かめる。

**Lexbor 自身**を差分オラクルにする。
cargo-fuzz に `html_diff` を足し、同じバイト列を両方でパースして構造ダンプを比べる。
document と fragment の両方を対象にする。
初期実装の完了条件は、この比較で許可リストに載っていない差が出ないことである。

**`rake diff` の baseline** はそのまま回帰ゲートになる。
記録されているのは C 実装の答えであり、パーサの実装には依存しない。

**ブラウザ**は補助証拠として使い、裁定役にはしない。
二つのブラウザが一致しても、両方が仕様変更に未対応なら、その一致は仕様準拠の実装を誤って退行させる。
コーパスを headless の Chromium と Firefox でパースして構造を比べる rake タスクは用意するが、比較するときは parse の方法（document か fragment か、context 要素）、script の実行条件、`innerHTML` の読み方を揃える。

オラクル同士が食い違ったとき、その食い違いは次の四つに分類し、根拠と最小再現例を差分表に残す。

- **仕様変更**：固定した版の仕様が答えを定めており、テストデータか参照実装が古い。
- **参照実装の未対応**：Lexbor またはブラウザが仕様を実装していない。
- **Makiri の不具合**：直す。初期実装の間は、参照実装との食い違いはすべてこれに当たる。
- **意図した互換差**：Nokogiri や Lexbor に合わせると決めた差。差分表に理由を書く。

### 性能とメモリのゲート

`rake bench` の各行に「種類としての退行なし」を課す。
前述の計測を `bytes/node` の行として bench に加え、現行の 202 B を上限の目安にする。
`rake oom` と `fuzz:sanitize` はそのまま効く。
mraw の盲点がなくなるので、ASan はパーサ全域を見るようになり、`sanitize:lexbor` は不要になる。

## アリーナ DOM

### 切り替えの単位

アリーナ DOM は、mutation と import、シリアライザ、bridge のノード identity を含めて一つの単位として完成させ、完成するまではテスト用の backend として現行と並存させる。
部分的に切り替えると既存 API を維持して出荷できない。
現行のシリアライザは Lexbor のポインタを C に渡すので新 DOM を出力できず、bridge はポインタを保存しているので新 DOM のノードをラッパにできない。

並存の形は、XML 側がすでに持っている。
`NodeWord` は `NodeId` からも作れ、`wrap_xml_node` はアリーナのノードをラッパにしている。
HTML のアリーナも `NodeId` の形で `NodeWord` に入り、文書の TypedData 型がどちらの表現かを決める。

### backend の選び方と API の形

アリーナは、`lexbor::adapter` と同じ名前と同じメソッドを持つ二つ目の実装として書き、Cargo のフィーチャ（`arena-dom`）でビルド時に選ぶ。
glue、bridge、XPath の HTML backend、CSS マッチャ、各 index は、どちらの実装かを知らずに一つの facade のパスから型を取る。
実行時に切り替えないのは、二つの表現を一つのビルドに持つと、ノード一つごとに分岐が入り、走査の速度に跳ね返るからである。
`arena-dom` のビルドで `rake spec`、`rake diff`、`rake conformance:parser` を通すことが、この段の完了条件になる。

読み取りの API は今の形のまま移せる。
今のハンドル（`HtmlNode<'doc>` など）は Lexbor のポインタを持つ `Copy` の値で、アリーナではアリーナへの参照とノードの index を持つ `Copy` の値になる。

書き込みの API は、移す前に形を変える。
今は読み取りのハンドルと書き込みのハンドル（`HtmlNodeMut` など）が同じ Lexbor のメモリを同時に指しうる。
安全な Rust では、伸びる Vec を共有参照で読みながら同じ Vec に書くことはできない。
そこで HTML の編集を、XML 側の `Editing::with_arena` と同じく、編集の間だけアリーナを `&mut` で渡す形にする。
HTML の編集はすでに一つの入り口（`bridge::html::edit` から `HtmlEdit::node`）を通っているので、変える箇所はその先に限られる。

この形の変更は、アリーナを書く前に Lexbor の backend の上で行う。
振る舞いは変わらないので、既存の spec と `rake diff` がそのまま検証になり、所有権と安全性の側の変更として扱える。
アリーナはその後、同じ API の二つ目の実装として入る。

**決定（2026-10-18）**：最終形を index のアリーナとし、ポインタを保つ DOM は経由しない。次の 1、2、3 の順で進め、各段で変える物を一つに絞る。1 は bridge を Lexbor の backend のまま直す段で、完了した（`RawNode` は文書の中への参照になり、編集は `&mut HtmlDocMut` を貸すクロージャの中だけで行う）。2 はアリーナを Lexbor の解析結果の複製で作る段で、これも完了した（「進捗」を参照）。3 は tree builder の移植である。以下は判断に至った調査の記録である。

**調査（2026-10-18）**：bridge は HTML のノードを「それだけで生きたノードに戻せる 1 ワード」として扱っている。
`RawNode` を使う箇所が bridge に 45、`NodeWord` が 33 あり、XPath のトークン、各 index と子の位置メモの表（`PtrTable` のキー）、ラッパのキャッシュの identity も同じ前提に立つ。
u32 の index だけでは、どの文書のノードかが分からないので、この前提を満たせない。
選択肢は二つで、どちらを取るかで第 3 段の作業量と、移植後の DOM の安全性の形が変わる。

- **ポインタを保つ DOM**：ノードを Lexbor の mraw と同じくアドレスの動かない塊に置き、ノード同士をポインタで結ぶ。`RawNode` は今と同じ 1 ワードのポインタのままで、bridge と adapter の API は変わらず、tree builder の移植も Lexbor の DOM 呼び出しをほぼそのまま写せる。代わりに DOM の核に `unsafe` が残る。ノードは Lexbor より小さく（要素で 80 バイト前後）、メモリの制約は満たす。
- **index のアリーナ**：u32 の index で結び、`#![forbid(unsafe_code)]` の下に置く（`xml/arena.rs` と同じ形）。ノードはさらに小さい（要素で 48 バイト前後）。代わりに、上の約 80 箇所を「文書と index」で引く形に先に直す必要があり、書き込みも下の 2 のとおり `&mut` を渡す形に変える。

前者を先に取り、後者への切り替えを所有権と安全性の側の独立した変更として後に回す道もある。
この選択は第 3 段に入る前に決める。
tokenizer はノードではなく token を出すので、この選択に依存しない。決まるまでは tokenizer の移植を先に進める。

作業は次の順に区切り、それぞれを単独でコミットできる状態にする。

1. facade のパスを作り、`lexbor::adapter` の外の利用者をすべてそこへ向ける（振る舞いは変わらない）。
2. HTML の編集を、編集の間だけ文書を `&mut` で渡す形に変える（Lexbor の backend の上で。振る舞いは変わらない）。
3. アリーナの読み取り、名前の intern（Lexbor と同じ静的な id の範囲を保つ）、Lexbor の解析結果からの複製を書き、`arena-dom` のビルドで読み取り系の spec を通す。
4. 編集、import、fragment、シリアライザ（`pretty:` を含む）を書き、`arena-dom` のビルドで spec 全体と `rake diff` を通す。
5. `rake oom`、ASan、ファザー、`GC.compact` を `arena-dom` のビルドで通す。

### ノードの構成

ノードは u32 の `NodeId` で引く。
`xml/arena.rs` がすでにこの形で、`#![forbid(unsafe_code)]` のまま動いている。
Vec が伸びても index は変わらないので、ポインタより安全である。

node に持たせるのは、親、最初の子、最後の子、前後の兄弟（u32 を 5 つ）、種別、名前空間 id、local name id、ソース位置、フラグである。
element は属性リストの先頭と末尾を足す。
属性は別の Vec に置き、値は byte アリーナの span で指す。
text、comment、PI の data も同じ byte アリーナの span である。

### byte アリーナは追記型から始める

byte アリーナは、最初の実装では追記のみとし、古い span を回収しなかった。
`content=` を繰り返す文書は育つが、GC への報告は capacity の和として正確に出るので、育った分は Ruby の GC が見る。

圧縮は独立した後続の作業とし、次のように定義して実装した（`html/dom/compact.rs`）。

- **生存判定**：detach 済みのノードや削除済みの属性もラッパから読めるので、それらの span は生きている。生存は「アリーナ内の全ノードと全属性が指す span」であり、木に繋がっているかどうかではない。
- **実行できる境界**：借用中の文字列（`RubyText` の借用、text index の `RawSlice`、XPath の評価中の `Borrowed` 値）がない時点に限る。index を落とし、評価中の文書を拒む `bridge::html::edit` の開始点がその境界である。
- **失敗時の原子性**：生きたバイト数を数えてから新しいバッファをその大きさで `falloc` で確保し、全 span を写してから差し替える。確保に失敗したら古いバッファのまま編集を続ける。当初は raise するとしていたが、圧縮は答えのために要るものではなく、古いバッファには全バイトが残っているので、失うものがない。続く編集の確保が失敗すれば、そこで raise する。
- **計算量**：「死んだバイト数が生きたバイト数を超えたら」だけでは償却線形にならない。空要素が大量にあり文字列が少ない文書では、小さな更新のたびに全ノード走査が起きる。閾値は、死んだバイト数が「生きたバイト数」と「ノード数と属性数の和に定数を掛けた量」の両方を超えたときとし、走査のコストを死んだバイト数で償う。

## tokenizer と tree builder

### 入力前処理と借用

HTML Standard の入力前処理は、CR LF と単独の CR を LF に正規化し、NUL を文脈に応じて U+FFFD に置き換えるか捨てる。
現行の `utf8_input.rs` は UTF-8 の修復だけを担当し、改行と NUL は Lexbor の tokenizer が状態関数の中で扱っている。
したがって「文字参照以外は入力のスライスをそのままコピーする」は成り立たない。

設計は次のとおりとする。
token は入力の位置の対（開始、終了）を持つ。
text と属性値は、改行正規化、NUL の処理、文字参照の展開のどれも必要なかった場合に限り、入力のスライスから byte アリーナへ memcpy する。
どれかが必要なら `falloc` のバッファへ書き、そこからアリーナへ移す。
元入力の位置は、正規化後の文字列とは別に保つ。
ソース位置は元入力の位置であり、文字列の長さから逆算しない。

### tokenizer

Lexbor の tokenizer は状態関数が chunk の末尾まで進む形で、Rust でも `fn(&mut Tokenizer, &[u8]) -> usize` の関数ポインタをループで回す形を仮説として採る。
enum と match で 1 バイトずつ dispatch する形より速いと見込むが、これは測定の対象であり、正しさが揃った後に両方を測って決める。
形を変える場合、それは所有権と安全性の側の変更として扱い、参照実装との完全一致で検証する。

chunk 入力の機構は要らない。
Makiri は常に文書全体を 1 バッファで渡すので、Lexbor の状態関数が至る所で扱う chunk 境界（`is_eof`、`markup` の退避、cross-chunk の UTF-8 バッファ）は持たない。

タグ名と属性名は id に intern し、tree builder は id で match する。
静的なタグ表は build 時に生成し、文書固有の名前は `falloc` で確保する map に入れる。

### tree builder

insertion mode ごとに 1 モジュールとし、制御フローは Lexbor の対応する関数に従い、doc comment に仕様の節の step を写す。
木の深さガードと `<select>` の option 数の上限は、tree builder の挿入処理に直接書く。
現行の `tree_guard` が token hook で外から数えているのは、Lexbor の tree builder に手を入れられなかったからである。

要素を作る箇所は「token から作った」か「パーサが補った」かを引数で受け取る。
初期実装はこれを記録するだけで、ソース位置の判定は現行の規則を再現する。

### 捨てるもの

Lexbor のうち、次は移植しない。

- **`encoding` モジュール（約 32 万行）**：入力は Ruby 側で UTF-8 への変換を済ませてから渡しており、`meta charset` の prescan も要らない。
- **chunk 入力の機構**：前述。
- **`serialize_ext.c`**：Makiri の表面に出ていない。
- **element の `style`、`list`、`is_value`、`custom_state`、node の `event_target`**：同じく表面に出ていない。
- **`node->user`**：ソース位置はノードの専用フィールドに持つ。

## シリアライザ

通常の直列化は HTML Standard の fragment serializing algorithm で、ノード単位の出力を Rust で `Buf` に直接書く。
現行の per-chunk callback がなくなる分、速くなる見込みである。
`lexbor::tests::serialize_walk` のバイト一致テストを、参照実装に対して保つ。

`pretty:` は別の問題である。
HTML Standard は通常の直列化しか定義しておらず、Lexbor の pretty 出力に対応する標準のレイアウトはない。
選択肢は「Lexbor のレイアウトをそのまま移植する」か「`pretty:` を廃止する」かの二つで、「Standard に寄せる」という選択肢はない。
既存 API を保つ方針なので、Lexbor のレイアウトを移植し、参照実装とのバイト一致で検証する。

## CSS

### セレクタパーサ

Lexbor の制御フローのまま Rust に移す（`src/selectors/`）。
セレクタの状態関数は汎用の構文パーサ（規則のスタックと状態のスタック、規則ごとの phase）の上で動き、エラーからの回復や寛容なリスト（`:is()`、`:where()`、`:has()`）の挙動はその組み合わせで決まるので、構文パーサの必要な部分ごと移す。
`css_match::compile` と `css/lower.rs` は `css_parser` の `Simple`、`Selector`、`List` のビューだけを消費しているので、同じ形のビューをアリーナの AST の上に置けば、消費側は import を変えるだけで済む。
`selector_cache` の別アリーナ、`Parsed` の Drop がアリーナ全体を掃除する問題、セレクタ解析用の `GvlCell` は、ここで消える。
`contains_guard` と `css_tokens` は、stylesheet パーサを移植するまで Lexbor の stylesheet パーサの前に残った。
検証は、Lexbor のパーサとのビューの比較（`lexbor::selector_diff`、`selector_diff` fuzz）、`spec/conformance/css_diff.rb` と `css_corpus.rb`、`html_css_diff` fuzz で行う。

### stylesheet パーサ

stylesheet パーサは、セレクタパーサとは独立した移植対象である。
`parse_stylesheet` は規則、at-rule の name と prelude と入れ子、declaration の name と value と important を返し、値は Lexbor がプロパティを型付きで解釈して正規化した形で返す。
`spec/lexbor_css_spec.rb` は「`margin: 1px   2px` が `1px 2px` になる」ことを既存の契約として固定しており、dommy がその cascade でこの出力を使っている。

選択肢は三つあった。
互換維持は、Lexbor のプロパティごとの文法（約 9,300 行）を移植する。
API 変更は、CSS Syntax Level 3 の generic な declaration（名前と生の値のトークン列）を返し、dommy 側の移行まで完了条件に含める。
中間案は、返す形（name、value、important）を保ち、値の正規化をプロパティによらない構文の規則に置き換える。

Lexbor の正規化は一貫していない。
型を持つ約 100 のプロパティだけを正規化し（`margin: 1px   2px` は `1px 2px`、`color: RED` は `red`）、未知のプロパティとカスタムプロパティは前後を削るだけで、`calc()` の中も詰めない。
どこまで正規化されるかが Lexbor の版で変わる契約を、移植の費用をかけて写す理由はない。

中間案を採る（2026-10-18 決定）。

- **構造は Lexbor のまま**：規則のリスト、at-rule（名前、prelude、入れ子）、qualified rule と `:bad_style`、宣言の境界、誤りからの回復、`!important` の判定、値の offset は、Lexbor の構文パーサとコールバック（`css/syntax/parser.c`、`css/state.c`、`css/at_rule/state.c`）を移植して保つ。
- **値**：宣言の値の範囲（Lexbor と同じ offset）を CSS Syntax のトークンとして読み直し、空白トークンの連続を空白一つにし、前後の空白を削る。それ以外のトークン（文字列、url、エスケープ、コメントを含む）は書かれたとおりに残す。プロパティごとの状態は使わない。
- **名前**：カスタムプロパティ（`--` で始まる）以外の宣言名は ASCII 小文字に揃える。at-rule の名前も ASCII 小文字に揃える。
- **変わる答え**：プロパティ固有の正規化（キーワードの小文字化、`url( x )` の空白、単位の大文字小文字）はしなくなる。未知のプロパティとカスタムプロパティの値の空白は詰まり、未知のプロパティと独自の at-rule の名前は小文字になる。いずれも CSS の値としての意味は変えない。

移植後、出荷するビルドは Lexbor の CSS モジュールを使わない（`contains_guard`、`css_tokens`、`css_engine`、`css_parser` は比較用の構成だけに残る）。

## 仕様全体の監査

移植が完了したら、対象とした仕様の版の全体から実装を監査する。
差分表に載っている既知の差を確かめるだけでは足りない。
差分表はテストとオラクルが食い違った箇所しか拾わないので、どのオラクルも覆っていない step の逸脱は残るからである。

監査の範囲は、HTML Standard の入力前処理、tokenization の全状態、tree construction の全 insertion mode と補助アルゴリズム（adoption agency、foster parenting、active formatting elements の再構築、template の insertion mode のスタック、quirks mode の判定）、fragment parsing algorithm、fragment serializing algorithm、名前付き文字参照の表である。
CSS は、Selectors Level 4 と CSS Syntax Level 3 のうち、セレクタパーサが受け付ける文法の範囲と、stylesheet パーサの方針で対象にした範囲である。

監査の成果物は、仕様の step ごとに、対応するコードの位置、対応の状態（一致、意図した差、未実装、逸脱）、根拠となるテストを記した対応表とする。
表は `docs/` に置き、仕様の版を進めるたびに、変わった節だけを見直す。
「一致」とした step がテストで覆われていなければ、テストを足すか、覆っていないことを状態に書く。
監査で見つけた逸脱は四分類に従って扱い、直すものは仕様適合の変更として一件ずつ入れる。

## 作業の順序

1. **固定**。既存 API、意図した仕様差の表、対象とする仕様と WPT と html5lib-tests の版、ソース位置の規則を文書にする。`parse_stylesheet` の互換維持か API 変更かも、ここで決める。
2. **検証基盤**。構造ダンプの比較器、html5lib の tokenizer テストの取り込みと runner、WPT の取り込み、`entities.json` からの生成、`lexbor-reference` フィーチャと `html_diff` fuzz（document と fragment）、差分 fuzz の許可リストの仕組み、ブラウザ比較の rake タスク。
3. **アリーナ DOM をテスト用 backend として完成させる**。アリーナ、mutation と import、シリアライザ（`pretty:` を含む）、bridge のノード identity と NodeSet。この段では Lexbor でパースしてアリーナにコピーし、XPath と CSS と text index と `rake diff` を新 backend で通す。すべて所有権と安全性の側の変更なので、現行との完全一致で検証する。
4. **tokenizer と tree builder の忠実な移植**。Lexbor の制御フローに対応させて書き、tokenizer テスト、tree-construction と WPT、参照実装との差分 fuzz で検証する。正常な入力だけでなく、異常な入力（深い入れ子、巨大な属性、EOF の位置ごと）の意味論も比べる。完了条件は、参照実装との差が許可リストの安全性修正だけであること。仕様からの逸脱はこの段では直さず、差分表に記録する。
5. **CSS**。セレクタパーサを同じく忠実に移植し、stylesheet パーサは 1 で決めた方針で扱う。API を変える方針なら、その変更は 7 で入れる。
6. **新経路の堅牢性と切り替え**。OOM 注入、panic の latch、小さい Fiber の stack、`GC.compact`、並行 parse、失敗後の文書と context の再利用を新経路で確認し、bench で種類としての退行がないことを確かめてから切り替える。
   byte アリーナの圧縮（「byte アリーナは追記型から始める」）も切り替えの条件とした（実装済み）。Lexbor は古い文字列を mraw に返して再利用するので、`content=` や属性値の更新を繰り返す文書が育ち続け、span の合計が 4 GiB で例外になるのは、追記型のままでは現行からの退行である。ここまでが初期実装である。
7. **仕様適合の変更**。差分表の「参照実装の未対応」と、ソース位置の規則の切り替えを、一件ずつ独立したコミットとして入れる。
8. **仕様全体の監査**。対応表を作り、見つけた逸脱を 7 と同じ手順で入れる。
9. **性能の最適化と出荷依存の除去**。bench の各行と `bytes/node` を測り、状態関数の形やノードの大きさの仮説をここで検証する。制御フローを変える最適化は、参照実装との一致（許可リストの差を除く）で検証する。通常ビルドから submodule、cmake、bindgen、`memory.rs`、`sanitize:lexbor` を外し、参照ジョブだけが Lexbor を取得する形にする。

3 を 4 より先に置くのは、tree builder が書き込む先を一度だけ相手にするためである。
Lexbor の DOM に tree builder を向けると、その癖（`attr_append` が `attr_id` の裏の属性を破棄する、など）に合わせて一度書き、アリーナに移してもう一度合わせることになる。

## 進捗（2026-10-18 時点）

ブランチ `parser-port/foundation` で次を終えた。

- **固定**：仕様の版（HTML Standard `de953a7`、WPT `5ffee20`、html5lib-tests `c777c40`、`entities.json` の digest）と差分表（`docs/parser_differences.ja.md`）を置いた。既存 API は `spec/api_surface_spec.rb` がすでに固定している。`parse_stylesheet` は中間案を採る（「stylesheet パーサ」の節）。
- **検証基盤**：tree construction のデータを WPT から取るように改め、Ruby の runner と crate の runner が同じ既知差分の一覧を読む。現行の Lexbor は WPT の 1,959 件中 1,951 件を通し、残る 8 件は差分表の P1 から P3（いずれも Lexbor が固定した版の仕様を未実装）に分類した。tokenizer の html5lib テストは Lexbor の tokenizer で 6,819 件すべてを通す。ブラウザ比較（Chromium、Firefox、WebKit）を用意し、オラクル同士の食い違いを B1 から B3 として記録した。
- **facade**：`lexbor/` の外の利用者は `crate::html_dom` から DOM を取る。
- **静的な表**：タグ、属性、名前空間の名前を Lexbor の id のまま生成し、文字参照の探索木を `entities.json` から Lexbor と同じ手順で作った。どちらも Lexbor の表と要素ごとに一致する。
- **tokenizer**：`src/html/tokenizer/` に Lexbor の状態関数を一対一で移植した。html5lib の tokenizer テストを 6,819 件すべて通し、Lexbor の tokenizer とは token、位置、parse error のすべてで一致する（html5lib の入力と、無作為な 20,000 入力、全初期状態）。

性能の測定（release、2.8 MB の bench 文書）では、tokenizer 単体が Lexbor の 1.07 倍の時間である。入力の種類別では、名前付き文字参照だけが約 1.3 倍で、名前の 1 文字あたり約 1 ns 遅い。原因は特定できていない（探索木の配置を縮めても変わらなかった）。

- **アリーナ DOM（第 3 段）**：`src/html/dom/` に Lexbor アダプタと同じ API のアリーナを置いた。読み取りハンドル、編集（ファクトリ、DOM の属性アルゴリズム、`textContent`、配置、文書内外の複製）、パース済み文書と索引、シリアライザ（`pretty:` を含む）、フラグメント、HTML と XML の相互コピー、XPath の `Dom` 実装がそろう。パースは Lexbor のまま行い、木をアリーナに複製する。`arena-dom` フィーチャ（`MAKIRI_ARENA_DOM=1`）で facade がアリーナを選ぶ。ブリッジ側の差は `bridge::dom` にまとめた。
- **アリーナの検証**：`lexbor::arena_tests` が、WPT の全文書入力について Lexbor アダプタとアリーナを比べる。全ノードの読み取り、無作為な編集列のあとの全読み取り、4 種類のシリアライズのバイト列、フラグメント（要素文脈と名前文脈）、相互コピー、XPath（40 式、strict と lax）、CSS（Lexbor 自身の旧エンジンと比較）が一致する。アリーナのビルドで Ruby の全スペック（1,870 件）、`rake diff`、`rake oom` が通る。

アリーナの移行で、Lexbor の挙動として次の二つを記録した。どちらも観測できる結果は変えない。

- `lxb_dom_attr_set_name_ns` は、接頭辞付きの属性の `local_name` に名前表の id ではなくエントリのポインタを入れる。名前の読み出しはどちらでも同じ文字列になり、名前による検索は修飾名を比べるので、アリーナは id を持つ。
- 生成済みの `TAG_UPPER_NAMES` は大文字ではなかった（`lxb_tag_name_upper_by_id` は静的 id に小文字の項目を返す）。生成器を直した。

bench（release、同一マシン）でのアリーナ構成と Lexbor 構成の比は次のとおり。parse は 0.71 倍（複製の分。tree builder の移植で消える）、`to_html` は 1.51 倍、CSS `ul li.item` は 0.80 倍、`//li` と属性軸は約 0.9 倍で、残りは誤差の範囲である。ノードあたりのメモリは Lexbor の約半分（90 B と 178 B）である。

残っている確認は二つある。ASan での全スペックは nightly ツールチェーンがこの環境にないため未実施である。CSS の 0.80 倍は第 9 段より前に原因を調べる。

- **tree builder（第 4 段）**：`src/html/tree/` に Lexbor の `tree.c` と全 insertion mode を一対一で移植し、アリーナへ直接書く形にした。tokenizer は名前表を sink から借りる形に改め（`TokenSink::names`）、tree builder が文書の名前表を持つ。タグのカテゴリ、SVG のタグ名の綴り、属性の調整表は Lexbor の `tag_res.h` と `tree_res.h` から生成する（`html/res/tree.rs`）。`arena-dom` のビルドは Lexbor からの複製をやめ、文書と fragment の両方を移植した tree builder でパースする。木の深さガード、`<select>` の option 数の上限、ソース位置の記録は、Lexbor の token hook と同じく各 token の前後で判定する（`html/tree/guard.rs`）。挿入の時点で判定すると、上限で push してすぐ pop する void 要素のように、hook が受け入れていた入力を拒むためである。
- **tree builder の検証**：`lexbor::tree_diff` が、Lexbor の木をアリーナに複製したものと移植版の木を、全ノードの種別、名前、名前空間、接頭辞、属性とその並び、`id`/`class` の参照、文字データ、template contents、ソース位置、quirks mode まで比べる。WPT の文書 1,753 件と fragment の全件、無作為な文書 10 万件と fragment で一致し、ガードの判定（深さ 397〜401、option 10,000 と 10,001）も一致する。cargo-fuzz に `html_diff`（`lexbor-reference` フィーチャ）を足した。実際の Web ページ 10 万件（web100k、展開後 22 GB）でも、全ページで木とガードの判定が一致した（`rake conformance:web100k`、CI では回さない）。アリーナのビルドで Ruby の全スペック（1,842 件）、`rake diff`、`rake oom` が通る。

移植で、Lexbor の挙動として次の二つを記録した。

- Lexbor のパーサは tokenizer に keep-duplicate を設定し、重複属性を tokenizer では落とさず、tree builder が名前と名前空間で照合して落とす。このため `<math xmlns=y xmlns=y>` は、一つ目の名前空間が XMLNS に調整された後で照合をすり抜け、属性が二つ残る。移植版の tokenizer にこの設定を足した。
- Lexbor の DOM は、パース中に要素の挿入と属性の追加と open elements からの pop に反応し、option の selectedness を更新して、選択中の option の子を `<selectedcontent>` に複製する。移植版はこれを `html/tree/select.rs` で同じ時点に行う。Lexbor の selectedness の設定アルゴリズムは、display size が 1 でないときに未初期化の値を読むので、その場合は step 2 を行わない（差分表の S1）。

性能（release、2.6 MB の文書）は、移植版のパースが Lexbor 単体の 1.01 倍、これまでの Lexbor でのパースと複製の 0.57 倍の時間である。

- **セレクタパーサ（第 5 段）**：`src/selectors/` に、Lexbor の CSS 構文 tokenizer（`css/syntax/state.c`、数値の変換に使う `core/strtod.c` と `diyfp`）、構文パーサの規則と状態のスタック（`css/syntax/parser.c`、`css/parser.c`、`css/state.c`）、セレクタの状態関数（`css/selectors/state.c`、`pseudo_state.c`、`syntax/anb.c`）を一対一で移植した。解析結果はアリーナの AST で、`lexbor::css_parser` と同じ形のビューで読む。疑似クラスの表は Lexbor の id のままで、`lexbor-contains` だけは載せない（未知の関数として拒むので、これまでの `contains_guard` の結果と同じになる）。HTML の `css`、`at_css`、`matches?`（`selector_cache` 経由）と、XML の CSS から XPath への変換は、移植版で解析する。`selector_cache` は Lexbor の共有アリーナをやめて解析結果を所有して持ち、キャッシュの方針と定数はそのままにした。Lexbor のセレクタ解析（`css_parser::parse` と `css_engine` のセレクタ用の部分）は比較用の構成（`lexbor-reference`、`css-reference`）にだけ残り、出荷するビルドで Lexbor の CSS パーサを使うのは stylesheet の読み取りだけになった。
- **セレクタパーサの検証**：tokenizer は Lexbor の tokenizer と token ごとに一致する（種類、offset、長さ、文字列、数値、無作為な 50 万入力）。パーサは `lexbor::selector_diff` が両者のビュー（リストごとの specificity、単純セレクタごとの結合子、名前、名前空間、種類と引数、入れ子のリスト）を比べ、手書きの約 250 件、リポジトリのスペックとテストにある 308 件、文法に沿った無作為な 100 万件で一致した。cargo-fuzz に `selector_diff` を足した。移植版で解析したセレクタを旧エンジン（Lexbor の解析と照合）と比べる既存の差分テストも通る。Ruby の全スペック（1,842 件）、`rake diff`、`rake oom`、`rake symbols` が通る。

移植で、差分表に二つを加えた。S2 は An+B の値が double で 2^63 になる整数の扱い（Lexbor は未定義の変換、移植版は飽和）、S3 はパーサ自身のメモリ不足（Lexbor は途中までのリストを成功として返す、移植版は失敗する）で、S3 は `rake oom` が見つけた。

性能は、解析だけを比べると移植版が Lexbor の 1.5〜2.5 倍の時間である（1 回 0.25〜1.1 µs、release）。Lexbor は作っておいた解析器を使い回すが、移植版は解析ごとに十前後の `Vec` を確保する。HTML の問い合わせは解析結果をキャッシュするので、繰り返すセレクタでは差が出ない（同じ Ruby のマイクロベンチで、`css`、`at_css`、`matches?` は揺れの範囲）。毎回違うセレクタを渡す経路では 5〜10%、キャッシュのない XML の CSS では数 % 遅い。解析器の作業領域の使い回しは第 9 段で扱う。

- **byte アリーナの圧縮**：死んだバイト数を数え（`set_data` の置き換えと、`append_text` が末尾へ移した元のデータ）、生きたバイト数と、ノード数と属性数の和の 8 倍の両方を超えたら、編集の開始点（`HtmlParsed::edit`、索引を捨てた直後）でストアを書き直す。単体テストで、置き換えを繰り返したストアが上限内に収まり、切り離したノード、属性値、PI、doctype の読み取りが変わらないことを確かめた。arena と Lexbor DOM に同じ無作為な編集を加える比較（`edits_leave_the_arena_reading_as_lexbor_does`）には、4 回に 1 回の強制的な圧縮を挟んだ（400 系列で一致）。arena のビルドで、4 KB のテキストを 5 万回置き換えても RSS は 44 MB で止まる（圧縮がなければ 200 MB を超える）。Ruby の全スペック（1,843 件）、`rake diff`、`rake oom` が通る。

- **stylesheet パーサ（第 5 段の残り）**：中間案で移植した。`src/selectors/parser/syntax.rs` に、構文パーサの規則の phase（rule list、at-rule、qualified rule、block、declarations、`css/syntax/parser.c`）、規則を作るコールバック（`css/state.c`、`css/at_rule/state.c`）、tokenizer の先読み（`lookup_colon`、`lookup_important`、`lookup_declaration_ws_end`）を、セレクタパーサと同じ規則のスタックの上に移した。規則は `selectors/rules.rs` のアリーナに作り、qualified rule の prelude はセレクタパーサの状態がそのまま読む。宣言の値は一つの状態で読み、値は `lexbor/stylesheet.rs` が offset の範囲を読み直して作る。到達しない部分（宣言リスト単体の解析で使う名前の検査と drop phase）は移していない。出荷するビルドは Lexbor の CSS パーサを使わなくなり、`contains_guard`、`css_tokens`、`css_engine`、`css_parser` は比較用の構成（`lexbor-reference`、`css-reference`）とテストにだけ残る。
- **stylesheet パーサの検証**：`lexbor::stylesheet_diff` が両者の規則の木（規則の種類、offset、セレクタ、at-rule の種類と独自の名前、宣言の名前と値の offset と `!important`、ブロックの入れ子、style rule が子として持つ規則）を比べ、手書きの約 110 件、文法に沿った無作為な入力、token を無作為に並べた入力で一致した。cargo-fuzz に `stylesheet_diff` を足した。Ruby の全スペック、`rake diff`、`rake oom`、`rake symbols` が通る。

次は第 6 段（既定の切り替え）の前の確認で、`html_diff`、`selector_diff`、`stylesheet_diff` の fuzz を CI で回して食い違いを拾うことと、bench で種類としての退行がないことの確認である。
