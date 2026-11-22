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
        body: JSON.stringify({ authorized: true, user_hash: 'mock_user_hash', token: 'mock_jwt_token' }),
        // 実サーバの callback と同じく refresh_token Cookie を設定し、
        // ログアウト時の credentials: Include で送られることを検証可能にする。
        headers: { 'set-cookie': 'refresh_token=mock_refresh; Path=/auth; HttpOnly; SameSite=Strict' },
      });
    });

    // /auth/logout をモックしてメソッドと Authorization ヘッダを記録する。
    let logoutMethod: string | undefined;
    let logoutAuthHeader: string | undefined;
    let logoutCookie: string | undefined;
    await page.route('**/auth/logout', async route => {
      logoutMethod = route.request().method();
      logoutAuthHeader = route.request().headers()['authorization'];
      // 実構成では app/api が同一サイトのため credentials: Include で Cookie が送られる。
      // E2E は localhost と 127.0.0.1 の別ホストのため Cookie 送信は検証しない。
      logoutCookie = route.request().headers()['cookie'];
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: 'true',
        headers: { 'set-cookie': 'refresh_token=; Path=/auth; HttpOnly; SameSite=Strict; Max-Age=0' },
      });
    });

    // R2-REFRESH-SEC: /auth/refresh モックは VerifyResponse 契約
    // （authorized / user_hash / token）を満たす200応答へ揃える。
    // R2-REFRESH-ORIGIN: Origin / Sec-Fetch-Site も従来どおり記録する。
    let refreshOrigin: string | undefined;
    let refreshFetchSite: string | undefined;
    const refreshBody = { authorized: true, user_hash: 'mock_user_hash', token: 'refreshed_jwt_token' };
    await page.route('**/auth/refresh', async route => {
      const headers = route.request().headers();
      refreshOrigin = headers['origin'];
      refreshFetchSite = headers['sec-fetch-site'];
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(refreshBody),
      });
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

    // R2-CLIENT-05: /auth/logout が失敗しても画面遷移は現行どおりログインへ戻し、
    // dedicated role=alert で再試行を案内する（auth 3件は不変）。
    let abortedLogout = 0;
    await page.unroute('**/auth/logout');
    await page.route('**/auth/logout', async route => {
      abortedLogout += 1;
      await route.abort();
    });
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'UPLOAD' })).toBeVisible({ timeout: 10000 });
    await page.getByTestId('logout-button').click();

    // 失敗時もログイン画面へ戻る（画面遷移の現行契約を維持）。
    await expect(page.getByRole('button', { name: 'CONNECT WITH JPKI' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'UPLOAD' })).toHaveCount(0);
    const logoutError = page.getByTestId('logout-error');
    await expect(logoutError).toHaveAttribute('role', 'alert');
    await expect(logoutError).toHaveText(
      'ログアウト処理が完了しませんでした。再試行してください。',
      { timeout: 10000 },
    );
    await expect.poll(() => abortedLogout).toBe(1);

    // R2-REFRESH-ORIGIN: POST /auth/refresh を明示1回叩き、Origin が API_BASE と
    // 一致することを記録・検証する。Sec-Fetch-Site はブラウザが付与する値に
    // 依存するため、サーバ契約どおり cross-site でないことのみを確認する。
    const probe = await page.evaluate(async () => {
      const API_BASE = window.location.origin;
      const response = await fetch('/auth/refresh', { method: 'POST' });
      return { API_BASE, status: response.status, body: await response.json() };
    });
    expect(probe.status).toBe(200);
    // R2-REFRESH-SEC: モック本文が VerifyResponse 契約どおりであることを検証する。
    expect(probe.body).toEqual(refreshBody);
    await expect.poll(() => refreshOrigin).toBe(probe.API_BASE);
    // ヘッダ欠損はサーバ契約で許可のため、cross-site でなければよい。
    expect(refreshFetchSite).not.toBe('cross-site');

    // R2-MOCK-SHFMT: 本番認証未実装（501）の固定文言と verify 未送信を検証する。
    // ログイン画面復帰後に /auth/login を501＋センチネル本文へ差し替える（既存ケース内へ統合）。
    await page.unroute('**/auth/login');
    const loginSentinel = 'SENTINEL_LOGIN_BODY_MUST_NOT_BE_SHOWN';
    await page.route('**/auth/login', async route => {
      await route.fulfill({
        status: 501,
        contentType: 'text/plain',
        body: loginSentinel
      });
    });

    // 未モックの /auth/verify が飛ばないことをリクエスト監視で確認する。
    await page.unroute('**/auth/verify');
    let verifyRequests = 0;
    page.on('request', request => {
      if (request.url().includes('/auth/verify')) {
        verifyRequests += 1;
      }
    });

    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();

    const loginError = page.getByTestId('login-error');
    await expect(loginError).toHaveAttribute('role', 'alert');
    await expect(loginError).toHaveText('本番認証は未実装です。');
    await expect(page.getByRole('button', { name: 'UPLOAD' })).not.toBeVisible();
    expect(await page.locator('body').innerText()).not.toContain(loginSentinel);
    expect(verifyRequests).toBe(0);
  });
});
