import { test, expect, type Page } from '@playwright/test';
import * as crypto from 'crypto';

/**
 * MET-01: 中立化URL件数指標のクライアント送信を検証する。
 *
 * API側のバリデーション（event のホワイトリスト、count の範囲、サイズ上限、
 * レート制限、カウンタの差分）はサーバー側の単体テストへ委譲する。
 * 本シナリオは「危険 URL を含む Markdown を描画したクライアントが、
 * 最小ペイロードを1回だけ送る」ことに集中する。
 */
test.describe('Neutralized URL metrics (MET-01)', () => {
  const CID = 'QmUoLUYcf44HS1LteR8Uuh98vPDDNXjyZXht8og27yNaxB';
  // 危険 URL が2件（javascript スキームのリンクと data スキームの画像）。
  const novelBody = '[click](javascript:alert(1)) ![img](data:text/html,alert)';
  const checksum = crypto.createHash('sha256').update(novelBody).digest('hex');

  type CapturedCall = {
    method: string;
    body: string;
    headers: Record<string, string | undefined>;
  };

  test('MET-01: neutralized URL count is posted once as a minimal client event', async ({ page }, testInfo) => {
    const startedAt = Date.now();
    const captured: CapturedCall[] = [];

    // page.route は goto / ログインより前に登録する。
    await page.route('**/auth/login', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ nonce: 'test-nonce' }),
      });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          authorized: true,
          user_hash: 'mock_user_hash',
          token: 'mock_jwt_token',
        }),
      });
    });
    await page.route(url => url.pathname === '/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([
          {
            cid: CID,
            checksum,
            type: 'Novel',
            status: 'Approved',
            price: 0,
            title: 'Neutralized Story',
            description: 'Dangerous URLs are neutralized on render',
            tags: ['metrics'],
            is_favorite: false,
          },
        ]),
      });
    });
    await page.route(url => url.pathname === `/content/${CID}`, async route => {
      await route.fulfill({ status: 200, contentType: 'text/markdown', body: novelBody });
    });
    await page.route(url => url.pathname === `/content/${CID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });

    await page.route('**/metrics/client-events', async route => {
      const request = route.request();
      const origin = request.headers()['origin'];
      // クロスオリジンの JSON POST はプリフライトが飛ぶため CORS を明示する。
      const corsHeaders: Record<string, string> = {
        'Access-Control-Allow-Origin': origin ?? 'http://127.0.0.1:8080',
        'Access-Control-Allow-Methods': 'POST, OPTIONS',
        'Access-Control-Allow-Headers': 'content-type',
      };
      if (request.method() === 'OPTIONS') {
        await route.fulfill({ status: 204, headers: corsHeaders });
        return;
      }
      captured.push({
        method: request.method(),
        body: request.postData() ?? '',
        headers: request.headers(),
      });
      // 成功は本文なしの 204。API 側の応答検証はサーバーの単体テストが担当する。
      await route.fulfill({ status: 204, headers: corsHeaders, body: '' });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();

    // 検証チェックサム一致後に Markdown が描画される（integrity.spec.ts と同じ待ち方）。
    await expect(page.locator('span', { hasText: '● VERIFIED' })).toBeVisible({ timeout: 30000 });
    await expect(page.locator('a[href="#"]')).toHaveCount(1);
    await expect(page.locator('img[src="#"]')).toHaveCount(1);

    // spawn_local の送信は描画直後ではなく少し遅れる可能性があるため poll で待つ。
    await expect
      .poll(() => captured.map(call => JSON.parse(call.body) as unknown), {
        intervals: [100, 100, 100, 100, 100],
        timeout: 5000,
      })
      .toEqual([{ event: 'neutralized_url', count: 2 }]);

    const call = captured[0];
    expect(call.method).toBe('POST');
    // event と count 以外のキーが無いことを生 JSON でも確認する。
    expect(call.body).not.toContain('javascript:alert(1)');
    expect(call.body).not.toContain('data:text/html,alert');
    expect(call.body).not.toContain('Neutralized Story');
    expect(Object.keys(JSON.parse(call.body) as object).sort()).toEqual(['count', 'event']);
    // 認証情報や IP をペイロードへ含めない（ブラウザが付与する User-Agent ヘッダは対象外）。
    expect(call.headers['authorization']).toBeUndefined();
    expect(call.headers['x-forwarded-for']).toBeUndefined();
    // 監視送信に Cookie（refresh_token 等）を一切載せない。
    expect(call.headers['cookie']).toBeUndefined();
    await expect(page.getByRole('alert')).toHaveCount(0);

    // PERF-01 Level 0: 観測値を保存するだけ。大小比較・閾値判定は行わない（ADR-0011 合意待ち）。
    await test.step('PERF-01: record paint/navigation timing observation', async () => {
      const timing = await page.evaluate(() => {
        const nav = performance.getEntriesByType('navigation')[0] as
          | PerformanceNavigationTiming
          | undefined;
        const paints = performance.getEntriesByType('paint');
        const fcp = paints.find(entry => entry.name === 'first-contentful-paint');
        return {
          firstContentfulPaint: fcp ? fcp.startTime : null,
          responseEnd: nav ? nav.responseEnd : null,
          domContentLoadedEventEnd: nav ? nav.domContentLoadedEventEnd : null,
        };
      });
      const observation = { ...timing, elapsedMs: Date.now() - startedAt };
      for (const value of Object.values(observation)) {
        if (value === null) continue;
        expect(Number.isFinite(value)).toBe(true);
        expect(value).toBeGreaterThanOrEqual(0);
      }
      await testInfo.attach('perf-observation', {
        body: JSON.stringify(observation),
        contentType: 'application/json',
      });
    });
  });

  /**
   * NEUT-URL-02 / NEUT-URL-03 共通のセットアップ。
   * 与えた本文を checksum 付きの Novel として配信し、/metrics/client-events の
   * POST（と CORS プリフライトの OPTIONS）を捕まえる。既存の MET-01 は変更しない。
   */
  async function setupNeutralizedMetricsPage(
    page: Page,
    body: string,
  ): Promise<{ captured: CapturedCall[]; contentBodyRequests: number[] }> {
    const captured: CapturedCall[] = [];
    const contentBodyRequests: number[] = [];
    const bodyChecksum = crypto.createHash('sha256').update(body).digest('hex');

    await page.route('**/auth/login', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ nonce: 'test-nonce' }),
      });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          authorized: true,
          user_hash: 'mock_user_hash',
          token: 'mock_jwt_token',
        }),
      });
    });
    await page.route(url => url.pathname === '/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([
          {
            cid: CID,
            checksum: bodyChecksum,
            type: 'Novel',
            status: 'Approved',
            price: 0,
            title: 'Neutralized Story',
            description: 'Dangerous URLs are neutralized on render',
            tags: ['metrics'],
            is_favorite: false,
          },
        ]),
      });
    });
    await page.route(url => url.pathname === `/content/${CID}`, async route => {
      contentBodyRequests.push(Date.now());
      await route.fulfill({ status: 200, contentType: 'text/markdown', body });
    });
    await page.route(url => url.pathname === `/content/${CID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });
    await page.route('**/metrics/client-events', async route => {
      const request = route.request();
      const origin = request.headers()['origin'];
      const corsHeaders: Record<string, string> = {
        'Access-Control-Allow-Origin': origin ?? 'http://127.0.0.1:8080',
        'Access-Control-Allow-Methods': 'POST, OPTIONS',
        'Access-Control-Allow-Headers': 'content-type',
      };
      if (request.method() === 'OPTIONS') {
        await route.fulfill({ status: 204, headers: corsHeaders });
        return;
      }
      captured.push({
        method: request.method(),
        body: request.postData() ?? '',
        headers: request.headers(),
      });
      await route.fulfill({ status: 204, headers: corsHeaders, body: '' });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.locator('span', { hasText: '● VERIFIED' })).toBeVisible({ timeout: 30000 });
    return { captured, contentBodyRequests };
  }

  test('NEUT-URL-02: protocol-relative URL は # に中立化され件数が送信される', async ({ page }) => {
    // network-path reference(//host) と Windows UNC(\\host) はスキームが無いため
    // 従来は安全とみなされていたが、ブラウザが現在のスキームを補完して
    // 外部ホストへ誘導し得るため拒否される（is_safe_markdown_url）。
    // 中立化された link/image の href/src は "#" になる（:462-486）。
    const protocolRelativeBody = '[phish](//evil.example/phish) ![img](//evil.example/img.png)';
    const { captured } = await setupNeutralizedMetricsPage(page, protocolRelativeBody);

    await expect(page.locator('a[href="#"]')).toHaveCount(1);
    await expect(page.locator('img[src="#"]')).toHaveCount(1);
    await expect(page.locator('a[href^="//"]')).toHaveCount(0);
    await expect(page.locator('img[src^="//"]')).toHaveCount(0);

    await expect
      .poll(() => captured.map(call => JSON.parse(call.body) as unknown), {
        intervals: [100, 100, 100, 100, 100],
        timeout: 5000,
      })
      .toEqual([{ event: 'neutralized_url', count: 2 }]);
  });

  test('NEUT-URL-03: 同一本文の再描画では client event を再送しない', async ({ page }) => {
    const body = '[click](javascript:alert(1)) ![img](data:text/html,alert)';
    const { captured, contentBodyRequests } = await setupNeutralizedMetricsPage(page, body);

    await expect
      .poll(() => captured.map(call => JSON.parse(call.body) as unknown))
      .toEqual([{ event: 'neutralized_url', count: 2 }]);
    expect(contentBodyRequests.length).toBe(1);

    // BROWSE → UPLOAD → BROWSE で ContentViewer を再マウントさせ、同じ本文を再描画する。
    await page.getByRole('link', { name: 'UPLOAD' }).click();
    // ビュー切り替えが完了した肯定アサーションを先に置き、否定は poll で待つ。
    await expect(page.locator('#upload-price')).toBeVisible();
    await expect
      .poll(() => page.getByText('Neutralized Story').count(), { timeout: 5000 })
      .toBe(0);
    await page.getByRole('link', { name: 'BROWSE' }).click();
    await expect(page.locator('span', { hasText: '● VERIFIED' })).toBeVisible({ timeout: 30000 });
    await expect.poll(() => contentBodyRequests.length, { timeout: 10000 }).toBe(2);

    // 同一描画ソースは thread_local の REPORTED で除外され POST は1回のまま
    // （thread_local の REPORTED 集合）。
    // 再マウント直後に最初の POST を捕まえた時点では captured.length === 1 が
    // 既に真で、重複 POST がその後に飛んでも検出できない。観測ウィンドウ中に
    // captured が変化しない（ネットワークが静穏な）状態が続くまで待ってから検証する。
    const QUIET_WINDOW_MS = 500;
    let observedLength = -1;
    let lastChangeAt = Date.now();
    await expect
      .poll(
        () => {
          const now = Date.now();
          if (captured.length !== observedLength) {
            observedLength = captured.length;
            lastChangeAt = now;
          }
          return observedLength === 1 && now - lastChangeAt >= QUIET_WINDOW_MS
            ? 'stable'
            : `waiting: captured=${observedLength}, quiet=${now - lastChangeAt}ms`;
        },
        { intervals: [100], timeout: 10000 },
      )
      .toBe('stable');
    expect(captured.map(call => JSON.parse(call.body) as unknown)).toEqual([
      { event: 'neutralized_url', count: 2 },
    ]);
  });
});
