import { test, expect } from '@playwright/test';
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

  test('MET-01: neutralized URL count is posted once as a minimal client event', async ({ page }) => {
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
        timeout: 2000,
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
    await expect(page.getByRole('alert')).toHaveCount(0);
  });
});
