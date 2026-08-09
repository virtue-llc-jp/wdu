# AGENTS.md

## プロジェクト概要

`wdu` は macOS に常駐し、ファイルシステムの更新イベントを起点にディレクトリ
単位のディスク使用量の増減を記録し、CLI からクエリする Rust ツールです。
macOS に特化してよく、他の OS への互換性を目的に設計しません。

## リポジトリ構成

- `crates/wdu-core`: 監視イベントと使用量差分の共有データモデル
- `crates/wdu-daemon`: `notify`/FSEvents を使う常駐監視プロセス
- `crates/wdu-cli`: `wdu` バイナリ。記録済みデータをクエリする入口
- `docs/`: アーキテクチャ、データモデル、開発手順

ルートの `Cargo.toml` が workspace の依存関係と共通 package metadata を管理します。
crate 固有の依存関係は各 crate の manifest に追加し、workspace dependency を優先して
バージョンを一元管理してください。

## 実装方針

- `wdu-core` は OS 固有 API に依存させない。
- FSEvents のイベントは使用量そのものではなく、再計算のトリガーとして扱う。
- ファイルイベントの受信、使用量の再計算、永続化、クエリを別の責務として保つ。
- 作業ツリーにあるユーザーの変更を上書きしない。無関係な既存変更を巻き戻さない。
- エラーを黙って無視せず、既存のエラー処理方針に沿って呼び出し元へ伝える。
- 仕様やデータ形式を変更した場合は、関連する `README.md` または `docs/` も更新する。

FSEvents はイベントをまとめたり、同一パスに対して複数回通知したりする可能性が
あります。将来の再計算・記録処理では、イベントを直接差分として解釈せず、対象
ディレクトリの状態を読み直す設計にしてください。

## 開発・検証

変更後は、変更範囲に応じて次のコマンドを実行します。

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
```

Lint が必要な変更では、既存の Rust toolchain に含まれる Clippy も実行します。

```sh
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

`wdu-daemon` の実際の監視動作は macOS 上で確認してください。Linux などでは
監視機能を起動せず、macOS 専用であることを示すエラーを返す方針です。

## コードスタイル

- edition 2024 と rustfmt の標準設定に従う。
- コメントはコードから読み取りにくい設計上の理由に限る。
- 不要な型キャストや広すぎる `catch` 相当のエラー処理を追加しない。
- 新しい抽象化を追加する前に、既存の共有モデルや helper を再利用できないか確認する。
