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
    // 見出しを含むことで TOC アンカーが描画され、未知 hash の遷移を検証できる。
    const novelBody = '# Chapter One\n\nnovel body';
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
    // R2-URL: 重複 q と未知キーを含む URL を注入し、正規化書き出しでも GET が増えないことを検証する。
    await page.evaluate(() => {
      history.replaceState(null, '', '/?q=Neon&q=Ghost&utm_source=test#dashboard');
    });
    await page.getByLabel('Search').pressSequentially('Neon', { delay: 20 });

    await expect.poll(() => seenQueries).toEqual([null, 'Neon']);
    await expect(page.getByText('Neon Alley')).toBeVisible();
    await expect(page.getByText('Moonlit Harbor')).toHaveCount(0);

    // R2-URL: 検索条件が URL クエリへ同期される（view は hash に残る）。
    await expect.poll(() => new URL(page.url()).searchParams.get('q')).toBe('Neon');
    expect(new URL(page.url()).hash).toBe('#dashboard');

    // R2-URL: 重複キー・未知キーは除去され、?q=Neon と #dashboard へ冪等に収束する。
    // 正規化は replaceState なので、seenQueries は [null, 'Neon'] のまま（GET は増えない）。
    await expect.poll(() => new URL(page.url()).search).toBe('?q=Neon');
    expect(new URL(page.url()).hash).toBe('#dashboard');
    await expect.poll(() => seenQueries).toEqual([null, 'Neon']);

    // 再読込 -> 未認証なのでログインへ戻る -> 再ログインで URL から条件が復元される。
    await page.reload();
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
    await expect(page.getByLabel('Search')).toHaveValue('Neon');
    // 復元時の二重 GET が無く、末尾 2 件が [Neon, Neon] になること。
    await expect.poll(() => seenQueries.slice(-2)).toEqual(['Neon', 'Neon']);

    // 条件付き空状態: q='Ghost' は0件なので専用文言とクリアボタンを表示する（既存ケース内へ統合）。
    // 条件なしの空状態文言（'承認済みコンテンツはありません。'）は据え置き。
    await page.getByLabel('Search').fill('Ghost');
    await expect.poll(() => seenQueries.slice(-1)).toEqual(['Ghost']);
    await expect(page.getByText('条件に一致するコンテンツはありません。')).toBeVisible();
    const clearFilters = page.getByTestId('clear-search-filters');
    await expect(clearFilters).toBeVisible();

    // クリアで条件が消え、一覧が復帰する。
    await clearFilters.click();
    await expect(page.getByText('Moonlit Harbor')).toBeVisible();
    await expect(page.getByText('Neon Alley')).toBeVisible();
    await expect(clearFilters).toHaveCount(0);
    await expect.poll(() => new URL(page.url()).search).toBe('');

    // R2-CLIENT-04: 小説 → TOC → タブ切替 → goBack。
    // TOC の #slug は既知 view ではないため、popstate で直前の既知 view
    // （ここでは dashboard）へ正規化されることを検証する。
    await page.getByText('Table of Contents').click();
    const tocAnchor = page.locator('details nav a').first();
    await expect(tocAnchor).toBeVisible();
    await tocAnchor.click();
    await expect.poll(() => new URL(page.url()).hash).not.toBe('#dashboard');
    await expect(page.getByRole('button', { name: 'BROWSE' })).toHaveAttribute('aria-current', 'page');

    await page.getByRole('button', { name: 'UPLOAD' }).click();
    await expect(page.getByRole('button', { name: 'UPLOAD' })).toHaveAttribute('aria-current', 'page');

    await page.goBack();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toHaveAttribute('aria-current', 'page');
  });
});
