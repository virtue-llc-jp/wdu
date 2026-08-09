# データモデル

共有モデルは `crates/wdu-core/src/lib.rs` に定義されています。イベント履歴を保存
するのではなく、ディレクトリごとの最新状態と観測開始からの累計を保存します。
JSON は開発用の表現であり、永続化には SQLite のスキーマを使用します。

## `FileChange`

ファイルシステムイベントを正規化した値です。

```json
{
  "path": "/Users/example/data/file.bin",
  "kind": "Modified",
  "observed_at_unix_secs": 1760000000
}
```

| フィールド | 型 | 意味 |
| --- | --- | --- |
| `path` | path | イベントに含まれるファイルまたはディレクトリ |
| `kind` | `FileChangeKind` | `Created`、`Modified`、`Removed`、`Other` |
| `observed_at_unix_secs` | `u64` | イベントを受信した Unix 秒 |

FSEvents の通知はまとめられることがあるため、この値だけから正確なファイル単位の
変更量を導出してはいけません。イベントは再計算のトリガーとしてだけ使います。

## `DirectoryUsageAggregate`

1 行が 1 ディレクトリに対応する、累計値を持つ集約です。使用量はそのディレクトリ
自身とすべての子孫の合計です。したがって、親ディレクトリの累計には子ディレクトリ
の累計がすでに含まれます。

```json
{
  "directory": "/Users/example/data",
  "current_usage_bytes": 12288,
  "cumulative_delta_bytes": 4096,
  "is_present": true,
  "observed_at_unix_secs": 1760000000
}
```

| フィールド | 型 | 意味 |
| --- | --- | --- |
| `directory` | path | 正規化した絶対ディレクトリパス |
| `current_usage_bytes` | `u64` | 最後に正常完了した再計算での配下全体の使用量 |
| `cumulative_delta_bytes` | `i64` | 初回観測からの増減累計。増加は正、減少は負 |
| `is_present` | `bool` | 最後の再計算時点でディレクトリが存在したか |
| `observed_at_unix_secs` | `u64` | 最後に値を確定した Unix 秒 |

初回の再帰スキャンでは `current_usage_bytes` を設定し、
`cumulative_delta_bytes` は 0 にします。削除されたディレクトリの行は消さず、使用量を
0、`is_present` を `false` として残します。これにより削除による減少も親の累計に
反映でき、同じパスが後で再作成された場合も再作成分を増加として記録できます。

## 更新アルゴリズム

ファイルイベントを受信したら、影響を受ける最小のディレクトリを再計算します。
前回値との差 `delta` は永続化せず、その場で次のように使います。

1. `new_usage - current_usage_bytes` で `delta` を求める。
2. 対象ディレクトリとすべての祖先について、`current_usage_bytes += delta` と
   `cumulative_delta_bytes += delta` を同一トランザクションで行う。
3. 対象ディレクトリの `is_present` と、更新時刻を更新する。

初回スキャン時に監視対象配下の全ディレクトリの行を作っておけば、指定階層の集計は
子孫行を `SUM` する必要がありません。例えば `/Users/example` を問い合わせる場合は、
そのパスの 1 行から `cumulative_delta_bytes` を読むだけです。子ディレクトリの行も
同じ値に含まれるため、親子の行をまとめて `SUM` してはいけません。

同じイベントが重複して届いても、最初の再計算後は `new_usage` と
`current_usage_bytes` が一致するため `delta` は 0 になります。FSEvents の集約や重複
通知に対しても、累計を二重に加算しません。

## SQLite スキーマ案

永続化層は `wdu-core` から分離し、`wdu-storage` crate が `rusqlite` を使う SQLite
adapter を提供します。パスは canonical path に正規化して保存します。

```sql
CREATE TABLE directory_usage (
    path TEXT PRIMARY KEY,
    parent_path TEXT REFERENCES directory_usage(path),
    current_usage_bytes INTEGER NOT NULL CHECK (current_usage_bytes >= 0),
    cumulative_delta_bytes INTEGER NOT NULL,
    is_present INTEGER NOT NULL CHECK (is_present IN (0, 1)),
    observed_at_unix_secs INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX directory_usage_parent_path_idx
    ON directory_usage(parent_path);
```

`path` の主キーで CLI の指定ディレクトリを直接検索し、`parent_path` を祖先更新の
ために使います。更新は 1 writer の SQLite トランザクションで行い、WAL と busy
timeout を有効にします。`current_usage_bytes` と累計値は SQLite の signed 64-bit
`INTEGER` に収まる使用量を前提にします。

祖先更新はアプリケーション側で親を辿っても構いませんが、SQLite では次のような
再帰 CTE で 1 回の更新にできます。

```sql
WITH RECURSIVE ancestors(path) AS (
    SELECT :directory
    UNION ALL
    SELECT directory.parent_path
    FROM directory_usage AS directory
    JOIN ancestors ON directory.path = ancestors.path
    WHERE directory.parent_path IS NOT NULL
)
UPDATE directory_usage
SET current_usage_bytes = current_usage_bytes + :delta_bytes,
    cumulative_delta_bytes = cumulative_delta_bytes + :delta_bytes,
    observed_at_unix_secs = :observed_at_unix_secs
WHERE path IN (SELECT path FROM ancestors);
```

`since` や `until` のような任意の時間範囲の集計は、この形式だけでは復元できません。
保存するのが最新状態と累計だけで、過去の各時点の値を捨てるためです。時間範囲クエリが
必要になった場合だけ、日次・時間単位のスナップショット表を追加します。通常の「監視
開始から現在まで」のクエリでは、現在の集約表だけを使う方が保存量とクエリコストを
小さくできます。
