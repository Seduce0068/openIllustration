import { test, expect } from '@playwright/test';
import * as crypto from 'crypto';

/**
 * R5-02: お気に入りトグル（POST/DELETE）と favorite_only 絞り込み。
 * 一覧は URL predicate でフックし、送信されたクエリとメソッドを検証する。
 */
test.describe('Favorites / Bookmarks (R5-02)', () => {
  const computeHash = (data: string) =>
    crypto.createHash('sha256').update(data).digest('hex');

  test('toggles favorites via POST/DELETE and filters with favorite_only=1', async ({ page }) => {
    const novelBody = 'favorite novel body';
    const plainBody = 'plain novel body';
    const items = [
      {
        cid: 'QmFavoriteNovel',
        checksum: computeHash(novelBody),
        type: 'Novel',
        status: 'Approved',
        price: 0,
        title: 'Fav Story',
        description: 'Initially favorited',
        tags: ['favorite'],
        is_favorite: true,
      },
      {
        cid: 'QmPlainNovel',
        checksum: computeHash(plainBody),
        type: 'Novel',
        status: 'Approved',
        price: 0,
        title: 'Plain Story',
        description: 'Not favorited yet',
        tags: ['plain'],
        is_favorite: false,
      },
    ];

    type ContentRequest = {
      q: string | null;
      tag: string | null;
      type: string | null;
      favorite_only: string | null;
    };
    const contentRequests: ContentRequest[] = [];
    const favoriteCalls: Array<{ method: string; path: string }> = [];
    let failNextToggle = false;

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

    // 一覧: クエリを記録し、favorite_only=1 のときはお気に入りのみ返す。
    await page.route(url => url.pathname === '/content', async route => {
      const params = new URL(route.request().url()).searchParams;
      const q = (params.get('q') ?? '').toLowerCase();
      const favoriteOnly = ['1', 'true'].includes(
        (params.get('favorite_only') ?? '').toLowerCase(),
      );
      contentRequests.push({
        q: params.get('q'),
        tag: params.get('tag'),
        type: params.get('type'),
        favorite_only: params.get('favorite_only'),
      });
      const visible = items.filter(
        item =>
          (!favoriteOnly || item.is_favorite) &&
          (!q || item.title.toLowerCase().includes(q)),
      );
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(visible),
      });
    });

    // トグル API: メソッドを記録して状態を反転する（成功時は冪等な JSON を返す）。
    await page.route(
      url => url.pathname.startsWith('/content/') && url.pathname.endsWith('/favorite'),
      async route => {
        const path = new URL(route.request().url()).pathname;
        const cid = path.split('/')[2] ?? '';
        const method = route.request().method();
        favoriteCalls.push({ method, path });
        if (failNextToggle) {
          failNextToggle = false;
          await route.fulfill({
            status: 500,
            contentType: 'application/json',
            body: JSON.stringify({ error: 'favorite update failed' }),
          });
          return;
        }
        const item = items.find(entry => entry.cid === cid);
        if (item) {
          item.is_favorite = method === 'POST';
        }
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ is_favorite: item?.is_favorite ?? false }),
        });
      },
    );

    // 各アイテムの実体と監査ログ（カードの検証表示を安定させる）。
    for (const item of items) {
      const body = item.cid === 'QmFavoriteNovel' ? novelBody : plainBody;
      await page.route(
        url => url.pathname === `/content/${item.cid}`,
        async route => {
          await route.fulfill({ status: 200, contentType: 'text/markdown', body });
        },
      );
      await page.route(
        url => url.pathname === `/content/${item.cid}/audit`,
        async route => {
          await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
        },
      );
    }

    // カードはタイトルで特定する（トグル後のラベル変化に依存しない）。
    const card = (title: string) =>
      page.locator('div.flex.flex-col.h-full').filter({ hasText: title });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });

    await expect(page.getByText('Fav Story')).toBeVisible({ timeout: 20000 });
    await expect(page.getByText('Plain Story')).toBeVisible();
    await expect(
      card('Fav Story').getByRole('button', { name: 'Remove from favorites' }),
    ).toHaveAttribute('aria-pressed', 'true');
    await expect(
      card('Plain Story').getByRole('button', { name: 'Add to favorites' }),
    ).toHaveAttribute('aria-pressed', 'false');

    // ☆ → POST が呼ばれ、成功後に一覧が再取得される。
    await card('Plain Story').getByRole('button', { name: 'Add to favorites' }).click();
    await expect.poll(() => favoriteCalls).toEqual([
      { method: 'POST', path: '/content/QmPlainNovel/favorite' },
    ]);
    await expect(
      card('Plain Story').getByRole('button', { name: 'Remove from favorites' }),
    ).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(() => contentRequests.length).toBeGreaterThanOrEqual(2);

    // ★ → DELETE が呼ばれ、ボタン状態が戻る。
    await card('Fav Story').getByRole('button', { name: 'Remove from favorites' }).click();
    await expect.poll(() => favoriteCalls).toEqual([
      { method: 'POST', path: '/content/QmPlainNovel/favorite' },
      { method: 'DELETE', path: '/content/QmFavoriteNovel/favorite' },
    ]);
    await expect(
      card('Fav Story').getByRole('button', { name: 'Add to favorites' }),
    ).toHaveAttribute('aria-pressed', 'false');

    // 「Favorites only」で favorite_only=1 が送られ、結果が絞り込まれる。
    await page.getByLabel('Favorites only').check();
    await expect.poll(() => contentRequests).toContainEqual({
      q: null,
      tag: null,
      type: null,
      favorite_only: '1',
    });
    await expect(page.getByText('Plain Story')).toBeVisible();
    await expect(page.getByText('Fav Story')).toHaveCount(0);

    // 既存の q と併用できること。
    await page.getByLabel('Search').fill('Plain');
    await expect.poll(() => contentRequests).toContainEqual({
      q: 'Plain',
      tag: null,
      type: null,
      favorite_only: '1',
    });
    await expect(page.getByText('Plain Story')).toBeVisible();

    // フィルタ解除後、キーボード（Enter）でもトグルできること。
    await page.getByLabel('Search').fill('');
    await page.getByLabel('Favorites only').uncheck();
    await expect(page.getByText('Fav Story')).toBeVisible();
    await card('Fav Story').getByRole('button', { name: 'Add to favorites' }).focus();
    await page.keyboard.press('Enter');
    await expect.poll(() => favoriteCalls).toEqual([
      { method: 'POST', path: '/content/QmPlainNovel/favorite' },
      { method: 'DELETE', path: '/content/QmFavoriteNovel/favorite' },
      { method: 'POST', path: '/content/QmFavoriteNovel/favorite' },
    ]);
    await expect(
      card('Fav Story').getByRole('button', { name: 'Remove from favorites' }),
    ).toHaveAttribute('aria-pressed', 'true');

    // API 失敗時はカード内にエラーが表示され、状態は変わらない。
    failNextToggle = true;
    await card('Fav Story').getByRole('button', { name: 'Remove from favorites' }).click();
    await expect(card('Fav Story').getByRole('alert')).toContainText(
      'Failed to update favorite',
    );
    await expect(
      card('Fav Story').getByRole('button', { name: 'Remove from favorites' }),
    ).toHaveAttribute('aria-pressed', 'true');
  });
});
