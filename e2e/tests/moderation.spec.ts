import { test, expect, type Page } from '@playwright/test';

test.describe('Moderation & Governance (GOV-01, GOV-02)', () => {
  const mockCID = 'QmVPyxX9eKoR9LaKp2Hgt9rVkAffptArJ3zW5hvPLE713h';

  // GET /mod/reports の reason はサーバー側 common::summarize_reason で
  // PII 除去 + 最大120文字に要約される。E2E では実サーバー結合を行わず、
  // 要約済み文字列をモックから返して UI 側の表示契約を検証する。
  const summarizedReason = '不審な投稿です [redacted]';
  const piiFixture = {
    email: 'reporter@example.com',
    phone: '090-1234-5678',
    url: 'https://phish.example.com/login',
  };
  const reportFixture = (reason: string) => ({
    report_id: 42,
    target_cid: 'QmRRTBnaNs7jwYiWadPrEDgrs9a199VZUD3ftVLEJ5DJw5',
    reason,
    created_at: '2026-11-18T00:00:00Z',
    status: 'Pending',
  });

  // client/src/lib.rs の VOTE_NOTICE / vote_error_message で確定した固定文言。
  // サーバーが返す本文や生のネットワークエラーは画面へ出さない契約を検証する。
  const voteNoticeText = '投票を記録しました。';
  const voteForbiddenText = '投票に失敗しました (HTTP 403).';
  const voteNetworkErrorText = 'Network error submitting vote.';

  // client/src/lib.rs の reports_error_message / REPORTS_PARSE_ERROR を grep して確定した
  // 固定文言。Vote と同様に、サーバー本文や生エラーは画面へ出さない契約を検証する。
  const reportsHttpErrorText = (status: number) => `投票一覧の取得に失敗しました (HTTP ${status}).`;
  const reportsNetworkErrorText = 'Network error loading reports.';

  // submit_report の失敗時の固定文言。
  // 成功・進捗は status シグナルへ入り <p role="status" class="text-xs text-gray-400">
  // （通報 status ライブリージョン）で描画される。失敗は dedicated な
  // <p role="alert" data-testid="report-failed"> で描画し、入力検証の
  // report-validation-error とは data-testid で区別する。
  const reportFailedText = 'Report failed.';
  const reportSuccessText = 'Report accepted. Content marked Reported.';

  // client/src/lib.rs の stats_error_message(StatsFailure::Parse) を grep して確定した
  // 固定文言。serde の生エラー（フィールド名・解析位置・型情報・レスポンス本文）は
  // 画面へ出さない契約を検証する。
  const statsParseErrorText = '集計の解析に失敗しました。';
  // client/src/lib.rs の 403 固定文言。一般画面へ漏れないことの否定オラクルとして使う。
  const statsForbiddenText = '運用者権限が必要です';

  // ADR-0008 段階2の公開集計 DTO。bans / unbans / unpins と CID・reason・user_hash は
  // 型に存在しないホワイトリストのみを含む。
  const publicStatsFixture = {
    generated_at: '2026-11-18T00:00:00Z',
    totals: {
      reports: 3,
      concluded: 2,
      approved: 1,
      rejected: 1,
      quorum_not_met: 0,
    },
    monthly: [
      {
        month: '2026-11',
        reports: 3,
        concluded: 2,
        approved: 1,
        rejected: 1,
      },
    ],
  };

  // 公開集計は MODERATION 画面の初期表示でオプインを待たず取得されるため、
  // 全テスト共通の既定モックを登録する（未モックの実サーバーへのリクエストを防ぐ）。
  // R5-09 では同じパターンのルートを後から登録して上書きする。
  // Playwright は最も新しく登録したルートを優先する。
  test.beforeEach(async ({ page }) => {
    await page.route('**/mod/stats/public', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(publicStatsFixture),
      });
    });
  });

  // 投票フィードバック用の共通セットアップ。モック認証でログインし、
  // 投票キューに1件（report_id=42）を表示した状態まで進める。
  const openModerationWithReport = async (page: Page) => {
    await page.route('**/auth/login', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ nonce: 'test-nonce' }) });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: 'juror_hash', token: 'mock_jwt_token' })
      });
    });
    await page.route('**/mod/reports', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([reportFixture(summarizedReason)]),
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'MODERATION' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'MODERATION' }).click();
    await expect(page.getByText('Jury Voting Queue')).toBeVisible({ timeout: 10000 });
    await expect(page.getByRole('button', { name: 'APPROVE (RESTORE)' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'REJECT (VOTE)' })).toBeVisible();
  };

  // 投票一覧の取得失敗を検証するため、認証だけモックして MODERATION 画面へ進む。
  // /mod/reports の失敗モックは呼び出し側が page.goto 前に登録する。
  const openModeration = async (page: Page) => {
    await page.route('**/auth/login', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ nonce: 'test-nonce' }) });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: 'juror_hash', token: 'mock_jwt_token' })
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'MODERATION' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'MODERATION' }).click();
    await expect(page.getByText('Jury Voting Queue')).toBeVisible({ timeout: 10000 });
  };

  test('GOV-01: should report content and transition to Reported status', async ({ page }) => {
    // 認証のモック
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

    await page.route('**/mod/report', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(true),
      });
    });
    // 陪審員キューへ要約済み reason のレポートを1件返す。
    await page.route('**/mod/reports', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([reportFixture(summarizedReason)]),
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'MODERATION' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'MODERATION' }).click();

    // サーバーが要約した reason がそのまま投票キューに表示されること。
    await expect(page.getByText('Jury Voting Queue')).toBeVisible({ timeout: 10000 });
    const reasonLine = page.locator('p:has-text("Reason:")').first();
    await expect(reasonLine).toContainText(summarizedReason, { timeout: 10000 });
    await expect(page.getByText('QmRRTBnaNs7jwYiWadPrEDgrs9a199VZUD3ftVLEJ5DJw5')).toBeVisible();

    await page.locator('input[placeholder="TARGET CID"]').fill('QmTtdWPtHiGyv92HS8cisWMXVh8afyrRUJPDYUraxfCFm2');
    await page.locator('textarea[placeholder="REASON"]').fill('Illegal content detected');

    await page.getByRole('button', { name: 'SUBMIT FORENSIC REPORT' }).click();

    await expect(page.locator('text=Report accepted. Content marked Reported')).toBeVisible({ timeout: 10000 });
  });

  test('GOV-01 (summary): PII を含む元理由は要約済み文字列のみ表示され、メール/電話/URL は露出しない', async ({ page }) => {
    // 元の通報理由は次のような全文（サーバー側で要約される前の想定値）:
    //   `不審な投稿です。連絡先: reporter@example.com / 090-1234-5678 / https://phish.example.com/login`
    // common::summarize_reason が PII を [redacted] に置換し最大120文字へ丸めるため、
    // クライアントが受け取るのは summarizedReason のみ。実サーバー結合は行わない。
    await page.route('**/auth/login', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ nonce: 'test-nonce' }) });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ authorized: true, user_hash: 'juror_hash', token: 'mock_jwt_token' })
      });
    });
    await page.route('**/mod/reports', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([reportFixture(summarizedReason)]),
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'MODERATION' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'MODERATION' }).click();

    // 要約済み文字列と [redacted] は表示される。
    await expect(page.getByText('Jury Voting Queue')).toBeVisible({ timeout: 10000 });
    const reasonLine = page.locator('p:has-text("Reason:")').first();
    await expect(reasonLine).toContainText(summarizedReason, { timeout: 10000 });
    await expect(reasonLine).toContainText('[redacted]');

    // 元全文に含まれていたメールアドレス・電話番号・URL は UI に出ない。
    for (const pii of [piiFixture.email, piiFixture.phone, piiFixture.url]) {
      await expect(reasonLine).not.toContainText(pii);
      await expect(page.locator('body')).not.toContainText(pii);
    }
  });

  test('should display audit trail for content', async ({ page }) => {
    const mockAuditLogs = [
      {
        log_id: 1,
        old_status: null,
        new_status: 'Approved',
        changed_by: 'SYSTEM',
        reason: 'Initial upload',
        created_at: '2026-06-08T00:00:00Z',
        entry_hash: 'abc123',
      },
      {
        log_id: 2,
        old_status: 'Approved',
        new_status: 'Reported',
        changed_by: 'user_hash_123',
        reason: 'User report: suspicious content',
        created_at: '2026-06-08T01:00:00Z',
        entry_hash: 'def456',
      },
    ];

    // 認証のモック
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
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: mockCID,
          checksum: 'mock_checksum',
          type: 'Novel',
          status: 'Reported',
          price: 100,
          title: 'Audit Fixture',
          description: 'Content with a forensic timeline',
          tags: ['audit', 'mock'],
          is_favorite: false
        }]),
      });
    });

    await page.route(`**/content/${mockCID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        body: 'Test content',
      });
    });

    await page.route(`**/content/${mockCID}/audit`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(mockAuditLogs),
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'BROWSE' }).click();
    
    const auditButton = page.locator('button:has-text("Audit")').first();
    await expect(auditButton).toBeVisible({ timeout: 20000 });
    await auditButton.click();

    await expect(page.locator('text=FORENSIC_TIMELINE')).toBeVisible();
    await expect(page.locator('text=Initial upload')).toBeVisible();
    await expect(page.locator('text=User report: suspicious content')).toBeVisible();
  });

  test('R5-09: should mount operator stats panel only on opt-in and stay silent for general users', async ({ page }) => {
    const statsFixture = {
      generated_at: '2026-11-18T00:00:00Z',
      totals: {
        reports: 3,
        concluded: 2,
        approved: 1,
        rejected: 1,
        quorum_not_met: 0,
        bans: 1,
        unbans: 0,
        unpins: 0,
      },
      monthly: [
        {
          month: '2026-11',
          reports: 3,
          concluded: 2,
          approved: 1,
          rejected: 1,
          bans: 1,
          unbans: 0,
          unpins: 0,
        },
      ],
    };

    // 同じテスト内で 200 → 403 を切り替える（ボタン押下でプローブさせる）。
    let forbidStats = false;
    const statsRequests: Array<{ method: string; path: string; authorization: string | undefined }> = [];
    // ADR-0008 段階2: 初期表示と再マウントで送られる公開集計のリクエストを記録する。
    const publicStatsRequests: Array<{ method: string; path: string; authorization: string | undefined }> = [];

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
    await page.route('**/mod/reports', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify([]) });
    });
    // beforeEach の既定モックを上書きし、公開集計のリクエストを記録する。
    await page.route('**/mod/stats/public', async route => {
      publicStatsRequests.push({
        method: route.request().method(),
        path: new URL(route.request().url()).pathname,
        authorization: route.request().headers()['authorization'],
      });
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(publicStatsFixture),
      });
    });
    await page.route('**/mod/stats', async route => {
      statsRequests.push({
        method: route.request().method(),
        path: new URL(route.request().url()).pathname,
        authorization: route.request().headers()['authorization'],
      });
      if (forbidStats) {
        await route.fulfill({
          status: 403,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'forbidden' }),
        });
      } else {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify(statsFixture),
        });
      }
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'MODERATION' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'MODERATION' }).click();

    // フェーズ0（公開集計）: オプインを待たず初期表示で 1 本だけ取得し、
    // 一般画面へホワイトリストの指標だけを描画する。
    await expect(page.getByTestId('public-mod-stats')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('public-mod-stats')).toContainText('GENERATED_AT: 2026-11-18T00:00:00Z');
    await expect(page.getByTestId('public-mod-stats-totals')).toContainText('Reports');
    await expect(page.getByTestId('public-mod-stats-totals')).toContainText('Quorum Not Met');
    await expect(page.getByTestId('public-mod-stats-monthly')).toContainText('2026-11');
    await expect(page.getByTestId('public-mod-stats-error')).toHaveCount(0);
    // 機微キー（型に存在しないフィールド名）は本文へ出ない。
    for (const sensitiveKey of ['reason', 'cid', 'user_hash', 'bans', 'unbans', 'unpins']) {
      await expect(page.locator('body')).not.toContainText(sensitiveKey);
    }
    expect(publicStatsRequests).toEqual([
      { method: 'GET', path: '/mod/stats/public', authorization: 'Bearer mock_jwt_token' },
    ]);
    // 公開集計が発生しても運用者向け /mod/stats は初期表示で 0 本のまま。
    expect(statsRequests).toEqual([]);

    // フェーズ1: 初期表示は一般画面として描画し、/mod/stats を一切送らない。
    await expect(page.getByText('Jury Voting Queue')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('moderation-stats-toggle')).toBeVisible();
    await expect(page.getByTestId('moderation-stats-section')).toHaveCount(0);
    await expect(page.getByTestId('moderation-stats-error')).toHaveCount(0);
    await expect(page.locator('body')).not.toContainText('TRANSPARENCY DASHBOARD');
    await expect(page.locator('body')).not.toContainText(statsForbiddenText);
    expect(statsRequests).toEqual([]);

    // フェーズ2: 中立ボタンの押下で1本だけ送信し、200 応答を根拠にパネルをマウントする。
    await page.getByTestId('moderation-stats-toggle').click();
    await expect(page.getByTestId('moderation-stats')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('moderation-stats-section')).toContainText('TRANSPARENCY DASHBOARD');
    await expect(page.getByTestId('moderation-stats')).toContainText('GENERATED_AT: 2026-11-18T00:00:00Z');
    await expect(page.getByTestId('moderation-stats-totals')).toContainText('Reports');
    await expect(page.getByTestId('moderation-stats-totals')).toContainText('Quorum Not Met');
    await expect(page.getByTestId('moderation-stats-monthly')).toContainText('2026-11');
    await expect(page.getByTestId('moderation-stats-error')).toHaveCount(0);
    expect(statsRequests).toEqual([
      { method: 'GET', path: '/mod/stats', authorization: 'Bearer mock_jwt_token' },
    ]);

    // フェーズ3: 403 では権限エラーを一般画面へ出さず、パネルを黙って閉じる。
    forbidStats = true;
    await page.getByTestId('moderation-stats-toggle').click();
    await expect(page.getByTestId('moderation-stats-section')).toHaveCount(0, { timeout: 10000 });
    await expect(page.getByTestId('moderation-stats-error')).toHaveCount(0);
    await expect(page.locator('body')).not.toContainText(statsForbiddenText);
    expect(statsRequests).toEqual([
      { method: 'GET', path: '/mod/stats', authorization: 'Bearer mock_jwt_token' },
      { method: 'GET', path: '/mod/stats', authorization: 'Bearer mock_jwt_token' },
    ]);

    // 再マウントしても初期表示は 0 本のまま（画面遷移による勝手な再フェッチがないこと）。
    await page.getByRole('button', { name: 'BROWSE' }).click();
    await page.getByRole('button', { name: 'MODERATION' }).click();
    await expect(page.getByTestId('moderation-stats-toggle')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('moderation-stats-section')).toHaveCount(0);
    expect(statsRequests).toHaveLength(2);
    // 再マウント後の公開集計のロード完了を待ってから本数を確認する（レース回避）。
    await expect(page.getByTestId('public-mod-stats')).toContainText('GENERATED_AT: 2026-11-18T00:00:00Z', { timeout: 10000 });
    // 公開集計はマウントごとに 1 本増える（初期表示 1 本 + 再マウント 1 本 = 2 本）。
    expect(publicStatsRequests).toHaveLength(2);

    // 未認証 401: page.request（APIRequestContext）は page.route の対象外なので
    // 実サーバーへ送る。絶対 URL でポート 3000 を明示する（client の baseURL:8080 を使わない）。
    const unauthorized = await page.request.get('http://127.0.0.1:3000/mod/stats/public');
    expect(unauthorized.status()).toBe(401);
  });

  test('R5-09 (parse error): /mod/stats が不正 JSON のとき moderation-stats-error に固定文言のみを表示し serde の生エラーを出さない', async ({ page }) => {
    // totals に文字列を入れて deserialization を失敗させる。serde の生エラーには
    // この文字列（フィールド値）や型情報・解析位置が含まれ得るため、センチネルとして
    // 画面に漏れないことを検証する。
    const sentinel = 'RAW_SERDE_ERROR_SENTINEL_DO_NOT_SHOW';
    const statsRequests: Array<{ method: string; path: string; authorization: string | undefined }> = [];

    await page.route('**/mod/stats', async route => {
      statsRequests.push({
        method: route.request().method(),
        path: new URL(route.request().url()).pathname,
        authorization: route.request().headers()['authorization'],
      });
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          generated_at: '2026-11-18T00:00:00Z',
          totals: sentinel,
        }),
      });
    });
    await page.route('**/mod/reports', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify([]) });
    });
    await openModeration(page);
    // 初期表示では 0 本なので、固定文言の検証はオプイン押下後にパネルを開いて行う。
    expect(statsRequests).toEqual([]);
    await page.getByTestId('moderation-stats-toggle').click();

    const statsError = page.getByTestId('moderation-stats-error');
    await expect(statsError).toBeVisible({ timeout: 10000 });
    await expect(statsError).toHaveAttribute('role', 'alert');
    // 固定文言そのものが表示される。
    await expect(statsError).toHaveText(statsParseErrorText);
    // 集計カードは出さない。
    await expect(page.getByTestId('moderation-stats')).toHaveCount(0);
    // 注入したセンチネル（生エラーに含まれ得るフィールド値）はページ全体で出ない。
    await expect(page.locator('body')).not.toContainText(sentinel);
    // serde の生エラー（フィールド名・型情報・解析位置）がエラー要素に出ない。
    for (const rawDetail of [
      'totals',
      'invalid type',
      'expected struct',
      'at line 1 column',
      'line 1',
      'serde',
      'error decoding response body',
    ]) {
      await expect(statsError).not.toContainText(rawDetail);
    }
    // 認証トークン付きで 1 回だけ取得されていること。
    expect(statsRequests).toEqual([
      { method: 'GET', path: '/mod/stats', authorization: 'Bearer mock_jwt_token' },
    ]);
  });

  test('VOTE-FB-01: /mod/vote が 403 を返すと vote-error に HTTP 403 を含む固定文言を表示する', async ({ page }) => {
    const voteRequests: Array<{ method: string; path: string; authorization: string | undefined }> = [];
    await page.route('**/mod/vote', async route => {
      voteRequests.push({
        method: route.request().method(),
        path: new URL(route.request().url()).pathname,
        authorization: route.request().headers()['authorization'],
      });
      await route.fulfill({
        status: 403,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'SERVER_DETAIL_MUST_NOT_BE_SHOWN' }),
      });
    });
    await openModerationWithReport(page);

    await expect(page.getByTestId('vote-error')).toHaveCount(0);
    await expect(page.getByTestId('vote-notice')).toHaveCount(0);

    await page.getByRole('button', { name: 'APPROVE (RESTORE)' }).click();

    const voteError = page.getByTestId('vote-error');
    await expect(voteError).toBeVisible({ timeout: 10000 });
    await expect(voteError).toHaveAttribute('role', 'alert');
    await expect(voteError).toHaveText(voteForbiddenText);
    await expect(voteError).toContainText('HTTP 403');
    await expect(voteError).not.toContainText('SERVER_DETAIL_MUST_NOT_BE_SHOWN');
    await expect(page.getByTestId('vote-notice')).toHaveCount(0);
    expect(voteRequests).toEqual([
      { method: 'POST', path: '/mod/vote', authorization: 'Bearer mock_jwt_token' },
    ]);
  });

  test('VOTE-FB-02: /mod/vote が network error の場合にも vote-error に固定文言を表示する', async ({ page }) => {
    await page.route('**/mod/vote', async route => {
      await route.abort();
    });
    await openModerationWithReport(page);

    await expect(page.getByTestId('vote-error')).toHaveCount(0);
    await page.getByRole('button', { name: 'APPROVE (RESTORE)' }).click();

    const voteError = page.getByTestId('vote-error');
    await expect(voteError).toBeVisible({ timeout: 10000 });
    await expect(voteError).toHaveAttribute('role', 'alert');
    await expect(voteError).toHaveText(voteNetworkErrorText);
    await expect(page.getByTestId('vote-notice')).toHaveCount(0);
  });

  test('VOTE-FB-03: 成功時に vote-notice を表示し、新しい試行で通知とエラーをクリアする', async ({ page }) => {
    let forbidVote = true;
    const voteRequests: Array<{ method: string; path: string; authorization: string | undefined }> = [];
    await page.route('**/mod/vote', async route => {
      voteRequests.push({
        method: route.request().method(),
        path: new URL(route.request().url()).pathname,
        authorization: route.request().headers()['authorization'],
      });
      if (forbidVote) {
        await route.fulfill({
          status: 403,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'forbidden' }),
        });
      } else {
        await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(true) });
      }
    });
    await openModerationWithReport(page);

    // 失敗 → vote-error を表示する。
    await page.getByRole('button', { name: 'APPROVE (RESTORE)' }).click();
    await expect(page.getByTestId('vote-error')).toHaveText(voteForbiddenText, { timeout: 10000 });
    await expect(page.getByTestId('vote-notice')).toHaveCount(0);

    // 成功 → エラーが消え、vote-notice を表示する。
    forbidVote = false;
    await page.getByRole('button', { name: 'APPROVE (RESTORE)' }).click();
    await expect(page.getByTestId('vote-error')).toHaveCount(0);
    await expect(page.getByTestId('vote-notice')).toHaveText(voteNoticeText, { timeout: 10000 });
    // vote-notice（data-testid="vote-notice"）は role="status"。vote-error は role="alert" のまま。
    await expect(page.getByTestId('vote-notice')).toHaveAttribute('role', 'status');

    // 再度失敗 → 通知が消え、エラーが表示される（新しい試行でクリア）。
    forbidVote = true;
    await page.getByRole('button', { name: 'APPROVE (RESTORE)' }).click();
    await expect(page.getByTestId('vote-notice')).toHaveCount(0);
    await expect(page.getByTestId('vote-error')).toHaveText(voteForbiddenText, { timeout: 10000 });

    expect(voteRequests).toEqual([
      { method: 'POST', path: '/mod/vote', authorization: 'Bearer mock_jwt_token' },
      { method: 'POST', path: '/mod/vote', authorization: 'Bearer mock_jwt_token' },
      { method: 'POST', path: '/mod/vote', authorization: 'Bearer mock_jwt_token' },
    ]);
  });

  test('REPORTS-ERR-01: /mod/reports が 500 のとき reports-error に固定文言を表示し NO_PENDING_VOTES を出さない', async ({ page }) => {
    const reportRequests: Array<{ method: string; path: string; authorization: string | undefined }> = [];
    await page.route('**/mod/reports', async route => {
      reportRequests.push({
        method: route.request().method(),
        path: new URL(route.request().url()).pathname,
        authorization: route.request().headers()['authorization'],
      });
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'SERVER_DETAIL_MUST_NOT_BE_SHOWN' }),
      });
    });
    await openModeration(page);

    const reportsError = page.getByTestId('reports-error');
    await expect(reportsError).toBeVisible({ timeout: 10000 });
    await expect(reportsError).toHaveAttribute('role', 'alert');
    await expect(reportsError).toHaveText(reportsHttpErrorText(500));
    await expect(reportsError).toContainText('HTTP 500');
    await expect(reportsError).not.toContainText('SERVER_DETAIL_MUST_NOT_BE_SHOWN');
    // 取得失敗を「保留投票なし」と誤表示しない。
    await expect(page.getByText('NO_PENDING_VOTES')).toHaveCount(0);
    expect(reportRequests).toEqual([
      { method: 'GET', path: '/mod/reports', authorization: 'Bearer mock_jwt_token' },
    ]);
  });

  test('REPORTS-ERR-02: /mod/reports が network error のとき reports-error に固定文言を表示し NO_PENDING_VOTES を出さない', async ({ page }) => {
    await page.route('**/mod/reports', async route => {
      await route.abort();
    });
    await openModeration(page);

    const reportsError = page.getByTestId('reports-error');
    await expect(reportsError).toBeVisible({ timeout: 10000 });
    await expect(reportsError).toHaveAttribute('role', 'alert');
    await expect(reportsError).toHaveText(reportsNetworkErrorText);
    await expect(page.getByText('NO_PENDING_VOTES')).toHaveCount(0);
  });

  test('REPORT-ERR-01: /mod/report が 500 を返すと固定文言のみを表示しサーバー本文・成功文言を出さない', async ({ page }) => {
    const reportRequests: Array<{ method: string; path: string; authorization: string | undefined }> = [];
    await page.route('**/mod/report', async route => {
      reportRequests.push({
        method: route.request().method(),
        path: new URL(route.request().url()).pathname,
        authorization: route.request().headers()['authorization'],
      });
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        // サーバー本文のセンチネル。固定文言以外が画面へ漏れないことを検証する。
        body: JSON.stringify({ error: 'SERVER_DETAIL_MUST_NOT_BE_SHOWN' }),
      });
    });
    await page.route('**/mod/reports', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify([]) });
    });
    await openModeration(page);

    // 送信失敗は dedicated な role="alert"（data-testid="report-failed"）で描画される。
    // 入力検証の report-validation-error とは別要素であり、初期状態では存在しない。
    const reportFailed = page.getByTestId('report-failed');
    await expect(reportFailed).toHaveCount(0);

    // 空 CID / 空 reason は validate_report_input のクライアント側ガードで止まり、
    // 専用の role="alert"（data-testid="report-validation-error"）を出す。POST は0本。
    const validationError = page.getByTestId('report-validation-error');
    await expect(validationError).toHaveCount(0);

    await page.locator('input[placeholder="TARGET CID"]').fill('');
    await page.locator('textarea[placeholder="REASON"]').fill('Illegal content detected');
    await page.getByRole('button', { name: 'SUBMIT FORENSIC REPORT' }).click();
    await expect(validationError).toHaveText('対象 CID を入力してください。');
    await expect(validationError).toHaveAttribute('role', 'alert');
    expect(reportRequests).toEqual([]);
    await expect(reportFailed).toHaveCount(0);

    await page.locator('input[placeholder="TARGET CID"]').fill('QmTtdWPtHiGyv92HS8cisWMXVh8afyrRUJPDYUraxfCFm2');
    await page.locator('textarea[placeholder="REASON"]').fill('');
    await page.getByRole('button', { name: 'SUBMIT FORENSIC REPORT' }).click();
    await expect(validationError).toHaveText('通報理由を入力してください。');
    await expect(validationError).toHaveAttribute('role', 'alert');
    expect(reportRequests).toEqual([]);
    await expect(reportFailed).toHaveCount(0);

    // ここから通常の送信フェーズ。検証エラーは消え、POST は1本だけになる。
    await page.locator('textarea[placeholder="REASON"]').fill('Illegal content detected');
    await page.getByRole('button', { name: 'SUBMIT FORENSIC REPORT' }).click();

    await expect(validationError).toHaveCount(0);
    await expect(reportFailed).toHaveCount(1);
    // 失敗は dedicated role="alert"、成功・進捗は status のまま。
    await expect(reportFailed).toHaveAttribute('role', 'alert');
    await expect(reportFailed).toHaveText(reportFailedText, { timeout: 10000 });
    // 成功文言とサーバー本文は出ない。
    await expect(reportFailed).not.toContainText(reportSuccessText);
    await expect(page.locator('body')).not.toContainText('SERVER_DETAIL_MUST_NOT_BE_SHOWN');
    expect(reportRequests).toEqual([
      { method: 'POST', path: '/mod/report', authorization: 'Bearer mock_jwt_token' },
    ]);
  });

  test('REPORT-ERR-02: /mod/report が network error でも固定文言のみを表示する', async ({ page }) => {
    await page.route('**/mod/report', async route => {
      await route.abort();
    });
    await page.route('**/mod/reports', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify([]) });
    });
    await openModeration(page);

    const reportFailed = page.getByTestId('report-failed');
    await expect(reportFailed).toHaveCount(0);

    await page.locator('input[placeholder="TARGET CID"]').fill('QmTtdWPtHiGyv92HS8cisWMXVh8afyrRUJPDYUraxfCFm2');
    await page.locator('textarea[placeholder="REASON"]').fill('Illegal content detected');
    await page.getByRole('button', { name: 'SUBMIT FORENSIC REPORT' }).click();

    // abort でも HTTP 500 と同じ固定文言（submit_report の else アーム）。
    await expect(reportFailed).toHaveAttribute('role', 'alert');
    await expect(reportFailed).toHaveText(reportFailedText, { timeout: 10000 });
    await expect(reportFailed).not.toContainText(reportSuccessText);
    // 生のネットワークエラー文字列は画面へ出さない。
    for (const raw of ['Failed to fetch', 'net::ERR_FAILED', 'AbortError']) {
      await expect(reportFailed).not.toContainText(raw);
    }
  });

  // 以下はお気に入りの double-click E2E（favorites.spec.ts:345）と同じ方式で、
  // submit_report / do_vote の二重送信ガードを検証する独立 test。
  // 既存の投票3回完全一致・お気に入り2/8回アサーションは変更しない。
  test('REPORT-GUARD-01: レスポンス保留中の二重クリックでも /mod/report への POST は1本だけ', async ({ page }) => {
    let releaseResponse = (): void => {};
    const responseGate = new Promise<void>(resolve => {
      releaseResponse = resolve;
    });
    const reportRequests: Array<{ method: string; path: string; authorization: string | undefined }> = [];
    await page.route('**/mod/report', async route => {
      reportRequests.push({
        method: route.request().method(),
        path: new URL(route.request().url()).pathname,
        authorization: route.request().headers()['authorization'],
      });
      // レスポンスを保留し、送信中ガードが効いている間は busy 状態を維持する。
      await responseGate;
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(true),
      });
    });
    await page.route('**/mod/reports', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify([]) });
    });
    await openModeration(page);

    await page.locator('input[placeholder="TARGET CID"]').fill('QmTtdWPtHiGyv92HS8cisWMXVh8afyrRUJPDYUraxfCFm2');
    await page.locator('textarea[placeholder="REASON"]').fill('Illegal content detected');

    const submit = page.getByRole('button', { name: 'SUBMIT FORENSIC REPORT' });
    await expect(submit).toBeEnabled();
    await expect(submit).toHaveAttribute('aria-busy', 'false');

    // 予告なしの高速2クリック（同一タスクで2回 dispatch）。
    await submit.evaluate((el: HTMLElement) => {
      el.click();
      el.click();
    });
    await expect.poll(() => reportRequests.length).toBe(1);

    // レスポンス保留中は disabled / aria-busy=true になるまで明示的に待ってから
    // もう一度送る（固定 sleep は使わない）。ガードが効き POST は増えない。
    await expect(submit).toBeDisabled();
    await expect(submit).toHaveAttribute('aria-busy', 'true');
    await submit.evaluate((el: HTMLElement) => el.click());
    await expect(submit).toBeDisabled();
    await expect.poll(() => reportRequests.length).toBe(1);

    // 完了後も再送されず、ガードが解除されて次の操作を受け付ける。
    releaseResponse();
    await expect(submit).toBeEnabled();
    await expect(submit).toHaveAttribute('aria-busy', 'false');
    await expect(page.locator('p[role="status"]:not([data-testid])')).toHaveText(reportSuccessText, { timeout: 10000 });
    await expect.poll(() => reportRequests.length).toBe(1);
    expect(reportRequests).toEqual([
      { method: 'POST', path: '/mod/report', authorization: 'Bearer mock_jwt_token' },
    ]);
  });

  test('VOTE-GUARD-01: レスポンス保留中の二重クリックでも /mod/vote への POST は1本だけ', async ({ page }) => {
    let releaseResponse = (): void => {};
    const responseGate = new Promise<void>(resolve => {
      releaseResponse = resolve;
    });
    const voteRequests: Array<{ method: string; path: string; authorization: string | undefined }> = [];
    await page.route('**/mod/vote', async route => {
      voteRequests.push({
        method: route.request().method(),
        path: new URL(route.request().url()).pathname,
        authorization: route.request().headers()['authorization'],
      });
      // レスポンスを保留し、送信中ガードが効いている間は busy 状態を維持する。
      await responseGate;
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(true),
      });
    });
    await openModerationWithReport(page);

    const approve = page.getByRole('button', { name: 'APPROVE (RESTORE)' });
    await expect(approve).toBeEnabled();
    await expect(approve).toHaveAttribute('aria-busy', 'false');

    // 予告なしの高速2クリック（同一タスクで2回 dispatch）。
    await approve.evaluate((el: HTMLElement) => {
      el.click();
      el.click();
    });
    await expect.poll(() => voteRequests.length).toBe(1);

    // レスポンス保留中は disabled / aria-busy=true になるまで明示的に待ってから
    // もう一度送る（固定 sleep は使わない）。ガードが効き POST は増えない。
    await expect(approve).toBeDisabled();
    await expect(approve).toHaveAttribute('aria-busy', 'true');
    await approve.evaluate((el: HTMLElement) => el.click());
    await expect(approve).toBeDisabled();
    await expect.poll(() => voteRequests.length).toBe(1);

    // 完了後も再送されず、ガードが解除されて次の操作を受け付ける。
    releaseResponse();
    await expect(approve).toBeEnabled();
    await expect(approve).toHaveAttribute('aria-busy', 'false');
    await expect(page.getByTestId('vote-notice')).toHaveText(voteNoticeText, { timeout: 10000 });
    await expect.poll(() => voteRequests.length).toBe(1);
    expect(voteRequests).toEqual([
      { method: 'POST', path: '/mod/vote', authorization: 'Bearer mock_jwt_token' },
    ]);
  });
});
