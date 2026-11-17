import { test, expect, type Page } from '@playwright/test';

/**
 * R5-04: 自分のデータをエクスポート（削除機能は含まない）。
 * `/me/export` はモックし、認証ヘッダー・ダウンロード・エラー表示を検証する。
 */
test.describe('R5-04: Export My Data', () => {
  const exportFixture = {
    version: 1,
    generated_at: '2026-11-18T00:00:00Z',
    sub: 'mock_sub_hash',
    users: [{ sub: 'mock_sub_hash' }],
    contents: [{ cid: 'QmRh161kL1ZzjpqnbeeGfeTycfyz9sjriFRgJNCZmDhs6y' }],
    favorites: [],
    transactions: [],
    signatures: [],
    content_audit_log: [],
    reports: [],
    votes: [],
    payment_orders: [],
    governance_audit_log: [],
    refresh_tokens: [],
    revoked_subs: [],
  };

  async function loginAndOpenDashboard(page: Page): Promise<void> {
    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('export-my-data')).toBeVisible({ timeout: 10000 });
  }

  test.beforeEach(async ({ page }) => {
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
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify([]) });
    });
  });

  test('R5-04: GET /me/export を認証トークン付きで呼び、JSON ダウンロードがトリガされること', async ({ page }) => {
    let requestMethod: string | undefined;
    let authHeader: string | undefined;
    let requestCount = 0;

    await page.route('**/me/export', async route => {
      requestCount += 1;
      requestMethod = route.request().method();
      authHeader = route.request().headers()['authorization'];
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(exportFixture),
      });
    });

    await loginAndOpenDashboard(page);

    const downloadPromise = page.waitForEvent('download');
    await page.getByTestId('export-my-data').click();
    const download = await downloadPromise;

    expect(requestCount).toBe(1);
    expect(requestMethod).toBe('GET');
    expect(authHeader).toBe('Bearer mock_jwt_token');
    expect(download.suggestedFilename()).toBe('openillustration-export.json');
    await expect(page.getByRole('alert')).toHaveCount(0);
    await expect(page.getByTestId('export-status')).toContainText('エクスポート完了');
  });

  test('R5-04: /me/export が 401 の場合は「ログインが必要です」を role=alert で表示すること', async ({ page }) => {
    await page.route('**/me/export', async route => {
      await route.fulfill({
        status: 401,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'unauthorized' }),
      });
    });

    await loginAndOpenDashboard(page);
    await page.getByTestId('export-my-data').click();

    await expect(page.getByTestId('export-error')).toHaveText('ログインが必要です', { timeout: 10000 });
    await expect(page.getByRole('alert')).toHaveText('ログインが必要です');
    await expect(page.getByTestId('export-status')).toHaveCount(0);
  });
});
