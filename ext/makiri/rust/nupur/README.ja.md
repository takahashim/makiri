# Nupur（ヌプㇽ）

Nupur は Makiri の拡張（extension）における Ruby バインディングレイヤーです。
これにより、拡張ライブラリは Ruby の C APIに対応する安全な API を利用できます。これには Ruby の値とそのクラスチェック済みハンドル、例外、メソッドトランポリン、TypedData、GVL、および例外を発生する可能性のあるすべての C 呼び出しに対するラッパーが含まれます。
Ruby とやりとりするすべての `unsafe` コードは、ただ一つの例外を除いて Nupur またはその配下のクレートに実装されています。その唯一の例外は、拡張ライブラリの `Init_` であり、定義を `run_init` に渡します。

レイヤー構成（上から順に）：

```
makiri_rs   拡張ライブラリ：Makiri の Ruby メソッド
nupur       nupur_sys 上に構築された安全な API
nupur_sys   (sys/) Ruby の C API。各呼び出しは証明（proof）を受け取る。rb-sys に依存する唯一のクレート
rb-sys      Ruby の C API（生成されたバインディング）
```

Nupurは拡張ライブラリ自体に依存しておらず、その存在すら認識していません。Nupurが拡張から必要とするもの（破損した内部不変条件に対する例外クラスや、`rake oom` フック）は、拡張によって `Init_` に登録されます。この拡張は`crate::nupur` としてNupurに公開され、その `bridge` は `rb`、`typed`、`gvl`、`build`、`hash`、および `args` を再エクスポートします。

## 保証しているもの

Nupurのルールの大部分は、その型とクレートの依存関係であり、これらを破るコードはコンパイルできません。`src/lib.rs`にそれぞれ詳細が記載されています。

- **raiseがRustの後始末を飛び越えない** raiseは`longjmp`であり、またいだRustフレームのデストラクタを実行しません。raiseを引き起こし得るすべてのC呼び出しは、`&Protected`を受け取る`nupur_sys::raising`のラッパーとなっています。`&Protected`を作るのは`rb_protect`のサンクだけで、サンクはそれを`raw::call`が実行するクロージャに貸し出すため、raiseは`Err`として返されます。NupurにはC関数を呼び出す他の手段はありません。rb-sysに依存しておらず、`nupur_sys`もC関数を再エクスポートしていません。保護の外でraiseが起こるのは次の2か所で、どちらも解放すべきものを持たないフレームしかまたぎません。
  - メソッドのトランポリンが`Err`をraiseするとき（または`break`や`throw`を再開するとき）と、`Init_`が`fatal`をraiseするとき。いずれもメソッドのすべてのRustフレームが復帰した後です。
  - ラップしたオブジェクトのマークコールバック内の`rb_gc_mark`（`nupur_sys::non_raising::gc_mark_in_dmark`）。Rubyのmark stackを伸ばせないとき、`NoMemoryError`をraiseし得ます。コールバックは後始末の要るものを何も持たず、このリスクはどのC拡張のマーク関数にも共通です。
- **Cに到達するpanicを禁止する** トランポリンはpanicを内部エラーに変換します。Cが呼び出すコールバックがpanicを捕捉し（`caught::PanicLatch`）、その呼び出し元が再開できるようにします。巻き戻り(unwind)してはならないコードは`no_unwind`サイトであり、`--cfg nupur_no_unwind`を指定したリリースビルドによって、それが不可能であることが証明されます（`rake no_unwind`）。
- **GVL は型になる**：GVL を必要とする Ruby の C APIを呼び出すには、GVL を保持していることを型レベルで保証する値を要求します。その保証する値には、対象となる Ruby の値（Nupur の `Value` が内部的に保持する `nupur_sys::value::Value`）か、対象となる値が存在しない場合にはトークン `Ruby` を使用します。どちらも `Send` を実装していないため、GVL を解放した状態で実行される処理の内部に持ち込むことはできません。値を生成できるのは `nupur_sys` の内部だけで、トークンを生成できるのは `rb::method` の非公開関数 `entry_token` のみです。この関数は Ruby から呼び出されるエントリポイントでのみ使用されます。
- **GCはラップされたオブジェクトが保持するすべてのRuby値を認識する** GCはラップされたオブジェクトのRustデータ内部を参照できないため、オブジェクトが生存させている値（MakiriノードはDocumentを保持します）は、そのマークコールバックによって報告される必要があります。Nupurはこのような値をスロット（`nupur_sys::value::Root`）に保持し、決してRustデータには保持せず、自らマークを行います。拡張機能はマーク関数を記述しないため、マークを忘れることはありません。
- **ラップされたオブジェクトはそのデータの借用期間よりも長く生存する** データには`typed::Typed` ハンドルを通じてのみアクセスでき、このハンドルが最後の借用が終了するまでオブジェクトを生存させ続けます。
- **Stringの読み取りはコピーで行う** 文字列のスライスが渡されることはありません。テキストはstdのバリデータによってUTF-8であることが確認されたコピーであり、Stringのキャッシュされたコードレンジから信頼されることはありません。
- **Cの整数は1箇所で変換する** (`nupur_sys::int`)。Cの `long` はWindowsでは32ビットであるため、変換処理はどちらのビット幅にも対応するよう１ヶ所に記述され、両方でテスト・検証されています (`verus/int.rs`)。
- **2つのRactorが同時にこのレイヤーで実行できる**。拡張ライブラリが自身をRactorセーフであると宣言した場合 (`rb::method::declare_ractor_safe`)、2つのRactorが同時に実行できます。Nupurの要素で、任意のRactorが読み取り可能なもの以外は、両者間で共有されません。

### 型で保持できないこと

一部のルールは型として表現できません。Makiri ではこれらをコードのチェック（`lints/makiri_lints` にある `rake dylint` の lint、`rake no_unwind`、`rake unsafe:boundaries`）によって保持しているため、他で Nupur のコピーを使用する場合は独自チェックが必要です。

- **トークンは Ruby から呼び出される場所でのみ作成されること**：Ruby が GVL を保持した状態でコードを呼び出したという事実は呼び出し元の状態であり、型で表現することはできません。`gvl_token_outside_entry` は トランポリン や `run_init` を Ruby が呼び出す場所に限定し、`nupur_sys` のトークン作成処理（`Ruby::assume_gvl`）を `entry_token` に制限します。
- **Ruby の値は GC のスタックスキャンに見える場所に留まること**：値は `Copy` であり、スキャンから内容が見えない `Vec` や `Box` への格納を型の仕組みだけで防ぐことはできません。`ruby_value_on_heap` がバインディング、フィールド、static、式の型を読み取って検出します。
- **`nupur_sys` 自体のコードがこのバインディング層のルールを守ること**：`nupur_sys` は rb-sys を参照できる唯一のクレートであるため、その内部では raise、GVL、整数のルールは lint として管理されます（`raising_call_outside_protect`、`ffi_without_gvl_proof`、`c_integer_outside_int`）。どの C 関数が raise し得るかは、Ruby の C API の調査に基づくテーブルで管理されています。

拡張ライブラリ自体もいくつかのルールを守る必要があります。GVL を解放して実行されるボディは Ruby を実行しないこと、呼び出し元の Ruby を勝手に実行しないこと、そして Ractor セーフな拡張ライブラリの独自状態は Ractor 間で安全であることです。`src/lib.rs` にこれらと各チェック方法が一覧化されています。使用前に読んでください。

## 拡張ライブラリの記述方法

メソッドはRustの関数で、GVL のトークン（`&Ruby`）を受け取り、その値または `Result` を返すものです。`method!` / `function!` マクロがこれを トランポリン にラップします。`Init_` は `run_init` に 2 つの関数を渡します。1 つは実行中 Ruby のチェック（拒否された場合は `LoadError` になる）、もう 1 つは各種定義です。以下は `test/ext/src/lib.rs` にあるテスト拡張ライブラリの簡略版コピーです。

```rust
use nupur::rb::{prelude::*, Error, ExceptionClass, RbConst, Ruby, Value};
use nupur::function;

static INTERNAL: RbConst<ExceptionClass> = RbConst::new();

fn echo(_: &Ruby, v: Value) -> Value {
    v
}

fn check(ruby: &Ruby) -> Result<(), Error> {
    nupur::abi::check_running_api("nupur_test", "rake nupur:spec")?;
    nupur::abi::check_raise_state(ruby)
}

fn init(ruby: &Ruby) -> Result<(), Error> {
    let m = ruby.define_module("NupurTest")?;
    let internal = m.define_error("InternalError", ruby.exception_exception())?;
    INTERNAL.set(internal)?;
    nupur::rb::error::INTERNAL_ERROR.set(internal)?;
    m.define_module_function("echo", function!(echo, 1))?;
    Ok(())
}

#[no_mangle]
#[allow(non_snake_case)]
pub extern "C" fn Init_nupur_test() {
    // 安全性: Ruby は自身のスレッド上で GVL を保持したまま `Init_nupur_test` を呼び出します。
    unsafe { nupur::rb::method::run_init(check, init) };
}
```

テスト用拡張ライブラリでは、スロットを持つオブジェクトのラップ（`TypedType`, `Birth`）、引数とキーワードの読み取り（`args`）、Hash のトラバース、String の読み取り、GVL 解放状態での処理の実行などもカバーしています。

## Magnus との比較

Nupur は Makiri において Magnus を置き換えたものであり、多くの名称（`Value`、`RString`、`Error`、`method!`）を継承しています。ここでの比較対象は、2026 年 10 月の置き換えまで Makiri で使用されていた Magnus 0.8.2 です。

Magnus は Ruby の C API の広範囲をカバーする汎用バインディングです。これに対して Nupur は、Makiri が使用する C 関数と型のみをカバーします。Magnus を使う場合、Makiri が必要とする 30 ほどの機能のために、Magnus の内部構造とすべての `unsafe` を信用することになります。Nupur を使う場合は、信頼するのは Makiri が選定し契約を記述した C 関数のみとなります。

両者の安全性に関する違い：

| | Magnus | Nupur |
| --- | --- | --- |
| Raise し得る C 呼び出し | 多くの呼び出しが `protect` 下で動作。どれが raise するかは実装者の判断でありユーザーは検証不可 | それぞれが保護のトークンを受け取るラッパーであり、`rb_protect` の外では呼び出し不可。どの関数が raise するかは Ruby C API の調査に基づくテーブルで管理 |
| メソッド内でのパニック | `fatal` として raise される（Magnus #118）。発生場所での rescue は不可 | 拡張ライブラリの内部エラー（ホスト側が名前で rescue 可能な `Exception`）として raise される |
| 引数 | `to_str` / `to_int` で自動変換される（呼び出し側で再定義可能な Ruby コード） | 自動変換なし：メソッドは `Value` を受け取って型をチェック。`RString` や `i64` を直接受け取るメソッドはコンパイル不可 |
| ラップされたオブジェクト内の Ruby 値 | ユーザーがマーク関数を記述。`Vec` やフィールド内に値を置くことが可能（ヒープ用には `BoxValue` が存在） | 固定スロット。一度だけ書き込まれ Nupur がマーク。ペイロードは Ruby 値を保持せず、ヒープ上の保持は lint で拒否 |
| ラップされたオブジェクトデータの参照 | 借用した `&T` が、最適化によって破棄されたその `Value` より長生きし、解放済みメモリを指す恐れがあった（Magnus #131, #101） | 最後の参照が終わるまで GC からオブジェクトが見える状態を保つ `Typed` ハンドル経由で借用 |
| String のバイト列 | `RString::as_slice` で貸し出し可能 | コピーによる取り出しのみ（貸し出し不可） |
| GVL | `Ruby::get()` で実行時にチェック | Ruby の値、または値がない場合はトークンで保持を証明。トークンは Ruby からの呼び出し口でのみ作成され、どちらも `Send` ではないため GVL 解放処理には進入不可 |
| 生の `VALUE` | `rb-sys` 機能（`AsRawValue`, `FromRawValue`）で使用可能 | Nupur からはアクセス不可：`nupur_sys` は `VALUE` を再エクスポートしない |

Magnus のほうが記述は容易です。引数や返り値を自動変換し、ハンドルは `Copy` であり、Ruby API の大部分をサポートする公開クレートです。Nupur はこれらを意図的に放棄しています。エラーを引き起こしやすい書き方は「不可能であるか、最も冗長な書き方になる」ように設計されています。Nupur は公開されておらず、Makiri のワークスペース内でのみ使用されます。

## ディレクトリ／モジュール構造

| Nupur | 含まれる内容 |
| --- | --- |
| `rb` | `Value` とクラスチェック付きハンドル、`Error`、`method!` / `function!` トランポリン、クラス・モジュール定義 |
| `typed` | TypedData: Rust のペイロードを Ruby オブジェクトとしてラップ、そのスロットと GC コールバック、GC へ報告するメモリ |
| `gvl` | GVL を解放した状態での処理実行 |
| `build` | Array、Hash、数値、Symbol、UTF-8 String の構築 |
| `string`, `encoding`, `utf8_copy` | コピーによる String の読み書き、およびエンコーディング |
| `args`, `hash` | メソッドの引数・キーワード引数の読み取り、Hash のトラバース |
| `describe`, `msg` | 呼び出しやメモリ割り当てを行わずに例外メッセージへ Ruby 値を書き込む処理 |
| `abi` | バイナリが実行中 Ruby に対して前提とする内容と、ロード時のチェック |
| `caught`, `no_unwind` | C 境界でのパニックキャッチ、巻き戻り禁止ボディ |
| `stack`, `table`, `inject` | ネイティブスタックチェック、内部用整数キーテーブル、`rake oom` フック |

`raw`（保護された呼び出し）、`int`（`nupur_sys::int` の再エクスポート）、`cname` はクレートのプライベートモジュールです。

| `nupur_sys` | 含まれる内容 |
| --- | --- |
| `protect` | `rb_protect` の thunk、`Protected` トークン、`$!`、保留中の非ローカルエグジット |
| `raising` | Raise する可能性のある C 呼び出し（各呼び出しは `&Protected` を受け取る） |
| `non_raising` | Raise しない C 呼び出し（値またはトークンを受け取る。読み取りのほか、書き込みや GVL の解放を行うものもある） |
| `value` | `Value`、トークン `Ruby`、スロットの `Root` |
| `data_type` | TypedData 型の `rb_data_type_t` とそのフラグ（`DataFlags`） |
| `int` | C の整数型と Rust のビット幅間の変換 |
| `no_unwind` | 巻き戻り禁止ボディのガード（Nupur が再エクスポート） |

## 機能（Features）と依存関係

Nupur は `nupur_sys` にのみ依存し、`nupur_sys` は rb-sys にのみ依存します。唯一の feature である `alloc-inject` は `rake oom` フックをコンパイルします。拡張ライブラリ自体の `alloc-inject` がこれを有効化し、さらに `nupur_sys` の同機能を有効化します。拡張ライブラリは既に `rb_*` シンボルを提供する Ruby プロセス内にロードされるため、rb-sys は `link-ruby` なしで使用されます。

## テスト

```bash
cargo test -p nupur -p nupur_sys
bundle exec rake nupur:spec
```

`rake nupur:spec` はテスト用拡張ライブラリ（`test/ext`。Makiri のコードを含まず Nupur の上にいくつかのメソッドを持つのみ）をビルドし、その横にあるスペック（`test/spec`）を実行します。テスト内容には、メソッド内・GVL 解放下・Hash トラバース中のパニック、ブロックのエグジット、GC とコンパクション下でのスロット動作、検証付き String 読み取りなどが含まれます。CI（Linux / macOS）で実行されています。

`nupur_sys` の compile-fail doctest は、トークンや値が外部で作成できないことを実証します。各テストが指定するエラー内容を検証できるのは nightly の rustdoc のみであるため、`rake rust:compile_fail` で実行されます。
