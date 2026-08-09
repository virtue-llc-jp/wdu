# Homebrew 配布

Homebrew formula での配布を前提に、実行ファイルと実行時データを分離します。

## Release artifact

macOS の host target 向け artifact は次で作成します。

```sh
bash scripts/package-release.sh
```

target triple を明示する場合は次のようにします。

```sh
bash scripts/package-release.sh aarch64-apple-darwin
bash scripts/package-release.sh x86_64-apple-darwin
```

スクリプトは `cargo build --release --locked` を実行し、`dist/` に次のファイルを
作成します。

```text
dist/wdu-<version>-<target>.tar.gz
dist/wdu-<version>-<target>.tar.gz.sha256
```

archive 内の配置は Homebrew formula からそのまま install できる形にします。

```text
wdu-<version>-<target>/
├── bin/
│   ├── wdu
│   └── wdu-daemon
└── share/doc/wdu/
    ├── README.md
    ├── architecture.md
    └── data-model.md
```

formula は tap 側の `Formula/w/wdu.rb` に置き、`install` は次の対応にします。

```ruby
bin.install "bin/wdu"
bin.install "bin/wdu-daemon"
doc.install Dir["share/doc/wdu/*"]
```

`dist/` はリポジトリへ commit せず、GitHub Release などの配布先へ upload します。
formula の `url` と `sha256` には target ごとの artifact を指定します。

## Runtime file placement

Homebrew の Cellar や `opt` は read-only の install 領域として扱い、SQLite DB や
ログをそこへ作成しません。

- 通常の macOS ユーザー実行: `~/Library/Application Support/wdu/wdu.sqlite3`
- Homebrew service: formula の `var/` 配下、例えば `var/wdu/wdu.sqlite3`
- 一時的な切り替え: daemon と CLI の `--database PATH`、または `WDU_DATABASE`

service formula では `wdu-daemon --database var/wdu/wdu.sqlite3` のように DB パスを
明示します。これにより Cellar 内の artifact は immutable のまま、service の状態は
Homebrew が管理する `var/` に置けます。現時点では設定ファイルを持たないため、
`etc/` への配置は行いません。