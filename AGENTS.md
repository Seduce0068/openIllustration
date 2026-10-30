---
type: Playbook
title: "AGENTS.md for openIllustration"
description: "openIllustration 固有の制約とグローバル規約との差分。"
tags:
  - agents
  - openillustration
  - rust
generated:
  by: opencode
  at: "2026-10-31T00:27:37+09:00"
---

## プロジェクト概要

イラスト・小説サイトのプロトタイプ。バックエンドは Rust/Axum、
フロントエンドは Leptos CSR。指定マイナアプリ/JPKI 認証と JPYC 決済が
本番要件で、現行のモック認証は開発専用。

## グローバル設定との関係

- 共通規約はグローバル `~/.config/opencode/AGENTS.md` を継承する。
- 本ファイルは本プロジェクト固有の差分だけを記述する。グローバルと
  重複する規約は追加せず、差分がある場合にのみ上書きする。
- グローバル TODO は `~/.config/opencode/todo.md`。複数プロジェクト
  横断のタスクはそこに置き、本プロジェクト固有のタスクは本リポジトリの
  issue やドキュメントで管理する。
- 本ファイルとグローバルが矛盾する場合は、本ファイルの記述を優先する。

## プロジェクト固有の制約

- `ALLOW_MOCK_AUTH=1` は開発専用。本番環境では設定しない。
- `JWT_SECRET` は 32 バイト以上のランダム値を使う。未設定または短すぎる
  場合はサーバが起動しない。
- 秘密情報（`JWT_SECRET`、JPYC 鍵、認証情報）を commit しない。
- 模擬支援 API は 501 で無効。JPYC 送金検証の実装まで支援・有償サービスを
  提供しない。
- DB 内ハッシュ連鎖は第三者検証可能な監査ではない。監査要件として扱わない。

## 検証

ローカルでは fmt / clippy / test / wasm ビルドを実行する。E2E は CI の
`bash e2e/ci-e2e.sh` で実行する。

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build -p client --target wasm32-unknown-unknown --locked
```

CI は `onoderakeita/workflows/rust-gates.yml@v1` に委譲している。
