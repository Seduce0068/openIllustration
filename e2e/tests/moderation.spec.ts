import { test, expect } from '@playwright/test';

test.describe('Moderation & Governance (GOV-01, GOV-02)', () => {
  const mockCID = 'QmAuditTestCID';

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

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'MODERATION' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'MODERATION' }).click();

    await page.locator('input[placeholder="TARGET CID"]').fill('QmTestReportCID');
    await page.locator('textarea[placeholder="REASON"]').fill('Illegal content detected');

    await page.getByRole('button', { name: 'SUBMIT FORENSIC REPORT' }).click();

    await expect(page.locator('text=Report accepted. Content marked Reported')).toBeVisible({ timeout: 10000 });
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
    expect(statsRequestCount).toBeGreaterThanOrEqual(2);
  });
});
