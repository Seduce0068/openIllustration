import { test, expect } from '@playwright/test';
import * as crypto from 'crypto';

/**
 * R5-01: 一覧の検索・絞り込みとメタデータ表示。
 * `/content` は URL predicate でフックし、送信された q パラメータを検証する。
 */
test.describe('Content search and filtering (R5-01)', () => {
  const computeHash = (data: string) =>
    crypto.createHash('sha256').update(data).digest('hex');

  test('filters the list by q and displays content metadata', async ({ page }) => {
    const novelBody = 'novel body';
    const illustrationBody = 'illustration body';
    const novel = {
      cid: 'QmQX9pNudeUXaYgUxs2tYxXY5ZmWp3HnThR3Q8ZD7GK6Cx',
      checksum: computeHash(novelBody),
      type: 'Novel',
      status: 'Approved',
      price: 0,
      title: 'Moonlit Harbor',
      description: 'A quiet seaside story',
      tags: ['fantasy', 'night'],
      is_favorite: false,
    };
    const illustration = {
      cid: 'QmbgbnQRxbKUvPLW7Nx4PonKWZfiTRa2YvJchE6chYqigT',
      checksum: computeHash(illustrationBody),
      type: 'Illustration',
      status: 'Approved',
      price: 100,
      title: 'Neon Alley',
      description: 'Cyberpunk city sketch',
      tags: ['urban', 'neon'],
      is_favorite: false,
    };
    const seenQueries: Array<string | null> = [];

    // 認証のモック
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

    // クエリ付き URL も確実に捕捉するため、glob ではなく URL predicate を使う。
    await page.route(url => url.pathname === '/content', async route => {
      const q = new URL(route.request().url()).searchParams.get('q');
      seenQueries.push(q);
      const items = [novel, illustration].filter(
        item => !q || item.title.toLowerCase().includes(q.toLowerCase()),
      );
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(items),
      });
    });

    // 各アイテムの実体と監査ログ（カードの検証表示を安定させる）
    await page.route(`**/content/${novel.cid}`, async route => {
      await route.fulfill({ status: 200, contentType: 'text/markdown', body: novelBody });
    });
    await page.route(`**/content/${illustration.cid}`, async route => {
      await route.fulfill({ status: 200, contentType: 'image/jxl', body: illustrationBody });
    });
    for (const cid of [novel.cid, illustration.cid]) {
      await page.route(`**/content/${cid}/audit`, async route => {
        await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
      });
    }

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });

    // 初期表示では全件とメタデータが見える
    await expect(page.getByText('Moonlit Harbor')).toBeVisible({ timeout: 20000 });
    await expect(page.getByText('Neon Alley')).toBeVisible();
    await expect(page.getByText('Cyberpunk city sketch')).toBeVisible();
    await expect(page.getByText('#neon')).toBeVisible();

    // 1文字ずつ入力してもデバウンスで1回の GET にまとまること。
    // 初期表示の q=null と合わせ、q 付きリクエストは最終語 'Neon' の1件だけ。
    await page.getByLabel('Search').pressSequentially('Neon', { delay: 20 });

    await expect.poll(() => seenQueries).toEqual([null, 'Neon']);
    await expect(page.getByText('Neon Alley')).toBeVisible();
    await expect(page.getByText('Moonlit Harbor')).toHaveCount(0);

    // R2-URL: 検索条件が URL クエリへ同期される（view は hash に残る）。
    await expect.poll(() => new URL(page.url()).searchParams.get('q')).toBe('Neon');
    expect(new URL(page.url()).hash).toBe('#dashboard');

    // 再読込 -> 未認証なのでログインへ戻る -> 再ログインで URL から条件が復元される。
    await page.reload();
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
    await expect(page.getByLabel('Search')).toHaveValue('Neon');
    // 復元時の二重 GET が無く、末尾 2 件が [Neon, Neon] になること。
    await expect.poll(() => seenQueries.slice(-2)).toEqual(['Neon', 'Neon']);
  });
});
