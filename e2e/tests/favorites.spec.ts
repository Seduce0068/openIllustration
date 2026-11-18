import { test, expect, type Page, type Route } from '@playwright/test';
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
        cid: 'QmUoLUYcf44HS1LteR8Uuh98vPDDNXjyZXht8og27yNaxB',
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
        cid: 'QmexSeEPM8WkRztX1j1bj4bnBf497ghku4Bmzqq3yuDTiP',
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
      const body = item.cid === 'QmUoLUYcf44HS1LteR8Uuh98vPDDNXjyZXht8og27yNaxB' ? novelBody : plainBody;
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

    // ☆ → POST が呼ばれ、成功後に一覧が「1回だけ」再取得される。
    // 回数を固定し、かつリクエスト内容も完全一致で検証して過剰再フェッチを検出する。
    await card('Plain Story').getByRole('button', { name: 'Add to favorites' }).click();
    await expect.poll(() => favoriteCalls).toEqual([
      { method: 'POST', path: '/content/QmexSeEPM8WkRztX1j1bj4bnBf497ghku4Bmzqq3yuDTiP/favorite' },
    ]);
    await expect(
      card('Plain Story').getByRole('button', { name: 'Remove from favorites' }),
    ).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(() => contentRequests).toEqual([
      { q: null, tag: null, type: null, favorite_only: null },
      { q: null, tag: null, type: null, favorite_only: null },
    ]);

    // ★ → DELETE が呼ばれ、ボタン状態が戻る。
    await card('Fav Story').getByRole('button', { name: 'Remove from favorites' }).click();
    await expect.poll(() => favoriteCalls).toEqual([
      { method: 'POST', path: '/content/QmexSeEPM8WkRztX1j1bj4bnBf497ghku4Bmzqq3yuDTiP/favorite' },
      { method: 'DELETE', path: '/content/QmUoLUYcf44HS1LteR8Uuh98vPDDNXjyZXht8og27yNaxB/favorite' },
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
    // デバウンス後の検索リクエストが飛ぶのを待ってから解除する。
    // 待たないと解除側の即時 GET にまとまり、リクエスト数が実行タイミング依存になる。
    await expect.poll(() => contentRequests).toHaveLength(6);
    await page.getByLabel('Favorites only').uncheck();
    await expect(page.getByText('Fav Story')).toBeVisible();
    await card('Fav Story').getByRole('button', { name: 'Add to favorites' }).focus();
    await page.keyboard.press('Enter');
    await expect.poll(() => favoriteCalls).toEqual([
      { method: 'POST', path: '/content/QmexSeEPM8WkRztX1j1bj4bnBf497ghku4Bmzqq3yuDTiP/favorite' },
      { method: 'DELETE', path: '/content/QmUoLUYcf44HS1LteR8Uuh98vPDDNXjyZXht8og27yNaxB/favorite' },
      { method: 'POST', path: '/content/QmUoLUYcf44HS1LteR8Uuh98vPDDNXjyZXht8og27yNaxB/favorite' },
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

    // 一連の操作で飛んだ一覧リクエストはちょうど8件だけ（失敗トグルでは再取得しない）。
    // 配列を完全一致で検証し、想定外の再フェッチ・クエリ混入を検出する。
    // 検索語クリア→デバウンス取得→お気に入り解除→Enter トグルの順で
    // 1操作1回の GET になるよう、途中の取得を明示的に待っている。
    await expect.poll(() => contentRequests).toEqual([
      { q: null, tag: null, type: null, favorite_only: null },
      { q: null, tag: null, type: null, favorite_only: null },
      { q: null, tag: null, type: null, favorite_only: null },
      { q: null, tag: null, type: null, favorite_only: '1' },
      { q: 'Plain', tag: null, type: null, favorite_only: '1' },
      { q: null, tag: null, type: null, favorite_only: '1' },
      { q: null, tag: null, type: null, favorite_only: null },
      { q: null, tag: null, type: null, favorite_only: null },
    ]);
  });

  // 追加ケースは上記のリクエスト回数完全一致アサーション（2回/8回）を壊さないよう
  // 独立した test にし、モックも各 test 内で完結させる。
  const GUARD_CID = 'QmYwAPJzv5CZsnAztHZk3RQ3m2YgFf9pRnvNsYQ7nBTeS';
  type FavoriteCall = { method: string; path: string };
  type FavoriteFixture = { isFavorite: boolean };

  async function setupFavoriteGuardPage(
    page: Page,
    handleFavorite: (route: Route, fixture: FavoriteFixture) => Promise<void>,
  ) {
    const body = 'favorite guard body';
    const item = {
      cid: GUARD_CID,
      checksum: computeHash(body),
      type: 'Novel',
      status: 'Approved',
      price: 0,
      title: 'Guard Story',
      description: 'Favorite guard fixture',
      tags: ['guard'],
      is_favorite: false,
    };
    const fixture: FavoriteFixture = { isFavorite: false };
    const favoriteCalls: FavoriteCall[] = [];
    const contentRequests: string[] = [];

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
      contentRequests.push(route.request().url());
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{ ...item, is_favorite: fixture.isFavorite }]),
      });
    });
    await page.route(
      url => url.pathname.startsWith('/content/') && url.pathname.endsWith('/favorite'),
      async route => {
        favoriteCalls.push({
          method: route.request().method(),
          path: new URL(route.request().url()).pathname,
        });
        await handleFavorite(route, fixture);
      },
    );
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

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
    await expect(page.getByText('Guard Story')).toBeVisible({ timeout: 20000 });

    const card = page.locator('div[data-testid="content-card"]').filter({ hasText: 'Guard Story' });
    const toggle = card.getByTestId('favorite-toggle');
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');
    return { favoriteCalls, contentRequests, toggle, card };
  }

  test('double click sends only one toggle request', async ({ page }) => {
    let releaseResponse = (): void => {};
    const responseGate = new Promise<void>(resolve => {
      releaseResponse = resolve;
    });
    const { favoriteCalls, toggle } = await setupFavoriteGuardPage(page, async (route, fixture) => {
      // レスポンスを保留し、送信中ガードが効いている間は busy 状態を維持する。
      await responseGate;
      fixture.isFavorite = route.request().method() === 'POST';
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ is_favorite: fixture.isFavorite }),
      });
    });

    // 予告なしの高速2クリック（同一タスクで2回 dispatch）。
    await toggle.evaluate((el: HTMLElement) => {
      el.click();
      el.click();
    });
    await expect.poll(() => favoriteCalls.length).toBe(1);

    // 少し gap を空けて2回目。レスポンス保留中は disabled で送られない。
    await page.waitForTimeout(100);
    await toggle.evaluate((el: HTMLElement) => el.click());
    await expect(toggle).toBeDisabled();
    await expect.poll(() => favoriteCalls.length).toBe(1);

    // 完了後も再送されず、ガードが解除されて次の操作を受け付ける。
    releaseResponse();
    await expect(toggle).toBeEnabled();
    await expect(toggle).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(() => favoriteCalls.length).toBe(1);
  });

  test('401 shows re-login guidance in favorite-error', async ({ page }) => {
    const { favoriteCalls, contentRequests, toggle, card } =
      await setupFavoriteGuardPage(page, async route => {
        await route.fulfill({
          status: 401,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'unauthorized' }),
        });
      });

    await toggle.click();
    const error = card.getByTestId('favorite-error');
    await expect(error).toHaveAttribute('role', 'alert');
    await expect(error).toContainText('再ログインが必要です。');
    await expect.poll(() => favoriteCalls).toEqual([
      { method: 'POST', path: `/content/${GUARD_CID}/favorite` },
    ]);
    // 失敗時は再取得せず、ガードも解除される。
    await expect.poll(() => contentRequests.length).toBe(1);
    await expect(toggle).toBeEnabled();
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');
  });
});
