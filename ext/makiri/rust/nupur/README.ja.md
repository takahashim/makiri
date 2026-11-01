# Nupur

Nupur（ヌプㇽ）はMakiri の拡張機能の下にある Ruby バインディング層で、rb-sys のみに基づいて構築されています。
拡張機能が Ruby と通信するために必要とされる `unsafe` コードはすべてここに集約されます。そして、Ruby の値およびクラスチェック済みハンドル、例外、メソッドのトランポリン、TypedData、GVL、例外を発生させる可能性のあるすべての C 呼び出しのラッパーといった、拡張機能に対するAPIを安全なAPIとして提供します。

Nupur 自体は拡張機能に依存しておらず、拡張機能に関する情報を一切持ちません。
内部の不変条件が破壊された場合の例外クラスや `rake oom` のフックなど、拡張機能から必要とするものは `Init_` 時に拡張機能側から登録されます。
拡張機能からは `crate::nupur` としてアクセスされ、その `bridge` は `rb`、`typed`、`gvl`、`build`、`hash`、`args` を再エクスポートします。

## 保証する事項

`src/lib.rs` には、各保証の詳細とそれを検証するものが記載されています。要約は以下の通りです。

- **例外（raise）が Rust を跨がない:** 例外を発生させる可能性のあるすべての C 呼び出しは `rb_protect`（`raw`）の下で実行され、`Err` として返されます。メソッドの `Err` は、そのメソッドのすべての Rust フレームが返された後に、トランポリンによって発生させられます。
- **パニック（panic）が C に到達しない:** トランポリンはパニックを内部エラーへと変換します。C が呼び出すコールバックは、呼び出し元が再開できるようにパニックを捕捉します（`caught::PanicLatch`）。アンワインドしてはならないコードは `no_unwind` サイトであり、`--cfg nupur_no_unwind` を指定したリリースビルドによってアンワインド不可であることが証明されます（`rake no_unwind`）。
- **GVL はトークンである:** `Ruby` は Ruby から呼び出された場所（メソッドのトランポリンおよび `rb::method::run_init`）でのみ作成され、Ruby を呼び出すものはすべてそれ自身またはトークンが存在した場所にのみ存在できる Ruby の値を受け取ります。
- **ラップされたオブジェクトの Ruby の値はそのスロットである:** ペイロードは Ruby の値を保持しません。スロットはオブジェクトの作成時に一度だけ書き込まれ、Nupur のマークコールバックによってマークされます。ペイロードへは `typed::Typed` ハンドルを通じてのみアクセス可能であり、データが借用されている間、オブジェクトを生かし続けます。
- **String の読み取りはコピーによって行う:** スライスが渡されることはありません。テキストはコピーされ、std のバリデータによって UTF-8 であることがチェックされます。String のキャッシュされたコードレンジを信用することはありません。
- **C の整数変換を一箇所で行う** (`int`): Windows では C の `long` が 32 ビットであるため、どちらのビット幅でも機能するように変換が一箇所に記述されています。
- **拡張機能が Ractor 安全であると宣言すれば、2つの Ractor が同時にこの層を実行可能** (`rb::method::declare_ractor_safe`): 任意の Ractor が読み取れるもの以外、Nupur の要素はそれらの間で共有されません。

同ファイルには、型では強制できず、拡張機能側で維持しなければならないルール（Ruby の値は GC のスタックスキャンが見える場所に配置する、GVL を解放したボディでは Ruby を実行しない、要請なしに呼び出し元の Ruby を実行しないなど）も記載されています。本バインディング層を使用する前に必ず読んでください。

## Nupur を使った拡張機能の実装

メソッドは、GVL のトークン（`&Ruby`）を受け取り、その値または `Result` を返す Rust の関数です。`method!` / `function!` マクロによってトランポリン内にラップされます。
`Init_` は `run_init` に2つの関数を渡します。1つは実行中の Ruby のチェック（拒否された場合は `LoadError` となる）で、もう1つは定義です。以下は `test/ext/src/lib.rs` にあるテスト拡張機能を簡易化したコピーです。

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
    // SAFETY: Ruby は GVL を保持した状態で、そのスレッド上で `Init_nupur_test` を呼び出します。
    unsafe { nupur::rb::method::run_init(check, init) };
}
```

テスト拡張機能では、スロットを持つオブジェクトのラップ（`TypedType`, `Birth`）、引数とキーワードの読み取り（`args`）、Hash の操作、String の読み取り、および GVL を解放した状態での処理の実行も行っています。

## Magnus との比較

Nupur は Makiri のために Magnus を置き換えたものであり、その名称の多く（`Value`、`RString`、`Error`、`method!`）を継承しています。以下の比較は、2026年10月の置き換えまで Makiri で使用されていたバージョンである Magnus 0.8.2 との比較です。

Magnus は Ruby の C API の大部分をカバーする汎用的なバインディングです。一方 Nupur は、Makiri が使用する C 関数と型のみをカバーしています（コメントを含めて Magnus が約 69,000 行であるのに対し、Nupur は約 7,300 行）。Magnus を使用するということは、Makiri が必要とする30個ほどの機能のために、そのすべての内部構造と `unsafe` を信用することを意味していました。Nupur では、Makiri が選択し契約を記述した C 関数のみが信頼の対象となります。

安全性における両者の違い:

| | Magnus | Nupur |
| --- | --- | --- |
| 例外を発生させ得る C 呼び出し | 多くの呼び出しが `protect` の下で実行される。どの呼び出しが例外を発生させ得るかは実装者の判断によるものであり、ユーザーが検証することはできない | すべての呼び出しが `rb_protect` 下の `raw` ラッパーである。どの関数が例外を発生させ得るかは Ruby の C API の調査に基づくテーブルで管理され、未リストの呼び出しはリントによって拒否される |
| メソッド内でのパニック | `fatal` として発生させられ（Magnus #118）、発生した場所で救済（rescue）することができない | 拡張機能の内部エラー（ホストが名前で救済できる `Exception`）として発生させられる |
| 引数 | 呼び出し元が再定義可能な Ruby コードである `to_str` / `to_int` によって変換される | 変換されない。メソッドは `Value` を受け取って型をチェックする。`RString` や `i64` を受け取るメソッドはコンパイルされない |
| ラップされたオブジェクト内の Ruby の値 | ユーザーがマーク関数を記述する。値は `Vec` やフィールド内に存在できる（ヒープ用には `BoxValue` が存在） | 固定スロットであり、一度だけ書き込まれ Nupur によってマークされる。ペイロードは Ruby の値を保持せず、ヒープ上の値はリントによって拒否される |
| ラップされたオブジェクトのデータの借用 | 借用された `&T` がその `Value`（最適化によって破棄されたもの）より長く生存し、解放されたメモリを指す可能性があった（Magnus #131, #101） | 最後の借用が終わるまでオブジェクトを GC から見える状態に保つ `Typed` ハンドルを通じて借用される |
| String のバイト列 | `RString::as_slice` によって貸し出される | コピー出力のみが行われ、貸し出されることはない |
| GVL のトークン | 実行時にチェックされる `Ruby::get()` | トランポリンと `Init_` によってのみ作成される。`Send` ではないため、GVL が解放されたボディに入ることはできない |

Magnus はコードが書きやすいという特徴があります。引数や戻り値を自動で変換し、ハンドルは `Copy` であり、より多くの Ruby API を備え、公開された crate となっています。Nupur はこれらを意図的に放棄しています。エラーを引き起こしやすい書き方は、不可能な仕組みになっているか、あるいはもっと回りくどい書き方になるよう設計されています。また、安全性の一部は型システムではなく Makiri のリポジトリにあるチェック機構（`rake dylint`、`rake no_unwind`、`rake unsafe:boundaries`）に依存しているため、Nupur を他所でコピーして使用する場合も同様のチェックが必要となります。Nupur は未公開であり、Makiri のワークスペース内でのみ使用されます。

## モジュール構造

| モジュール | 保持する内容 |
| --- | --- |
| `rb` | `Value` およびクラスチェック済みハンドル、`Error`、`method!` / `function!` トランポリン、クラスおよびモジュールの定義 |
| `typed` | TypedData: Rust のペイロードを Ruby オブジェクトとしてラップ、そのスロットと GC コールバック、GC に報告されるメモリ |
| `gvl` | GVL を解放した状態での処理の実行 |
| `build` | Array、Hash、数値、Symbol、および UTF-8 String の構築 |
| `string`, `encoding`, `utf8_copy` | コピーによる String の読み書き、およびそのエンコーディング |
| `args`, `hash` | メソッドの引数やキーワードの読み取り、Hash のトラバース |
| `describe`, `msg` | 呼び出しや割り当てを行わずに例外メッセージへ Ruby の値を書き込む |
| `abi` | バイナリが実行中の Ruby に対して前提としている事項と、ロード時のチェック |
| `caught`, `no_unwind` | C の境界でのパニックの捕捉、アンワインドしてはならない処理のボディ |
| `stack`, `table`, `inject` | ネイティブスタックのチェック、プライベートな整数キーテーブル、`rake oom` フック |

`raw`（例外を発生させ得る C 呼び出し）、`int`、`cname` は crate 内のプライベートモジュールです。

## 機能（Features）と依存関係

唯一の機能である `alloc-inject` は `rake oom` フックをコンパイルします。拡張機能自体の `alloc-inject` がこれを有効にします。拡張機能はすでに `rb_*` シンボルを提供している Ruby にロードされるため、rb-sys は `link-ruby` なしで使用されます。

## テスト

```bash
cargo test -p nupur
bundle exec rake nupur:spec
```

`rake nupur:spec` はテスト拡張機能（`test/ext`。本バインディング層上のいくつかのメソッドで構成され、Makiri 固有のコードは含まれません）をビルドし、そのスペック（`test/spec`）を実行します。スペックでは、メソッド内、GVL 解放下、および Hash トラバース中でのパニック、ブロックの抜け出し、GC とコンパクション下でのスロット、検証付き String 読み取りなどをテストします。
CI はこれを Linux および macOS 上で実行します。

Makiri の `rake dylint`（`lints/makiri_lints`）、`rake no_unwind`、および `rake unsafe:boundaries` は、本層のコード自体が自身のルールに従っているかをチェックします。この層を他の場所にコピーして使用する場合は、独自のチェック機構が必要となります（`src/lib.rs` に一覧が記載されています）。
