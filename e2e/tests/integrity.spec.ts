import { test, expect } from '@playwright/test';
import * as crypto from 'crypto';

test.describe('Forensic Integrity Verification (INT-01)', () => {
  const MOCK_CID = 'QmTFfE7GUfHHzrJDcUQycwRqHhxMCJ2Z1nJ8cqu2uoerhp';
  const ORIGINAL_CONTENT = 'This is a genuine forensic evidence content.';
  const TAMPERED_CONTENT = 'This is a TAMPERED forensic evidence content.';
  
  // SHA-256ハッシュの生成ヘルパー
  const computeHash = (data: string) => {
    return crypto.createHash('sha256').update(data).digest('hex');
  };

  test('should verify genuine content successfully', async ({ page }) => {
    const hash = computeHash(ORIGINAL_CONTENT);

    // 1. コンテンツインデックスのモック (ログイン前に登録)
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: MOCK_CID,
          checksum: hash,
          type: 'Novel',
          status: 'Approved',
          price: 0,
          // タイトル空の fixture: illustration_aria_label の cid 先頭8文字フォールバックを検証する。
          title: '',
          description: 'Integrity verification fixture',
          tags: ['forensic', 'test'],
          is_favorite: false
        }])
      });
    });

    // 2. コンテンツ実体のモック
    await page.route(`**/content/${MOCK_CID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        headers: {
          'x-forensic-checksum': hash,
          'content-type': 'text/markdown'
        },
        body: ORIGINAL_CONTENT
      });
    });

    await page.route(`**/content/${MOCK_CID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });

    // 認証のモック
    await page.route('**/auth/login', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ nonce: 'test-nonce' }) });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: 'mock_user_hash', token: 'mock_jwt_token' })
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    
    // 検証は非同期で即完了し得るため、中間状態の PENDING は待たない。
    // ダッシュボードで最終的な「VERIFIED」が表示されることを確認する。
    const verifiedBadge = page.locator('span', { hasText: '● VERIFIED' });
    await expect(verifiedBadge).toBeVisible({ timeout: 30000 });

    // タイトル空のとき canvas の aria-label は cid 先頭8文字のフォールバックになる
    // （illustration_aria_label）。小説カードでも canvas は role="img" で描画される。
    await expect(page.locator('canvas[role="img"]')).toHaveAttribute(
      'aria-label',
      `無題のイラスト (${MOCK_CID.slice(0, 8)})`,
    );
  });

  test('should detect and block tampered content', async ({ page }) => {
    const originalHash = computeHash(ORIGINAL_CONTENT);

    // 1. コンテンツインデックスのモック (ログイン前に登録)
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: MOCK_CID,
          checksum: originalHash,
          type: 'Novel',
          status: 'Approved',
          price: 0,
          title: 'Forensic Evidence Log',
          description: 'Integrity verification fixture',
          tags: ['forensic', 'test'],
          is_favorite: false
        }])
      });
    });

    // 2. コンテンツ実体のモック
    await page.route(`**/content/${MOCK_CID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        headers: {
          'x-forensic-checksum': originalHash,
          'content-type': 'text/markdown'
        },
        body: TAMPERED_CONTENT
      });
    });

    await page.route(`**/content/${MOCK_CID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });

    // 認証のモック
    await page.route('**/auth/login', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ nonce: 'test-nonce' }) });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: 'mock_user_hash', token: 'mock_jwt_token' })
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    
    // 検証は非同期で即完了し得るため、中間状態の PENDING は待たない。
    // 「COMPROMISED」が表示されることを確認
    const compromisedBadge = page.locator('span', { hasText: '× COMPROMISED' });
    await expect(compromisedBadge).toBeVisible({ timeout: 30000 });
    
    // 詳細メッセージの確認
    await expect(page.locator('text=Integrity Breach Detected')).toBeVisible();
    await expect(page.locator('text=Wasm checksum does not match forensic registry.')).toBeVisible();
  });

  test('should not mark VERIFIED when checksum matches but JXL decoding fails (R1-13)', async ({ page }) => {
    // チェックサムは一致するが JXL コーデックストリームとして不正なバイト列を返すモックゲートウェイ。
    // HTTP 200・チェックサム一致でも、デコードに失敗した場合は VERIFIED にしないことを検証する。
    const invalidJxl = 'not a jxl codestream';
    const hash = computeHash(invalidJxl);
    const gatewayStatuses: number[] = [];

    page.on('response', response => {
      if (response.url().endsWith(`/content/${MOCK_CID}`)) {
        gatewayStatuses.push(response.status());
      }
    });

    // 1. コンテンツインデックスのモック（ログイン前に登録）
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: MOCK_CID,
          checksum: hash,
          type: 'Illustration',
          status: 'Approved',
          price: 0,
          title: 'Invalid JXL Evidence',
          description: 'Checksum matches but the codestream is invalid',
          tags: ['forensic', 'jxl'],
          is_favorite: false
        }])
      });
    });

    // 2. 不正な JXL バイト列を 200 で返す（チェックサムは一致）
    await page.route(`**/content/${MOCK_CID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/octet-stream',
        headers: {
          'x-forensic-checksum': hash,
          'content-type': 'application/octet-stream'
        },
        body: invalidJxl
      });
    });

    await page.route(`**/content/${MOCK_CID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });

    // 認証のモック
    await page.route('**/auth/login', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ nonce: 'test-nonce' }) });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: 'mock_user_hash', token: 'mock_jwt_token' })
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();

    // デコード失敗表示は検証完了後に出るため、詳細文言の出現を待つ。
    await expect(page.getByTestId('jxl-decode-error-detail')).toBeVisible({ timeout: 30000 });

    // 実装側と確定した文言・testid（client/src/lib.rs の decode_error 分岐）。
    await expect(page.getByTestId('content-verification-error')).toBeVisible();
    // role="alert" はライブリージョンでアクセシブル名が本文から決まらないため、
    // 名前ではなく role 属性と testid の両方をセレクタで確認する。
    await expect(page.locator('[data-testid="jxl-decode-error"][role="alert"]')).toBeVisible();
    await expect(page.getByTestId('jxl-decode-error')).toHaveText('JXL Decode Failed');
    await expect(page.getByTestId('jxl-decode-error-detail')).toHaveText('JXL のデコードに失敗しました。');
    await expect(page.getByTestId('content-verification-badge')).toHaveText('× DECODE FAILED');

    // R1-13 の回帰: デコード失敗時に VERIFIED や改ざん検知文言が出ないこと。
    // 上でバッジ文言・エラー詳細の肯定アサーションが通る = 状態遷移が確定している。
    // その上で否定を expect.poll に置き、描画前の瞬間に成立する脆弱な
    // 単発 toHaveCount(0) を避ける（.async な遅延描画でも取りこぼさない）。
    await expect(page.getByTestId('content-verification-badge'))
      .toHaveText('× DECODE FAILED', { timeout: 30000 });
    await expect
      .poll(() => page.locator('span', { hasText: '● VERIFIED' }).count(), { timeout: 5000 })
      .toBe(0);
    await expect
      .poll(() => page.getByText('Integrity Breach Detected').count(), { timeout: 5000 })
      .toBe(0);
    await expect
      .poll(() => page.getByText('Wasm checksum does not match forensic registry.').count(), { timeout: 5000 })
      .toBe(0);

    // ゲートウェイ取得は 200 で 1 回だけ（チェックサム一致によるデコード試行）。
    expect(gatewayStatuses).toEqual([200]);
  });

  test('should render FORENSIC_TIMELINE in log_id DESC order (newest first)', async ({ page }) => {
    const hash = computeHash(ORIGINAL_CONTENT);

    // 実 API（9c0aafa）と同じく /audit は log_id 新しい順（DESC）で返す。
    // reason を一意にして、DOM 上の並びから順序を検証できるようにする。
    const auditLogs = [
      {
        log_id: 3,
        old_status: 'Approved',
        new_status: 'Reported',
        changed_by: 'reporter-3',
        reason: 'newest log 3',
        created_at: '2026-11-18T03:00:00+09:00',
        entry_hash: '3333',
      },
      {
        log_id: 2,
        old_status: 'Approved',
        new_status: 'Reported',
        changed_by: 'reporter-2',
        reason: 'middle log 2',
        created_at: '2026-11-18T02:00:00+09:00',
        entry_hash: '2222',
      },
      {
        log_id: 1,
        old_status: null,
        new_status: 'Approved',
        changed_by: 'SYSTEM',
        reason: 'oldest log 1',
        created_at: '2026-11-18T01:00:00+09:00',
        entry_hash: '1111',
      },
    ];

    // 1. コンテンツインデックスのモック（ログイン前に登録）
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: MOCK_CID,
          checksum: hash,
          type: 'Novel',
          status: 'Approved',
          price: 0,
          title: 'Forensic Evidence Log',
          description: 'Audit timeline ordering fixture',
          tags: ['forensic', 'audit'],
          is_favorite: false
        }])
      });
    });

    // 2. コンテンツ実体のモック（チェックサム一致）
    await page.route(`**/content/${MOCK_CID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        headers: {
          'x-forensic-checksum': hash,
          'content-type': 'text/markdown'
        },
        body: ORIGINAL_CONTENT
      });
    });

    // 3. /audit モック（log_id DESC の3件）
    await page.route(`**/content/${MOCK_CID}/audit`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(auditLogs)
      });
    });

    // 認証のモック
    await page.route('**/auth/login', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ nonce: 'test-nonce' }) });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: 'mock_user_hash', token: 'mock_jwt_token' })
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();

    // 検証完了（カード描画と /audit 取得）を待つ。
    await expect(page.locator('span', { hasText: '● VERIFIED' })).toBeVisible({ timeout: 30000 });

    // 既存 testid（client/src/lib.rs の ContentViewer）でカードを特定し、Audit ボタンを押す。
    const card = page.getByTestId('content-card').first();
    await card.getByRole('button', { name: 'Audit' }).click();

    await expect(card.getByText('FORENSIC_TIMELINE')).toBeVisible();
    // 取得失敗ではないことを既存 testid で確認する。
    await expect(card.getByTestId('audit-error')).toHaveCount(0);

    // 各エントリの Reason 行は <p class="italic"> に描画される（client/src/lib.rs の ContentViewer）。
    // モックは log_id DESC で返すため、DOM 順も 3 → 2 → 1 でなければならない。
    const reasonLines = card.getByText('FORENSIC_TIMELINE').locator('..').locator('p.italic');
    await expect(reasonLines).toHaveText([
      'Reason: newest log 3',
      'Reason: middle log 2',
      'Reason: oldest log 1',
    ]);

    // 各 Reason 行の直後に要約注記が添わる（全部で3行）。全文は署名付きエクスポート側の契約。
    await expect(card.getByText('※ 表示は要約です。全文は署名付きエクスポートで確認')).toHaveCount(3);

    // R2: 監査日時は Intl.DateTimeFormat で整形表示され、生 ISO の区切り 'T' を含まない。
    // ロケール依存の完全一致は期待せず、書式と生 ISO 属性の保持のみを確認する。
    const createdAtCells = card.getByTestId('audit-created-at');
    const displayedDates = await createdAtCells.allTextContents();
    expect(displayedDates).toHaveLength(3);
    for (const displayed of displayedDates) {
      expect(displayed).not.toContain('T');
      expect(displayed.trim()).not.toBe('');
    }
    const rawDates = await createdAtCells.evaluateAll((els) =>
      els.map((el) => el.getAttribute('title'))
    );
    for (const raw of rawDates) {
      expect(raw).toContain('T');
    }
  });

  test('R2: Audit toggle exposes ARIA state and dialog closes on Escape', async ({ page }) => {
    const hash = computeHash(ORIGINAL_CONTENT);

    // コンテンツカードを描画するためのモック（既存テストと同じ経路）。
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: MOCK_CID,
          checksum: hash,
          type: 'Novel',
          status: 'Approved',
          price: 0,
          title: 'Forensic Evidence Log',
          description: 'ARIA toggle fixture',
          tags: ['forensic', 'a11y'],
          is_favorite: false
        }])
      });
    });

    await page.route(`**/content/${MOCK_CID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        headers: {
          'x-forensic-checksum': hash,
          'content-type': 'text/markdown'
        },
        body: ORIGINAL_CONTENT
      });
    });

    await page.route(`**/content/${MOCK_CID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });

    await page.route('**/auth/login', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ nonce: 'test-nonce' }) });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: 'mock_user_hash', token: 'mock_jwt_token' })
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.locator('span', { hasText: '● VERIFIED' })).toBeVisible({ timeout: 30000 });

    const card = page.getByTestId('content-card').first();
    const auditToggle = card.getByRole('button', { name: 'Audit' });

    // 初期状態: 閉じており、aria-controls でタイムラインパネルを参照している。
    await expect(auditToggle).toHaveAttribute('aria-expanded', 'false');
    await expect(auditToggle).toHaveAttribute('aria-controls', /^forensic-timeline-.+$/);

    await auditToggle.click();

    // クリックで展開状態になり、アクセシブル名付きの dialog が表示される。
    await expect(auditToggle).toHaveAttribute('aria-expanded', 'true');
    const dialog = page.getByRole('dialog', { name: 'Forensic Timeline' });
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute('id', /^forensic-timeline-.+$/);

    // aria-controls の参照先と dialog の id が厳密に一致することを確認する。
    const controls = await auditToggle.getAttribute('aria-controls');
    await expect(dialog).toHaveAttribute('id', controls as string);

    // Escape はトグル（カード内）で受け取り、カードの keydown ハンドラで閉じる。
    await auditToggle.focus();
    await page.keyboard.press('Escape');
    await expect(auditToggle).toHaveAttribute('aria-expanded', 'false');
    await expect(dialog).toHaveCount(0);
    await expect(auditToggle).toBeFocused();
  });

  test('R2: Audit toggle dialog closes via CLOSE button and restores focus', async ({ page }) => {
    const hash = computeHash(ORIGINAL_CONTENT);

    // Escape ケースと同じモックを用意し、CLOSE ボタン経路だけを独立して切り出す。
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: MOCK_CID,
          checksum: hash,
          type: 'Novel',
          status: 'Approved',
          price: 0,
          title: 'Forensic Evidence Log',
          description: 'ARIA toggle CLOSE fixture',
          tags: ['forensic', 'a11y'],
          is_favorite: false
        }])
      });
    });

    await page.route(`**/content/${MOCK_CID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        headers: {
          'x-forensic-checksum': hash,
          'content-type': 'text/markdown'
        },
        body: ORIGINAL_CONTENT
      });
    });

    await page.route(`**/content/${MOCK_CID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });

    await page.route('**/auth/login', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ nonce: 'test-nonce' }) });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: 'mock_user_hash', token: 'mock_jwt_token' })
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.locator('span', { hasText: '● VERIFIED' })).toBeVisible({ timeout: 30000 });

    const card = page.getByTestId('content-card').first();
    const auditToggle = card.getByRole('button', { name: 'Audit' });
    await expect(auditToggle).toHaveAttribute('aria-expanded', 'false');

    await auditToggle.click();
    const dialog = page.getByRole('dialog', { name: 'Forensic Timeline' });
    await expect(dialog).toBeVisible();
    await expect(auditToggle).toHaveAttribute('aria-expanded', 'true');

    const closeButton = dialog.getByRole('button', { name: 'CLOSE' });

    // モーダル中核契約①: requestAnimationFrame による初期フォーカス。
    // オープン直後にダイアログ内 CLOSE がフォーカスされる。rAF の遅延があるため
    // toBeFocused の自動待ち（expect retries）に任せ、固定 sleep は置かない。
    await expect(closeButton).toBeFocused();

    // モーダル中核契約②: フォーカストラップ。CLOSE フォーカス中に Tab / Shift+Tab を
    // 送っても背後のトグル等へ抜けず CLOSE に留まる（ContentViewer の on:keydown フォーカス制御）。
    await page.keyboard.press('Tab');
    await expect(closeButton).toBeFocused();
    await page.keyboard.press('Shift+Tab');
    await expect(closeButton).toBeFocused();

    // Escape ではなく dialog 内の CLOSE ボタンで閉じる（audit_close_ref / forensic_panel_id のパネル）。
    await closeButton.click();

    await expect(auditToggle).toHaveAttribute('aria-expanded', 'false');
    await expect(dialog).toHaveCount(0);
    await expect(auditToggle).toBeFocused();
  });
});
