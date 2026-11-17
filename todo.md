---
type: Todo
title: "openIllustration プロジェクトTODO"
description: "openIllustration 固有のタスクを管理するプロジェクト内 TODO。複数プロジェクト横断のタスクはグローバルTODOへ置く。"
tags:
  - todo
  - openillustration
  - tasks
generated:
  by: dsh (lead)
  at: "2026-11-17T22:26:20+09:00"
status: current
---

## 進行中

- [x] プロジェクト全体の6パス・クロスレビュー（統合完了:
  [UNKNOWN_TASKS_INVESTIGATION.md](docs/UNKNOWN_TASKS_INVESTIGATION.md) §6.3）

## 未着手

### 優先度: 高

P0（本番公開前に解消すべきセキュリティ／データ完全性）:

- [ ] 年齢ゲートを OIDC callback / refresh へ配線し、信頼属性必須でフェイルクローズ（R1-17）
- [x] 通報中・却下コンテンツの直接 CID 取得を 404 化（R2-09、2026-11-17 対応済み）
- [x] refresh Cookie の `Path` を `/auth` へ広げ、logout 時のファミリ失効を有効化（R1-18、2026-11-17 対応済み）
- [x] ウォレット bind を DB に記録し verify の必須条件にする（R1-24、2026-11-17 対応済み）
- [x] 本番起動時に OIDC 必須設定と `PINATA_JWT` を検査し fail-closed 化（R4-01 / R4-04、2026-11-17 対応済み）
- [x] アップロード UI の file パートへ MIME を設定（R1-09、2026-11-17 対応済み）
- [x] Tailwind をビルド時 CSS 化し、外部 CDN/フォント依存と無 SRI を解消（R1-10、2026-11-17 対応済み）

### 優先度: 中

P1（本番運用の健全性）:

- [ ] 認証・アップロード・注文作成へレート制限と同時実行上限を導入（R4-02）
- [ ] OIDC HTTP タイムアウト設定キーを追加（R4-03）
- [ ] `/auth/login` を `ALLOW_MOCK_AUTH` でゲートし設計書と実装を整合（C2 / R1-21）
- [ ] `verify_order` へ CAS とトランザクションを導入し確定後不変を担保（R1-25）
- [ ] メトリクスカウンタを差分アサーション＋注入可能設計へ変更（R3-01）
- [x] 主要画面に実サーバー認証を使うスモーク E2E を1本追加（C1、2026-11-17 対応済み）

### 優先度: 低

P2（品質・保守・プロダクト拡張）:

- [ ] 現行状態を謳う設計・状態ドキュメントを更新（C4 / R2-17〜R2-21）
- [ ] セキュリティ勧告の受容根拠を RS256 本番署名に合わせて再評価（C6）
- [ ] firefox / webkit プロジェクトを追加し Chrome 固定を解消（R3-09）
- [ ] 設定検証 CLI（`doctor`）を追加（R5-06）
- [ ] 機能提案（R5-01 / R5-02 / R5-03 / R5-09 / R5-10）。法務・決済依存の
  R5-04 / R5-05 / R5-08 は一次ソース確認と設計が先

## 完了

- [x] todo をプロジェクト内（`todo.md`）へ配置し、グローバル AGENTS に運用を明記（2026-11-17）
- [x] 6パス・クロスレビューの統合結果を P0/P1/P2 として本ファイルへ転記（2026-11-17）
- [x] R2-09: `get_content` を `status = 'Approved'` で絞り、Reported/Rejected を 404 化（2026-11-17）

## メモ

- 詳細な残タスク棚卸しは `docs/TASK_INVENTORY.md`、レビュー履歴は `docs/REVIEW_REPORT.md`、
  未知タスク調査は `docs/UNKNOWN_TASKS_INVESTIGATION.md` を参照。
- 本ファイルにはクロスレビューで統合・採用した項目を追記する。
- P0 の R1-17 / R1-18 / R2-09 はコード上確定（Runtime Proof 済み）。
  他の項目はレポートの静的推論を含むため、着手時に事実確認する。
