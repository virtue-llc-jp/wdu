# 開発手順

## 前提

- macOS
- Rust stable
- edition 2024 を扱える toolchain

toolchain のバージョンはリポジトリ内で固定していません。作業環境の stable
toolchain を最新に保ってください。

## 検証

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
```

必要に応じて Clippy を実行します。

```sh
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

依存関係を更新した場合は `Cargo.lock` も変更対象に含めます。

## 実行

監視対象を指定して daemon を起動します。プロセスは停止されるまで動作します。

```sh
cargo run -p wdu-daemon -- /tmp/wdu-watch
```

起動時に監視対象を再帰スキャンして SQLite を初期化し、イベント受信後は影響を受ける
ディレクトリを再計算して永続化します。受信イベントは開発用に標準出力へ 1 行 1 JSON
で出力されます。

設定ファイルを指定する場合は次のようにします。`--config` を省略すると、
`WDU_CONFIG`、Homebrew の `etc/wdu/config.toml`、ユーザー設定の順に既存ファイルを
探します。

```sh
cargo run -p wdu-daemon -- \
  --config /opt/homebrew/etc/wdu/config.toml
```

CLI の引数は次の形です。

```sh
cargo run -p wdu-cli -- query \
  --directory /tmp/wdu-watch \
  --since 1760000000
```

指定ディレクトリの配下全体について、観測開始からの増減累計を JSON で返します。
`--since` を指定すると hourly/daily bucket の時間範囲差分を返します。daemon と別の
DB を使う場合は `--database` で同じパスを指定します。

daemon を起動せず現在容量を記録するには次を実行します。

```sh
cargo run -p wdu-cli -- record \
  --directory /tmp/wdu-watch \
  --database /tmp/wdu.sqlite3
```

## 実装時の注意

- macOS 固有の依存関係は `wdu-daemon` の target-specific dependency に置く。
- `wdu-core` に OS 固有の型を漏らさない。
- イベントの重複・集約を前提にし、イベントをそのままバイト差分として記録しない。
- ディレクトリの現在使用量と増減累計を親ディレクトリにも反映し、指定階層の集計で
  子孫行を重複して `SUM` しない。
- 削除されたパスの `metadata` 取得失敗は通常のケースになり得るため、再計算対象の
  決定とエラーの扱いを分ける。
- シンボリックリンクは辿らず、論理サイズを通常ファイルごとに合計する。
- hourly bucket の更新と aggregate の更新を同じ SQLite transaction で行う。
- 7 日より古い hourly bucket は日次 bucket へ圧縮する。

Homebrew 向けの release artifact は次で作成します。

```sh
bash scripts/package-release.sh
```

生成物の配置と formula 側の install 方針は [`homebrew.md`](homebrew.md) を参照して
ください。
