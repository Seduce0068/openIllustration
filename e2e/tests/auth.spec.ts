import { test, expect } from '@playwright/test';

test.describe('Authentication Flow (SEC-01, SEC-03)', () => {
  test('should login successfully with mock JPKI token', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('h1')).toContainText('OPENILLUSTRATION');

    const loginButton = page.getByRole('button', { name: 'CONNECT WITH JPKI' });
    await expect(loginButton).toBeVisible();
    await loginButton.click();

    await expect(page.getByRole('button', { name: 'UPLOAD' })).toBeVisible({ timeout: 10000 });
    
    const userHash = page.getByTestId('user-hash');
    await expect(userHash).toBeVisible();
    const hashText = await userHash.innerText();
    expect(hashText.length).toBeGreaterThan(0);
  });

  test('SEC-03: should reject underage user', async ({ page }) => {
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 403,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Age verification failed for JP: age 9, required 18' })
      });
    });

    await page.goto('/');
    const loginButton = page.getByRole('button', { name: 'CONNECT WITH JPKI' });

    // 固定スリープではなく、インターセプトした 403 レスポンスの完了を待つ。
    const rejected = page.waitForResponse(
      response => response.url().includes('/auth/verify') && response.status() === 403,
      { timeout: 10000 }
    );
    await loginButton.click();
    await rejected;

    await expect(page.getByRole('button', { name: 'UPLOAD' })).not.toBeVisible();
  });

  test('should log out and return to login screen with Bearer header on /auth/logout', async ({ page }) => {
    // ダミーのモック認証のみ（秘密情報は使わない）。
    await page.route('**/auth/login', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ nonce: 'test-nonce' })
      });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: 'mock_user_hash', token: 'mock_jwt_token' })
      });
    });

    // /auth/logout をモックしてメソッドと Authorization ヘッダを記録する。
    let logoutMethod: string | undefined;
    let logoutAuthHeader: string | undefined;
    await page.route('**/auth/logout', async route => {
      logoutMethod = route.request().method();
      logoutAuthHeader = route.request().headers()['authorization'];
      await route.fulfill({ status: 200, contentType: 'application/json', body: 'true' });
    });

    // R2-REFRESH-ORIGIN: /auth/refresh はモック200で通し、Origin / Sec-Fetch-Site を記録する。
    let refreshOrigin: string | undefined;
    let refreshFetchSite: string | undefined;
    await page.route('**/auth/refresh', async route => {
      const headers = route.request().headers();
      refreshOrigin = headers['origin'];
      refreshFetchSite = headers['sec-fetch-site'];
      await route.fulfill({ status: 200, contentType: 'application/json', body: 'true' });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'UPLOAD' })).toBeVisible({ timeout: 10000 });

    const logoutButton = page.getByTestId('logout-button');
    await expect(logoutButton).toHaveText('LOG OUT');
    await logoutButton.click();

    // 画面は API 成否にかかわらず即座にログインへ戻る（client の handle_logout 契約）。
    await expect(page.getByRole('button', { name: 'UPLOAD' })).not.toBeVisible();
    await expect(page.getByRole('button', { name: 'CONNECT WITH JPKI' })).toBeVisible();

    // リクエストは spawn_local で飛ぶため、到達までポールで待つ。
    await expect.poll(() => logoutAuthHeader).toBe('Bearer mock_jwt_token');
    expect(logoutMethod).toBe('POST');

    // R2-REFRESH-ORIGIN: POST /auth/refresh を明示1回叩き、Origin が API_BASE と
    // 一致することを記録・検証する。Sec-Fetch-Site はブラウザが付与する値に
    // 依存するため、サーバ契約どおり cross-site でないことのみを確認する。
    const probe = await page.evaluate(async () => {
      const API_BASE = window.location.origin;
      const response = await fetch('/auth/refresh', { method: 'POST' });
      return { API_BASE, status: response.status };
    });
    expect(probe.status).toBe(200);
    await expect.poll(() => refreshOrigin).toBe(probe.API_BASE);
    // ヘッダ欠損はサーバ契約で許可のため、cross-site でなければよい。
    expect(refreshFetchSite).not.toBe('cross-site');
  });
});
