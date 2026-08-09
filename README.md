# wdu

`wdu` は macOS のファイルシステム更新イベントを常駐監視し、ディレクトリ単位の
ディスク使用量の増減累計を記録・検索するツールです。

## 現在の状態

Rust workspace の初期実装です。現在は以下まで実装されています。

- macOS の再帰的なファイルシステムイベント監視
- 監視イベントを共有ドメインモデルへ変換
- ディレクトリ配下全体の現在使用量と増減累計を表す共有データモデル
- SQLite への階層累計の保存と、重複通知・削除の処理
- hourly/daily bucket の時系列差分保存と古い履歴の圧縮
- 指定ディレクトリの累計・時間範囲を JSON で返す CLI クエリ
- CLI から現在容量を測定して記録する `record` サブコマンド

監視イベントは開発用に NDJSON として標準出力へ出力されます。永続化先は既定では
macOS の `~/Library/Application Support/wdu/wdu.sqlite3` で、`--database` または
`WDU_DATABASE` で変更できます。`--config` または `WDU_CONFIG` で TOML 設定を指定
でき、Homebrew では `$(brew --prefix)/etc/wdu/config.toml` が候補になります。

## Workspace 構成

| crate | 役割 |
| --- | --- |
| `wdu-core` | `FileChange` と `DirectoryUsageAggregate` などの共有データモデル |
| `wdu-daemon` | macOS のイベントを再帰的に監視する常駐プロセス |
| `wdu-storage` | SQLite の階層集約ストレージ |
| `wdu-config` | CLI と daemon が共有する TOML 設定 |
| `wdu-usage` | CLI と daemon が共有するファイルシステム scanner |
| `wdu-cli` | 記録済みデータを検索する CLI。バイナリ名は `wdu` |

監視バックエンドは `notify` の macOS 実装（FSEvents）を使用します。OS 固有の
処理は `wdu-daemon` に閉じ込め、`wdu-core` はプラットフォーム非依存に保ちます。

## 必要環境

- macOS
- Rust stable（edition 2024）

## 開発コマンド

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
```

監視を起動するには次を実行します。

```sh
cargo run -p wdu-daemon -- /path/to/watch
```

データベースの場所を指定する場合は次のように起動します。

```sh
cargo run -p wdu-daemon -- /path/to/watch --database /path/to/wdu.sqlite3
```

SQLite DB は監視対象ディレクトリの外側に置いてください。daemon は自己監視と DB の
更新イベントによる再帰処理を防ぐため、監視対象配下の DB を拒否します。

```sh
cargo run -p wdu-cli -- query --directory /path/to/watch
```

時間範囲の差分を確認するには次を実行します。

```sh
cargo run -p wdu-cli -- query \
	--directory /path/to/watch \
	--since 1760000000
```

現在容量を手動で測定して記録するには次を実行します。

```sh
cargo run -p wdu-cli -- record --directory /path/to/watch
```

クエリは指定ディレクトリの集約行を読み取り、観測開始から現在までの配下全体の
増減累計を返します。`--since` を指定すると hourly/daily bucket の時間範囲差分を
返します。event 単位では保存せず、既定では 7 日より古い hourly bucket を日次へ
圧縮します。

Homebrew 向けの release artifact と、formula でのファイル配置は
[`docs/homebrew.md`](docs/homebrew.md) に記載しています。

今後の課題と実装順序は [`docs/roadmap.md`](docs/roadmap.md) に記録しています。

設計と開発手順は [`docs/`](docs/) を参照してください。エージェント向けの作業
ルールは [`AGENTS.md`](AGENTS.md) にまとめています。
