# Nupur（ヌプㇽ）

Nupur は Makiri の拡張（extension）が使用している Ruby バインディングです。

拡張ライブラリに対して、Ruby の C APIを安全な API として提供します。これには Ruby の値とそのクラスチェック付きハンドル、例外、メソッド トランポリン、TypedData、GVL、および例外を発生する可能性のあるすべての C 呼び出しに対するラッパーが含まれます。
Ruby と通信するすべての `unsafe` コードは、たった 1 つを除いて Nupur またはその下のクレート内に存在します。その唯一の例外は、拡張ライブラリの `Init_` であり、定義を `run_init` に渡す役割を担っています。

レイヤー構造（上から順）：

```
makiri_rs   拡張ライブラリ：Makiri の Ruby メソッド
nupur       nupur_sys 上に構築された安全な API
nupur_sys   (sys/) Ruby の C API。各呼び出しは証明（proof）を受け取る。rb-sys に依存する唯一のクレート
rb-sys      Ruby の C API（生成されたバインディング）
```

Nupur は拡張ライブラリ自体に依存しておらず、拡張ライブラリについて何も知りません。Nupur が拡張ライブラリから必要とするもの（内部不変条件が破壊された場合の例外クラス、`rake oom` フックなど）は、拡張ライブラリが `Init_` 時に登録します。拡張ライブラリは Nupur に `crate::nupur` としてアクセスし、その `bridge` は `rb`、`typed`、`gvl`、`build`、`hash`、`args` を再エクスポートしています。

## 保証する事項

Nupur のルールの大部分は型とクレートの依存関係として定義されているため、ルールを破るコードはコンパイルされません。`src/lib.rs` に各詳細が記載されています。

- **Rust を跨ぐ raise の禁止**： Rubyの例外は Rust フレームを越える `longjmp` であり、発生させてはなりません。例外が発生する可能性のあるすべての C 呼び出しは、`&Protected` を受け取る `nupur_sys::raising` 内のラッパーとなっています。`rb_protect` の thunk のみがこれを作成し、`raw::call` が実行するクロージャに貸し出すため、例外が発生しても `Err` として戻ってきます。メソッドの `Err` は、そのメソッドのすべての Rust フレームが復帰した後に トランポリン によって raise されます。Nupur には C 関数を呼び出す他の方法はありません。Nupur は rb-sys に依存しておらず、`nupur_sys` も C 関数を再エクスポートしていません。
- **C に到達するパニックの禁止**：トランポリン は Rust のパニックを内部エラーに変換します。C が呼び出すコールバックは、呼び出し元が再開できるようにパニックをキャッチします（`caught::PanicLatch`）。巻き戻り（unwind）してはならないコードは `no_unwind` 箇所とし、`--cfg nupur_no_unwind` を指定したリリースビルドで巻き戻らないことを証明します（`rake no_unwind`）。
- **GVL の型化**：GVL を必要とする Ruby C の呼び出しは、GVL が保持されている証明を受け取ります。その証明とは、対象となる Ruby の値（Nupur の `Value` が保持する `nupur_sys::value::Value`）、または値が存在しない場合はトークン `Ruby` です。どちらも `Send` ではないため、GVL が解放された状態で実行されるボディに進入することはできません。値は `nupur_sys` の内部でのみ作成され、トークンは Ruby が呼び出すエントリのみが使用する `rb::method` のプライベートな `entry_token` によってのみ作成されます。
- **整数としての Ruby 値保持の禁止**：`nupur_sys` は `VALUE` を再エクスポートせず、値とワード間の変換を内部にとどめます。
- **ラップされたオブジェクトの Ruby 値はスロットである**：ペイロードは Ruby の値を保持しません。スロット（`nupur_sys::value::Root`）はオブジェクト作成時に一度だけ書き込まれ、Nupur のマークコールバックがそれらをマークします。ペイロードへは `typed::Typed` ハンドルを介してのみアクセスでき、データが参照されている間、オブジェクトを生かしたまま保持します。
- **String の読み取りはコピーで行う**：スライスを外部に渡すことはありません。テキストは std のバリデータによって UTF-8 であることが検証されたコピーであり、String のキャッシュされた coderange を信頼することはありません。
- **C 整数は 1 箇所で変換する**（`nupur_sys::int`）：C の `long` は Windows では 32 ビットであるため、変換処理は両方のビット幅に対応するよう 1 箇所に記述され、両方でテスト・証明されています（`verus/int.rs`）。
- **2 つの Ractor が同時に Nupur 上で実行可能**：拡張ライブラリが自身を Ractor セーフ（`rb::method::declare_ractor_safe`）であると宣言していれば、2 つの Ractor が同時に実行できます。Nupur のもので Ractor 間に共有されるのは、任意の Ractor が読み取り可能なものだけです。

### 型で保持できないこと

一部のルールは型として表現できません。Makiri ではこれらをコードのチェック（`lints/makiri_lints` にある `rake dylint` の lint、`rake no_unwind`、`rake unsafe:boundaries`）によって保持しているため、他で Nupur のコピーを使用する場合は独自チェックが必要です。

- **トークンは Ruby から呼び出される場所でのみ作成されること**：Ruby が GVL を保持した状態でコードを呼び出したという事実は呼び出し元の状態であり、型で表現することはできません。`gvl_token_outside_entry` は トランポリン や `run_init` を Ruby が呼び出す場所に限定し、`nupur_sys` のトークン作成処理（`Ruby::assume_gvl`）を `entry_token` に制限します。
- **Ruby の値は GC のスタックスキャンに見える場所に留まること**：値は `Copy` であり、スキャンから内容が見えない `Vec` や `Box` への格納を型の仕組みだけで防ぐことはできません。`ruby_value_on_heap` がバインディング、フィールド、static、式の型を読み取って検出します。
- **`nupur_sys` 自体のコードがこのバインディング層のルールを守ること**：`nupur_sys` は rb-sys を参照できる唯一のクレートであるため、その内部では raise、GVL、整数のルールは lint として管理されます（`raising_call_outside_protect`、`ffi_without_gvl_proof`、`c_integer_outside_int`）。どの C 関数が raise し得るかは、Ruby の C API の調査に基づくテーブルで管理されています。

拡張ライブラリ自体もいくつかのルールを守る必要があります。GVL を解放して実行されるボディは Ruby を実行しないこと、呼び出し元の Ruby を勝手に実行しないこと、そして Ractor セーフな拡張ライブラリの独自状態は Ractor 間で安全であることです。`src/lib.rs` にこれらと各チェック方法が一覧化されています。使用前に読んでください。

## 拡張ライブラリの記述方法

メソッドは、GVL のトークン（`&Ruby`）を受け取り、その値または `Result` を返す Rust 関数です。`method!` / `function!` マクロがこれを トランポリン にラップします。`Init_` は `run_init` に 2 つの関数を渡します。1 つは実行中 Ruby のチェック（拒否された場合は `LoadError` になる）、もう 1 つは各種定義です。以下は `test/ext/src/lib.rs` にあるテスト拡張ライブラリの簡略版コピーです。

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

Magnus は Ruby の C API の広範囲をカバーする汎用バインディングです。対して Nupur は、Makiri が使用する C 関数と型のみをカバーします。Magnus を使うことは、Makiri が必要とする 30 ほどの機能のために、Magnus の内部構造とすべての `unsafe` を信用することを意味していました。Nupur を使えば、信頼するのは Makiri が選定し契約を記述した C 関数のみとなります。

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
