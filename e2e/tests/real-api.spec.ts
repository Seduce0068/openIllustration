import { test, expect, type APIRequestContext, type Page } from '@playwright/test';

/**
 * API をモックせず実サーバーへ送信するスモークテスト。
 * R1-09: クライアントが file パートへ MIME を設定しないと実サーバーは 400 を返す。
 * C1: 主要画面の実サーバー認証も同時にカバーする。
 * R1-02: 検索 / お気に入り / エクスポートの実サーバー結合も追加する。
 *
 * ここでは page.route を一切使わない。認証は既存テストと同じく
 * 「CONNECT WITH JPKI」ボタンから実サーバーの /auth/login → /auth/verify
 * （ALLOW_MOCK_AUTH=1）を通し、取得した JWT を API 呼び出しへ再利用する。
 */
test.describe('Real server integration', () => {
  test('uploads JXL through the real API without mocks', async ({ page }) => {
    await page.goto('/');

    // 実サーバーの /auth/login + /auth/verify を通る（ルートをモックしない）。
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('link', { name: 'UPLOAD' })).toBeVisible({ timeout: 10000 });

    await page.getByRole('link', { name: 'UPLOAD' }).click();

    // CID の一意性のため毎回異なるバイト列を使う（FF 0A = JXL raw codestream）。
    const body = Buffer.concat([
      Buffer.from([0xff, 0x0a]),
      Buffer.from(`real-api smoke ${Date.now()}`),
    ]);
    await page.locator('input[type="file"]').setInputFiles({
      name: 'smoke.jxl',
      mimeType: 'image/jxl',
      buffer: body,
    });

    await page.getByRole('button', { name: /Sign & Submit/i }).click();

    await expect(page.locator('text=SUCCESS: Content registered')).toBeVisible({ timeout: 15000 });
  });

  // クライアント (client/src/lib.rs の API_BASE) と同じ実サーバー宛先。
  // E2E_API_BASE で直接、または E2E_SERVER_PORT でポートのみ上書きできる。
  // 未設定時は従来どおり http://127.0.0.1:3000（playwright.config.ts と同契約）。
  const apiBaseFromEnv = process.env.E2E_API_BASE || '';
  const serverPortFromEnv = process.env.E2E_SERVER_PORT || '';
  const API_BASE =
    apiBaseFromEnv || `http://127.0.0.1:${serverPortFromEnv || '3000'}`;
  // どの環境へ向かっているか（帰属）をテスト開始時に可視化する。
  console.log(
    `[real-api] API_BASE=${API_BASE} (source: ${
      apiBaseFromEnv
        ? 'E2E_API_BASE'
        : serverPortFromEnv
          ? 'E2E_SERVER_PORT'
          : 'default 127.0.0.1:3000'
    })`,
  );

  type RealSession = {
    /** 実 /auth/verify が発行した JWT。 */
    token: string;
    /** JWT の sub (= user_hash)。upload の owner_hash に必要。 */
    userHash: string;
  };

  type ContentMeta = {
    cid: string;
    title: string;
    description: string;
    tags: string[];
    is_favorite: boolean;
  };

  /** 衝突を避けるための固有値。英字を含めない（export の "sid" 検査と混同しない）。 */
  const uniqueValue = () =>
    `${Date.now()}${Math.floor(Math.random() * 1_000_000_000).toString().padStart(9, '0')}`;

  /**
   * 既存の JXL アップロードテストと同じブラウザ認証フローを再利用する。
   * 実 /auth/login → /auth/verify のレスポンスを待ち受け、モックせずに
   * JWT と user_hash を取り出して以降の API 呼び出しへ渡す。
   */
  async function loginOnRealServer(page: Page): Promise<RealSession> {
    await page.goto('/');
    const verifyResponse = page.waitForResponse(
      response =>
        new URL(response.url()).pathname === '/auth/verify' &&
        response.request().method() === 'POST',
    );
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    const response = await verifyResponse;

    expect(response.status()).toBe(200);
    const payload = await response.json();
    expect(payload.authorized).toBe(true);
    expect(typeof payload.token).toBe('string');
    expect(typeof payload.user_hash).toBe('string');
    // ブラウザ側も認証済み状態になっていることを確認する（既存テストと同じ遷移）。
    await expect(page.getByRole('link', { name: 'UPLOAD' })).toBeVisible({ timeout: 10000 });

    return { token: payload.token as string, userHash: payload.user_hash as string };
  }

  const authHeaders = (session: RealSession) => ({
    Authorization: `Bearer ${session.token}`,
  });

  /**
   * 実サーバーへ JXL を 1 件アップロードし、発行された CID を返す。
   * signature は ALLOW_MOCK_AUTH が許可する全ゼロ署名（クライアントと同じ）。
   */
  async function uploadJxl(
    request: APIRequestContext,
    session: RealSession,
    metadata: { title: string; description: string; tags: string[] },
  ): Promise<string> {
    const body = Buffer.concat([
      Buffer.from([0xff, 0x0a]), // JXL raw codestream
      Buffer.from(`real-api integration ${Date.now()} ${Math.random()}`),
    ]);
    const response = await request.post(`${API_BASE}/content/upload`, {
      headers: authHeaders(session),
      multipart: {
        file: { name: 'smoke.jxl', mimeType: 'image/jxl', buffer: body },
        owner_hash: session.userHash,
        price: '0',
        title: metadata.title,
        description: metadata.description,
        tags: metadata.tags.join(','),
        signature: {
          name: 'signature.bin',
          mimeType: 'application/octet-stream',
          buffer: Buffer.alloc(256),
        },
      },
    });
    const text = await response.text();
    expect(response.status(), `upload failed: ${text}`).toBe(200);
    const cid = JSON.parse(text) as string;
    expect(typeof cid).toBe('string');
    expect(cid.length).toBeGreaterThan(0);
    return cid;
  }

  /** 実サーバーの GET /content へ認証付きでアクセスする（モックなし）。 */
  async function listContents(
    request: APIRequestContext,
    session: RealSession,
    params: URLSearchParams,
  ): Promise<ContentMeta[]> {
    const response = await request.get(`${API_BASE}/content?${params.toString()}`, {
      headers: authHeaders(session),
    });
    const text = await response.text();
    expect(response.status(), `list failed: ${text}`).toBe(200);
    return JSON.parse(text) as ContentMeta[];
  }

  const cids = (items: ContentMeta[]) => items.map(item => item.cid);

  test('searches an uploaded content by q on the real server', async ({ page, request }) => {
    const session = await loginOnRealServer(page);

    const marker = `realapi-search-${uniqueValue()}`;
    const title = `Search ${marker}`;
    const description = `Description for ${marker}`;
    const tags = ['real-api', `tag-${marker}`];
    const cid = await uploadJxl(request, session, { title, description, tags });

    // 固有語で絞り込むと、アップロードした 1 件だけが返る。
    const filtered = await listContents(request, session, new URLSearchParams({ q: marker }));
    expect(filtered).toHaveLength(1);
    expect(filtered[0]).toMatchObject({ cid, title, description });
    expect(filtered[0].tags).toEqual(tags);

    // 未知の固有語では 0 件（q が実サーバー側で適用されている証拠）。
    const noMatch = await listContents(
      request,
      session,
      new URLSearchParams({ q: `${marker}absent` }),
    );
    expect(noMatch).toHaveLength(0);

    // 条件なしの一覧には確実に含まれる。
    expect(cids(await listContents(request, session, new URLSearchParams()))).toContain(cid);
  });

  test('favorites an uploaded content on the real server and removes it', async ({
    page,
    request,
  }) => {
    const session = await loginOnRealServer(page);

    const marker = `realapi-favorite-${uniqueValue()}`;
    const cid = await uploadJxl(request, session, {
      title: `Favorite ${marker}`,
      description: `Favorite target ${marker}`,
      tags: ['real-api', 'favorite'],
    });

    const favoriteOnly = new URLSearchParams({ favorite_only: '1' });

    // 追加前は favorite_only=1 に現れない。
    expect(cids(await listContents(request, session, favoriteOnly))).not.toContain(cid);

    // 実サーバーへ POST /content/{cid}/favorite。
    const added = await request.post(`${API_BASE}/content/${cid}/favorite`, {
      headers: authHeaders(session),
    });
    expect(added.status()).toBe(200);
    expect(await added.json()).toEqual({ is_favorite: true });

    // 追加後は favorite_only=1 に現れ、is_favorite が true になる。
    const favorites = await listContents(request, session, favoriteOnly);
    expect(cids(favorites)).toContain(cid);
    expect(favorites.find(item => item.cid === cid)?.is_favorite).toBe(true);

    // 実サーバーへ DELETE /content/{cid}/favorite。
    const removed = await request.delete(`${API_BASE}/content/${cid}/favorite`, {
      headers: authHeaders(session),
    });
    expect(removed.status()).toBe(200);
    expect(await removed.json()).toEqual({ is_favorite: false });

    // 削除後は favorite_only=1 から消え、通常の一覧には残る。
    expect(cids(await listContents(request, session, favoriteOnly))).not.toContain(cid);
    expect(cids(await listContents(request, session, new URLSearchParams()))).toContain(cid);
  });

  test('GET /me/export returns the real 15-field payload without session secrets', async ({
    page,
    request,
  }) => {
    const session = await loginOnRealServer(page);

    const response = await request.get(`${API_BASE}/me/export`, {
      headers: authHeaders(session),
    });
    expect(response.status()).toBe(200);
    const text = await response.text();
    const payload = JSON.parse(text) as Record<string, unknown>;

    // version / generated_at / sub + 12 配列 = 15 項目。
    const arrayKeys = [
      'users',
      'contents',
      'favorites',
      'transactions',
      'signatures',
      'content_audit_log',
      'reports',
      'votes',
      'payment_orders',
      'governance_audit_log',
      'refresh_tokens',
      'revoked_subs',
    ];
    expect(Object.keys(payload).sort()).toEqual(
      ['version', 'generated_at', 'sub', ...arrayKeys].sort(),
    );
    expect(Object.keys(payload)).toHaveLength(15);
    expect(payload.version).toBe(1);
    expect(typeof payload.generated_at).toBe('string');
    expect(Number.isNaN(Date.parse(payload.generated_at as string))).toBe(false);
    expect(payload.sub).toBe(session.userHash);
    for (const key of arrayKeys) {
      expect(Array.isArray(payload[key]), `${key} must be an array`).toBe(true);
    }

    // セッション ID / リフレッシュトークン識別子はフィールド名として現れない。
    // CID 等のデータ値に "sid" という部分文字列が偶然含まれることは誤検知なので、
    // 生文字列の substring 検索ではなく、再帰的にキー名を検査する。
    const collectKeys = (value: unknown): string[] => {
      if (Array.isArray(value)) return value.flatMap(collectKeys);
      if (value !== null && typeof value === 'object') {
        return Object.entries(value as Record<string, unknown>).flatMap(([key, child]) => [
          key,
          ...collectKeys(child),
        ]);
      }
      return [];
    };
    const keys = collectKeys(payload);
    for (const secret of ['token_hash', 'sid', 'family_id']) {
      expect(keys, `"${secret}" must not leak as a field name`).not.toContain(secret);
    }

    // キー名だけでなく「値」の漏洩も検証する。ログインで得た既知のシークレット値
    // （JWT そのものと、デコードできればその sid クレーム）が、export のどの文字列値
    // およびキー名にも部分一致しないことを再帰的に確認する。固定文字列 "sid" で
    // 検索するのではなく既知値との比較なので、CID 等に "sid" が偶然含まれても誤検知しない。
    const knownSecrets: string[] = session.token.length > 0 ? [session.token] : [];
    try {
      const payloadSegment = session.token.split('.')[1];
      if (typeof payloadSegment === 'string') {
        const claims = JSON.parse(Buffer.from(payloadSegment, 'base64url').toString('utf8')) as {
          sid?: unknown;
        };
        if (typeof claims.sid === 'string' && claims.sid.length > 0) {
          knownSecrets.push(claims.sid);
        }
      }
    } catch {
      // sid を取り出せない環境でも token 自身での検証は続ける。
    }
    expect(knownSecrets.length, 'login must yield at least one known secret value').toBeGreaterThan(0);

    const collectStrings = (value: unknown): string[] => {
      if (typeof value === 'string') return [value];
      if (Array.isArray(value)) return value.flatMap(collectStrings);
      if (value !== null && typeof value === 'object') {
        // 既知シークレット値がキー名として現れるケースも検出するため、値だけでなく
        // Object.entries のキーも走査対象に含める。
        return Object.entries(value as Record<string, unknown>).flatMap(([key, child]) => [
          key,
          ...collectStrings(child),
        ]);
      }
      return [];
    };
    const stringValues = collectStrings(payload);
    for (const secret of knownSecrets) {
      for (const value of stringValues) {
        // シークレット値自体はメッセージに出さない（テストログでの二次漏洩を防ぐ）。
        expect(
          value.includes(secret),
          'known session secret must not appear in any export string value or key name',
        ).toBe(false);
      }
    }
  });
});
