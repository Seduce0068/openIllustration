import { test, expect } from '@playwright/test';

test.describe('Authentication Flow (SEC-01, SEC-03)', () => {
  test('should login successfully with mock JPKI token', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('h1')).toContainText('OPENILLUSTRATION');
    // 初回マウントではフォーカスを奪わない（view 変更時のみ #main-content へ移す）。
    await expect(page.locator('#main-content')).not.toBeFocused();

    const loginButton = page.getByRole('button', { name: 'CONNECT WITH JPKI' });
    await expect(loginButton).toBeVisible();
    await loginButton.click();

    await expect(page.getByRole('button', { name: 'UPLOAD' })).toBeVisible({ timeout: 10000 });

    // R2-A11Y: view が login → dashboard へ変わったら本文へフォーカスが移る。
    // 初回マウントでは奪わず、実際の view 変更時だけ移す契約を検証する。
    await expect(page.locator('#main-content')).toBeFocused();

    const userHash = page.getByTestId('user-hash');
    await expect(userHash).toBeVisible();
    const hashText = await userHash.innerText();
    expect(hashText.length).toBeGreaterThan(0);
  });

  test('SEC-03: should reject underage user', async ({ page }) => {
    await page.route('**/auth/verify', async route => {
      // 送信中の aria-busy / role=status を観測するため、応答を少し保留する。
      await new Promise(resolve => setTimeout(resolve, 300));
      await route.fulfill({
        status: 403,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Age verification failed for JP: age 9, required 18' })
      });
    });

    await page.goto('/');
    const loginButton = page.getByRole('button', { name: 'CONNECT WITH JPKI' });

    // R2-CLIENT-LOGIN-GUARD: 二重クリックで /auth/login と /auth/verify が
    // 1本ずつかを記録する（既存ケース内のみ、新規 test() なし）。
    // 1回目のクリックで文言が VERIFYING... へ変わるため、2回目は名前依存しない
    // 安定セレクタ（lang + aria-busy）で同じボタンを直接指定する。
    const loginButtonDom = page.locator('button[lang="en"][aria-busy]');
    let loginRequests = 0;
    let verifyRequests = 0;
    page.on('request', request => {
      if (request.url().includes('/auth/login')) loginRequests += 1;
      if (request.url().includes('/auth/verify')) verifyRequests += 1;
    });

    // 固定スリープではなく、インターセプトした 403 レスポンスの完了を待つ。
    const rejected = page.waitForResponse(
      response => response.url().includes('/auth/verify') && response.status() === 403,
      { timeout: 10000 }
    );
    // 2回の click イベントを連続で発火させ、disabled の DOM 反映より前に
    // claim_submission が2回目を止めることを確認する。
    await loginButtonDom.dispatchEvent('click');
    await loginButtonDom.dispatchEvent('click');

    // R2-CLIENT-LOGIN-BUSY: 送信中はボタンが disabled / aria-busy=true になり、
    // 専用の role=status が日本語の進捗を告知する（lang="ja" を明示）。
    // ボタンの文言は VERIFYING... へ変わるため、両方の文言に一致する locator を使う。
    const busyButton = page.getByRole('button', { name: /VERIFYING|CONNECT WITH JPKI/ });
    await expect(busyButton).toBeDisabled();
    await expect(busyButton).toHaveAttribute('aria-busy', 'true');
    const loginProgress = page.getByTestId('login-progress');
    await expect(loginProgress).toHaveAttribute('role', 'status');
    await expect(loginProgress).toHaveAttribute('lang', 'ja');
    await expect(loginProgress).toContainText('本人確認の通信中です。');

    await rejected;
    await expect(busyButton).toHaveAttribute('aria-busy', 'false');
    await expect(loginProgress).toHaveCount(0);

    // R2-CLIENT-LOGIN-RESULT: 失敗時は role=alert を表示しボタンを再活性化する。
    // P4: /auth/login の 403 は IPv6 要件の dedicated 文言へ分岐するため、
    // 汎用 login-error ではなく dedicated alert を検証する（auth 3件維持）。
    const loginError = page.getByTestId('login-ipv4-alert');
    await expect(loginError).toHaveAttribute('role', 'alert');
    // R2-CLIENT-LANG: 日本語固定文言の role=alert は error_lang 経由で lang="ja" を明示する。
    await expect(loginError).toHaveAttribute('lang', 'ja');
    await expect(loginError).toContainText('IPv6');
    await expect(loginButton).toBeEnabled();

    // 二重送信ガード: /auth/login と /auth/verify はいずれも1本だけ。
    expect(loginRequests).toBe(1);
    expect(verifyRequests).toBe(1);

    // P4: /auth/login が 403 のときだけ IPv6 要件の専用固定文言を dedicated role=alert で出す。
    // サーバー本文（センチネル）や IP はエコーしない（既存ケース内へ統合、auth 3件維持）。
    await page.route('**/auth/login', async route => {
      await route.fulfill({
        status: 403,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'IPv4 203.0.113.7 SENTINEL_LOGIN_BODY_MUST_NOT_BE_SHOWN' }),
      });
    });
    await loginButton.click();
    const ipv4Alert = page.getByTestId('login-ipv4-alert');
    await expect(ipv4Alert).toBeVisible({ timeout: 10000 });
    await expect(ipv4Alert).toHaveAttribute('role', 'alert');
    await expect(ipv4Alert).toHaveAttribute('lang', 'ja');
    await expect(ipv4Alert).toHaveText(
      'IPv6 接続が必要です。IPv4 のみの環境では IPv6 対応回線へ切り替えてください。',
    );
    await expect(ipv4Alert).not.toContainText('203.0.113.7');
    await expect(ipv4Alert).not.toContainText('SENTINEL_LOGIN_BODY_MUST_NOT_BE_SHOWN');
    await expect(page.getByTestId('login-error')).toHaveCount(0);

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
