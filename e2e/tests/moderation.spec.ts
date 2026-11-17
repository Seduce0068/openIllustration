import { test, expect } from '@playwright/test';

test.describe('Moderation & Governance (GOV-01, GOV-02)', () => {
  const mockCID = 'QmVPyxX9eKoR9LaKp2Hgt9rVkAffptArJ3zW5hvPLE713h';

  // GET /mod/reports の reason はサーバー側 common::summarize_reason で
  // PII 除去 + 最大120文字に要約される。E2E では実サーバー結合を行わず、
  // 要約済み文字列をモックから返して UI 側の表示契約を検証する。
  const summarizedReason = '不審な投稿です [redacted]';
  const piiFixture = {
    email: 'reporter@example.com',
    phone: '090-1234-5678',
    url: 'https://phish.example.com/login',
  };
  const reportFixture = (reason: string) => ({
    report_id: 42,
    target_cid: 'QmRRTBnaNs7jwYiWadPrEDgrs9a199VZUD3ftVLEJ5DJw5',
    reason,
    created_at: '2026-11-18T00:00:00Z',
    status: 'Pending',
  });

  test('GOV-01: should report content and transition to Reported status', async ({ page }) => {
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

    await page.route('**/mod/report', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(true),
      });
    });
    // 陪審員キューへ要約済み reason のレポートを1件返す。
    await page.route('**/mod/reports', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([reportFixture(summarizedReason)]),
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'MODERATION' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'MODERATION' }).click();

    // サーバーが要約した reason がそのまま投票キューに表示されること。
    await expect(page.getByText('Jury Voting Queue')).toBeVisible({ timeout: 10000 });
    const reasonLine = page.locator('p:has-text("Reason:")').first();
    await expect(reasonLine).toContainText(summarizedReason, { timeout: 10000 });
    await expect(page.getByText('QmRRTBnaNs7jwYiWadPrEDgrs9a199VZUD3ftVLEJ5DJw5')).toBeVisible();

    await page.locator('input[placeholder="TARGET CID"]').fill('QmTtdWPtHiGyv92HS8cisWMXVh8afyrRUJPDYUraxfCFm2');
    await page.locator('textarea[placeholder="REASON"]').fill('Illegal content detected');

    await page.getByRole('button', { name: 'SUBMIT FORENSIC REPORT' }).click();

    await expect(page.locator('text=Report accepted. Content marked Reported')).toBeVisible({ timeout: 10000 });
  });

  test('GOV-01 (summary): PII を含む元理由は要約済み文字列のみ表示され、メール/電話/URL は露出しない', async ({ page }) => {
    // 元の通報理由は次のような全文（サーバー側で要約される前の想定値）:
    //   `不審な投稿です。連絡先: reporter@example.com / 090-1234-5678 / https://phish.example.com/login`
    // common::summarize_reason が PII を [redacted] に置換し最大120文字へ丸めるため、
    // クライアントが受け取るのは summarizedReason のみ。実サーバー結合は行わない。
    await page.route('**/auth/login', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ nonce: 'test-nonce' }) });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: 'juror_hash', token: 'mock_jwt_token' })
      });
    });
    await page.route('**/mod/reports', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([reportFixture(summarizedReason)]),
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'MODERATION' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'MODERATION' }).click();

    // 要約済み文字列と [redacted] は表示される。
    await expect(page.getByText('Jury Voting Queue')).toBeVisible({ timeout: 10000 });
    const reasonLine = page.locator('p:has-text("Reason:")').first();
    await expect(reasonLine).toContainText(summarizedReason, { timeout: 10000 });
    await expect(reasonLine).toContainText('[redacted]');

    // 元全文に含まれていたメールアドレス・電話番号・URL は UI に出ない。
    for (const pii of [piiFixture.email, piiFixture.phone, piiFixture.url]) {
      await expect(reasonLine).not.toContainText(pii);
      await expect(page.locator('body')).not.toContainText(pii);
    }
  });

  test('should display audit trail for content', async ({ page }) => {
    const mockAuditLogs = [
      {
        log_id: 1,
        old_status: null,
        new_status: 'Approved',
        changed_by: 'SYSTEM',
        reason: 'Initial upload',
        created_at: '2026-06-08T00:00:00Z',
        entry_hash: 'abc123',
      },
      {
        log_id: 2,
        old_status: 'Approved',
        new_status: 'Reported',
        changed_by: 'user_hash_123',
        reason: 'User report: suspicious content',
        created_at: '2026-06-08T01:00:00Z',
        entry_hash: 'def456',
      },
    ];

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

    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: mockCID,
          checksum: 'mock_checksum',
          type: 'Novel',
          status: 'Reported',
          price: 100,
          title: 'Audit Fixture',
          description: 'Content with a forensic timeline',
          tags: ['audit', 'mock'],
          is_favorite: false
        }]),
      });
    });

    await page.route(`**/content/${mockCID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        body: 'Test content',
      });
    });

    await page.route(`**/content/${mockCID}/audit`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(mockAuditLogs),
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'BROWSE' }).click();
    
    const auditButton = page.locator('button:has-text("Audit")').first();
    await expect(auditButton).toBeVisible({ timeout: 20000 });
    await auditButton.click();

    await expect(page.locator('text=FORENSIC_TIMELINE')).toBeVisible();
    await expect(page.locator('text=Initial upload')).toBeVisible();
    await expect(page.locator('text=User report: suspicious content')).toBeVisible();
  });

  test('R5-09: should display moderation stats for operators and permission error on 403', async ({ page }) => {
    const statsFixture = {
      generated_at: '2026-11-18T00:00:00Z',
      totals: {
        reports: 3,
        concluded: 2,
        approved: 1,
        rejected: 1,
        quorum_not_met: 0,
        bans: 1,
        unbans: 0,
        unpins: 0,
      },
      monthly: [
        {
          month: '2026-11',
          reports: 3,
          concluded: 2,
          approved: 1,
          rejected: 1,
          bans: 1,
          unbans: 0,
          unpins: 0,
        },
      ],
    };

    // 同じテスト内で 200 → 403 を切り替える（画面遷移で再フェッチさせる）。
    let forbidStats = false;
    let statsRequestCount = 0;
    let statsAuthHeader: string | undefined;
    // 再フェッチ回数とリクエスト内容を完全一致で検証し、過剰リクエストを検出する。
    const statsRequests: Array<{ method: string; path: string; authorization: string | undefined }> = [];

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
    await page.route('**/mod/reports', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify([]) });
    });
    await page.route('**/mod/stats', async route => {
      statsRequestCount += 1;
      statsAuthHeader = route.request().headers()['authorization'];
      statsRequests.push({
        method: route.request().method(),
        path: new URL(route.request().url()).pathname,
        authorization: statsAuthHeader,
      });
      if (forbidStats) {
        await route.fulfill({
          status: 403,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'forbidden' }),
        });
      } else {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify(statsFixture),
        });
      }
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'MODERATION' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'MODERATION' }).click();

    // 認証トークン付きで集計が取得され、集計カード / 月次表が表示されること。
    await expect(page.getByTestId('moderation-stats')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('moderation-stats-section')).toContainText('TRANSPARENCY DASHBOARD');
    await expect(page.getByTestId('moderation-stats')).toContainText('GENERATED_AT: 2026-11-18T00:00:00Z');
    await expect(page.getByTestId('moderation-stats-totals')).toContainText('Reports');
    await expect(page.getByTestId('moderation-stats-totals')).toContainText('Quorum Not Met');
    await expect(page.getByTestId('moderation-stats-monthly')).toContainText('2026-11');
    await expect(page.getByTestId('moderation-stats-error')).toHaveCount(0);
    expect(statsAuthHeader).toBe('Bearer mock_jwt_token');

    // 403 では権限エラーを表示し、集計カードを出さないこと。
    forbidStats = true;
    await page.getByRole('button', { name: 'BROWSE' }).click();
    await page.getByRole('button', { name: 'MODERATION' }).click();
    await expect(page.getByTestId('moderation-stats-error')).toHaveText('運用者権限が必要です', { timeout: 10000 });
    await expect(page.getByTestId('moderation-stats')).toHaveCount(0);

    // 集計は初期表示と画面遷移後の2回だけ。メソッド・パス・Authorization も
    // 完全一致で検証し、余計な再フェッチや未認証リクエストを検出する。
    expect(statsRequestCount).toBe(2);
    expect(statsRequests).toEqual([
      { method: 'GET', path: '/mod/stats', authorization: 'Bearer mock_jwt_token' },
      { method: 'GET', path: '/mod/stats', authorization: 'Bearer mock_jwt_token' },
    ]);
  });
});
