# 今後の課題

ここでは、現在の実装とは分離して、Homebrew で常用するためと、時間を指定した増分
クエリを提供するための次の課題を記録します。

## 1. Homebrew service と runtime data

Homebrew の formula に `service do` を定義し、インストールされた daemon を
`brew services start wdu` で起動できるようにします。`brew install` の中で無断で
daemon を起動するのではなく、Homebrew service の標準的な明示起動を使います。
ログイン時・再起動後も起動する設定は `brew services start` が生成する launchd job に
任せます。

ファイル配置は次の役割に分けます。

| 種類 | Homebrew での配置 | 内容 |
| --- | --- | --- |
| 実行ファイル | `bin/` | `wdu`、`wdu-daemon` |
| 設定 | `$(brew --prefix)/etc/wdu/config.toml` | daemon と CLI が共有する設定 |
| SQLite DB | `$(brew --prefix)/var/wdu/wdu.sqlite3` | 可変な runtime data |
| ログ | `$(brew --prefix)/var/log/wdu/` | daemon の stdout/stderr |
| ドキュメント | `share/doc/wdu/` | README と設計文書 |

Cellar は immutable な install 領域として扱い、DB・ログ・設定を書き込みません。
formula の `post_install` では `var/wdu` と `var/log/wdu` のディレクトリだけを作り、
DB ファイル自体は daemon の初回起動時に作成します。

設定ファイルが存在しない初回でも動くようにし、`etc/wdu/config.toml` は formula の
install 時に必須生成しません。ユーザーが設定を追加・編集できるよう、必要なら
`etc/wdu/config.toml.example` を `share/doc/wdu/` に同梱します。

## 2. CLI と daemon の設定共有

CLI と daemon が同じ SQLite を参照できるよう、設定読み込みを共通 crate または
共通 module に切り出します。設定の候補は次のようにします。

```toml
database = "/opt/homebrew/var/wdu/wdu.sqlite3"
watch_root = "/Users/example/data"
hourly_bucket_secs = 3600
hourly_retention_secs = 604800
```

設定の優先順位は次の順にします。

1. コマンドライン引数（`--config`、`--database` など）
2. `WDU_CONFIG` / `WDU_DATABASE` などの環境変数
3. `$(brew --prefix)/etc/wdu/config.toml`
4. macOS のユーザー向けデフォルト値

`--database` と `WDU_DATABASE` は既存利用者のために後方互換で残します。config の
相対パスは config ファイルの親ディレクトリを基準に解決し、CLI と daemon で異なる
DB を誤って開かないよう、起動時に解決済みパスを表示します。

## 3. CLI による現在容量の記録

daemon を動かさず、CLI から現在の使用量を測定して SQLite に書き込めるようにします。
候補コマンドは次の形です。

```sh
wdu record --directory /Users/example/data
```

このコマンドは daemon と同じ scanner・storage 更新処理を使います。再帰スキャンした
現在容量と観測時刻を記録し、前回値との差を対象ディレクトリと全祖先へ反映します。
初回は baseline として扱い、既存 DB の watch root と異なるディレクトリへの記録は
拒否します。

測定結果は JSON で表示できるようにし、`--database` と `--config` も query と同じ
規則で受け付けます。CLI と daemon で測定結果がずれないよう、scanner を binary
crate から共有 library crate へ移すことを先に行います。

## 4. 時系列の増分記録

現在の `directory_usage` は観測開始からの累計だけを保持するため、任意の時点以降の
増分を知るには時間バケット表を追加します。イベントを 1 件ずつ保存するのではなく、
変更が確定した差分を時間単位で集約します。

候補スキーマは次の形です。

```sql
CREATE TABLE directory_usage_bucket (
    path TEXT NOT NULL,
    bucket_start_unix_secs INTEGER NOT NULL,
    granularity_secs INTEGER NOT NULL,
    delta_bytes INTEGER NOT NULL,
    PRIMARY KEY (path, bucket_start_unix_secs, granularity_secs)
) WITHOUT ROWID;

CREATE INDEX directory_usage_bucket_query_idx
    ON directory_usage_bucket(path, granularity_secs, bucket_start_unix_secs);
```

通常の変更は `bucket_start_unix_secs = floor(observed_at / 3600) * 3600` に丸め、
対象ディレクトリと祖先の各行へ同じ差分を加算します。`wdu query` には将来、次の
ような範囲指定を追加します。

```sh
wdu query --directory /Users/example/data --since 1760000000
```

範囲の境界が時間バケットの途中にある場合、現在の時間バケット方式ではその 1 時間
内の正確な境界までは復元できません。仕様として「時間単位の近似」と明示するか、
境界だけ event-level の短期記録を併用するかを実装時に決めます。

時系列行の追加と現在の materialized aggregate 更新は同一 SQLite transaction で行い、
集計だけ成功して履歴が欠ける状態を作らないようにします。重複イベントは現在値との
比較で差分 0 になるため、同じ時間バケットへ二重加算しません。

## 5. 古い時系列の圧縮

daemon の定期処理で、現在から 1 週間以上前の hourly bucket を圧縮します。最初の
圧縮方式は、日付単位の増分を維持するため次の形にします。

- 直近 7 日間: 1 時間粒度を保持
- 7 日より前: 1 日粒度へ統合
- 統合対象: 同じ `path` と日付に属する `delta_bytes` の合計
- 実行単位: SQLite transaction

日次化によって、古い期間の `since` クエリは日単位の精度で継続できます。古い行を
単一の総計へ潰すと「いつからの増分か」を失うため、その方式は採用しません。

圧縮処理は daemon の通常イベント処理と分離し、起動時または一定間隔で実行します。
同じ圧縮を再実行しても結果が変わらないよう、`granularity_secs` を含む一意制約と
移動先への upsert 後の削除を同一 transaction で行います。圧縮中も query が一貫した
結果を読めるよう、SQLite の transaction 境界を利用します。

## 実装順序

1. config loader を追加し、CLI と daemon の DB path 解決を共通化する。
2. scanner を共有 library に移し、`wdu record` を追加する。
3. hourly bucket と `--since` query を追加する。
4. daemon の日次圧縮処理と retention 設定を追加する。
5. Homebrew formula の service、`etc`/`var` 配置、launchd 起動を実機で検証する。