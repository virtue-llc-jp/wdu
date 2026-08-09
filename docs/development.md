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

CLI の引数は次の形です。

```sh
cargo run -p wdu-cli -- query \
  --directory /tmp/wdu-watch
```

指定ディレクトリの配下全体について、観測開始からの増減累計を JSON で返します。
daemon と別の DB を使う場合は `--database` で同じパスを指定します。

## 実装時の注意

- macOS 固有の依存関係は `wdu-daemon` の target-specific dependency に置く。
- `wdu-core` に OS 固有の型を漏らさない。
- イベントの重複・集約を前提にし、イベントをそのままバイト差分として記録しない。
- ディレクトリの現在使用量と増減累計を親ディレクトリにも反映し、指定階層の集計で
  子孫行を重複して `SUM` しない。
- 削除されたパスの `metadata` 取得失敗は通常のケースになり得るため、再計算対象の
  決定とエラーの扱いを分ける。
- シンボリックリンクを辿る場合は循環と二重計上を防ぐ。

Homebrew 向けの release artifact は次で作成します。

```sh
bash scripts/package-release.sh
```

生成物の配置と formula 側の install 方針は [`homebrew.md`](homebrew.md) を参照して
ください。
