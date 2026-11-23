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

  // client/src/lib.rs の EXPORT_PARSE_ERROR で確定した固定文言。
  const exportParseErrorText = 'エクスポート結果の解析に失敗しました。';

  async function loginAndOpenDashboard(page: Page): Promise<void> {
    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('export-my-data')).toBeVisible({ timeout: 10000 });
    // 一覧フェッチの完了を待ち、クリック後の再描画で export-error が消えないようにする。
    await expect(page.getByText('0件のコンテンツを表示中')).toBeVisible({ timeout: 10000 });
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
    // 後続フェーズでレスポンスを不正 JSON へ差し替えるためのフラグ。
    let malformedResponse = false;
    // R2-BUSY: 最初の応答を明示的に解放するまで保留し、保留中の二重クリックを
    // 固定スリープに依存せず決定論的に観測する。
    let releaseExport: () => void = () => {};
    const exportHeld = new Promise<void>(resolve => { releaseExport = resolve; });

    await page.route('**/me/export', async route => {
      requestCount += 1;
      requestMethod = route.request().method();
      authHeader = route.request().headers()['authorization'];
      if (!malformedResponse) {
        // busy 中の aria-busy / アクセシブル名を決定論的に観測するため応答を保留する。
        await exportHeld;
      }
      if (malformedResponse) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          // version が文字列の不正 JSON。DataExport として読めず、
          // 「0 件」成功にせず固定エラーを表示する（EXPORT-11）。
          body: JSON.stringify({ ...exportFixture, version: 'invalid' }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(exportFixture),
      });
    });

    await loginAndOpenDashboard(page);

    // R2-CLIENT-07: object URL の revoke 予約が固定10秒より十分に遅いことを観測する。
    // web-sys の set_timeout... は setTimeout(fn, timeout) へ写像されるため、
    // setTimeout をラップして予約された遅延値を記録する（開始直後の失効はしない）。
    await page.evaluate(() => {
      const w = window as unknown as { __scheduledTimeouts: number[] };
      w.__scheduledTimeouts = [];
      const original = window.setTimeout.bind(window);
      window.setTimeout = ((handler: TimerHandler, timeout?: number, ...args: unknown[]) => {
        if (typeof timeout === 'number') {
          w.__scheduledTimeouts.push(timeout);
        }
        return original(handler, timeout, ...args);
      }) as typeof window.setTimeout;
    });

    const downloadPromise = page.waitForEvent('download');
    const exportButton = page.getByTestId('export-my-data');
    // R2-CLIENT-08: idle → busy → idle で aria-label が切り替わる。
    await expect(exportButton).toHaveAttribute('aria-label', 'データをエクスポート');
    // R2-BUSY: ARIA boolean は "true"/"false" の文字列トークンを要求する。
    await expect(exportButton).toHaveAttribute('aria-busy', 'false');
    await exportButton.click();
    await expect.poll(() => exportButton.getAttribute('aria-label')).toBe('エクスポート中');
    await expect(exportButton).toHaveAttribute('aria-busy', 'true');
    await expect(exportButton).toBeDisabled();
    // R2-BUSY: レスポンス保留中に disabled を迂回した2回目の click イベントを
    // dispatchEvent で届かせても、claim_submission が同期で排除する。
    await exportButton.dispatchEvent('click');
    expect(requestCount).toBe(1);
    releaseExport();
    const download = await downloadPromise;
    await expect.poll(() => exportButton.getAttribute('aria-label')).toBe('データをエクスポート');
    await expect(exportButton).toHaveAttribute('aria-busy', 'false');
    await expect(page.getByTestId('export-status')).toContainText('エクスポート完了');

    const longDelays = await page.evaluate(() =>
      (window as unknown as { __scheduledTimeouts: number[] }).__scheduledTimeouts.filter(
        delay => delay >= 10_000,
      ),
    );
    expect(longDelays.some(delay => delay > 10_000)).toBe(true);

    // R2-BUSY: 保留中の二重クリック後も /me/export は1本のまま。
    expect(requestCount).toBe(1);
    expect(requestMethod).toBe('GET');
    expect(authHeader).toBe('Bearer mock_jwt_token');
    // R2-P2-DOCS: ファイル名は generated_at 由来の UTC 世代を含む（既存ケース内で検証）。
    const suggestedFilename = download.suggestedFilename();
    expect(suggestedFilename).toBe('openillustration-export-20261118T000000Z.json');
    expect(suggestedFilename).toMatch(/^openillustration-export-\d{8}T\d{6}Z\.json$/);
    // 機微情報（sub / JWT / 本文）はファイル名へ一切含まれない。
    expect(suggestedFilename).not.toContain(exportFixture.sub);
    expect(suggestedFilename).not.toContain('mock_jwt_token');
    await expect(page.getByRole('alert')).toHaveCount(0);
    await expect(page.getByTestId('export-status')).toContainText('エクスポート完了');

    // R2-CLIENT: 成功時に dedicated 要素で整形済み generated_at と sub の短縮表示を出す。
    // 生 ISO / 全文 sub / 生 JWT はエクスポート領域へ出さない。
    const exportGeneratedAt = page.getByTestId('export-generated-at');
    await expect(exportGeneratedAt).toBeVisible();
    await expect(exportGeneratedAt).toContainText('GENERATED_AT: ');
    await expect(exportGeneratedAt).toContainText('2026');
    await expect(exportGeneratedAt).not.toContainText(exportFixture.generated_at);
    const exportSub = page.getByTestId('export-sub');
    await expect(exportSub).toBeVisible();
    await expect(exportSub).toContainText('SUB: mock_sub…');
    await expect(exportSub).not.toContainText(exportFixture.sub);
    await expect(page.getByTestId('export-section')).not.toContainText('mock_jwt_token');

    // 追加フェーズ（EXPORT-11）: 同じケース内でレスポンスを不正 JSON へ差し替える。
    // 型として読めないため export-error に固定文言のみを出し、成功文言は出さない。
    malformedResponse = true;
    await page.getByTestId('export-my-data').click();

    await expect(page.getByTestId('export-error')).toHaveText(exportParseErrorText, { timeout: 10000 });
    await expect(page.getByTestId('export-error')).toHaveAttribute('role', 'alert');
    await expect(page.getByTestId('export-status')).toHaveCount(0);
    await expect(page.getByTestId('export-generated-at')).toHaveCount(0);
    await expect(page.getByTestId('export-sub')).toHaveCount(0);
    expect(requestCount).toBe(2);
  });

  test('R5-04: /me/export が 401 の場合は「ログインが必要です」を role=alert で表示すること', async ({ page }) => {
    // R2-REFRESH-RECOVERY 段C: 失敗フェーズ後に成功側へ差し替えるためのフラグと記録用カウンタ。
    let refreshSucceeds = false;
    let refreshAttempts = 0;
    const exportAuthHeaders: string[] = [];
    // R2-REFRESH-SEC: 新 JWT の失効フェーズと user_hash 不一致フェーズ用のフラグ。
    let refreshedTokenRevoked = false;
    let refreshUserHash = 'mock_user_hash';

    await page.route('**/me/export', async route => {
      const authorization = route.request().headers()['authorization'];
      exportAuthHeaders.push(authorization);
      // refresh 成功後の新トークンによる再送（2回目の試行）だけ 200 を返す。
      // R2-REFRESH-SEC: 失効フェーズでは同じトークンにも 401 を返す。
      if (authorization === 'Bearer refreshed_jwt_token' && !refreshedTokenRevoked) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify(exportFixture),
        });
        return;
      }
      await route.fulfill({
        status: 401,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'unauthorized' }),
      });
    });
    // R2-REFRESH-ENABLE: 最初のクリックは失敗側で、元の 401 を返す契約を維持する。
    // R2-REFRESH-RECOVERY: 既存アサーション完了後にハンドラ内フラグで成功側へ差し替える。
    await page.route('**/auth/refresh', async route => {
      refreshAttempts += 1;
      if (!refreshSucceeds) {
        await route.fulfill({
          status: 401,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'unauthorized' }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: refreshUserHash, token: 'refreshed_jwt_token' })
      });
    });
    await loginAndOpenDashboard(page);
    await page.getByTestId('export-my-data').click();

    await expect(page.getByTestId('export-error')).toHaveText('ログインが必要です', { timeout: 10000 });
    await expect(page.getByRole('alert')).toHaveText('ログインが必要です');
    await expect(page.getByTestId('export-status')).toHaveCount(0);

    // 回復フェーズ: 成功する refresh → 1回再送 → 200 を同じケース内で検証する。
    refreshSucceeds = true;
    await page.getByTestId('export-my-data').click();

    await expect(page.getByTestId('export-error')).toHaveCount(0);
    await expect(page.getByTestId('export-status')).toContainText('エクスポート完了');
    expect(refreshAttempts).toBeGreaterThanOrEqual(1);
    expect(exportAuthHeaders).toContain('Bearer refreshed_jwt_token');

    // R2-REFRESH-SEC-1: refresh 失敗で REFRESHED_JWT がクリアされる回帰。
    // 成功フェーズの新 JWT を失効させ、refresh を失敗側へ戻す。
    refreshedTokenRevoked = true;
    refreshSucceeds = false;
    await page.getByTestId('export-my-data').click();
    await expect(page.getByTestId('export-error')).toHaveText('ログインが必要です', { timeout: 10000 });
    // クリア前は保持中の新 JWT が送られる（元の 401 は呼び出し側へ返る）。
    expect(exportAuthHeaders[exportAuthHeaders.length - 1]).toBe('Bearer refreshed_jwt_token');

    // 次の試行では REFRESHED_JWT が消え、ログイン時のトークンへ戻る。
    await page.getByTestId('export-my-data').click();
    await expect(page.getByTestId('export-error')).toHaveText('ログインが必要です', { timeout: 10000 });
    expect(exportAuthHeaders[exportAuthHeaders.length - 1]).toBe('Bearer mock_jwt_token');

    // R2-REFRESH-SEC-2: user_hash 不一致の refresh 応答から得た JWT は採用しない。
    const beforeMismatch = exportAuthHeaders.length;
    refreshSucceeds = true;
    refreshUserHash = 'other_user_hash';
    await page.getByTestId('export-my-data').click();
    await expect(page.getByTestId('export-error')).toHaveText('ログインが必要です', { timeout: 10000 });
    expect(exportAuthHeaders.slice(beforeMismatch)).not.toContain('Bearer refreshed_jwt_token');
    expect(exportAuthHeaders[exportAuthHeaders.length - 1]).toBe('Bearer mock_jwt_token');
  });

  test('R5-04: click が例外を投げた場合は固定文言のエラーを表示し、完了文言を出さないこと', async ({ page }) => {
    await page.route('**/me/export', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(exportFixture),
      });
    });

    await loginAndOpenDashboard(page);

    // ネットワーク応答は成功のまま、ダウンロード開始だけを失敗させる。
    await page.evaluate(() => {
      HTMLAnchorElement.prototype.click = () => {
        throw new Error('E2E: download blocked');
      };
    });

    await page.getByTestId('export-my-data').click();

    await expect(page.getByTestId('export-error')).toHaveText(
      'ダウンロードの開始に失敗しました。',
      { timeout: 10000 }
    );
    await expect(page.getByTestId('export-error')).toHaveAttribute('role', 'alert');
    await expect(page.getByTestId('export-status')).toHaveCount(0);
    // R2-CLIENT-07: 失敗時は既存固定文言に加えて dedicated な再試行案内を出す。
    await expect(page.getByTestId('export-download-hint')).toContainText('再度お試しください');
  });
});
