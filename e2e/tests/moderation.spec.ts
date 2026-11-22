import { test, expect, type Page, type Locator } from '@playwright/test';
import * as crypto from 'crypto';

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
  const reportsParseErrorText = '投票一覧の解析に失敗しました。';

  // submit_report の失敗時の固定文言。
  // 成功・進捗は status シグナルへ入り <p role="status" class="text-xs text-gray-400">
  // （通報 status ライブリージョン）で描画される。失敗は dedicated な
  // <p role="alert" data-testid="report-failed"> で描画し、入力検証の
  // report-validation-error とは data-testid で区別する。
  const reportFailedText = 'Report failed.';
  const reportSuccessText = 'Report accepted. Content marked Reported.';

  // R2-CLIENT-06: 集計 GENERATED_AT は format_local_datetime で整形表示し、
  // 生 ISO は title 属性へ保持する。ロケール差に依存しない書式のみを検証する。
  const generatedAtRaw = '2026-11-18T00:00:00Z';
  const assertGeneratedAt = async (scope: Locator, testId: string) => {
    const generatedAt = scope.getByTestId(testId);
    await expect(generatedAt).toBeVisible();
    const text = (await generatedAt.textContent())?.trim() ?? '';
    // ロケール依存の完全一致は避け、生 ISO でないことと年が含まれることのみを確認する。
    expect(text).toMatch(/^GENERATED_AT: /);
    expect(text).toContain('2026');
    expect(text).not.toContain(generatedAtRaw);
    await expect(generatedAt).toHaveAttribute('title', generatedAtRaw);
  };

  // client/src/lib.rs の stats_error_message(StatsFailure::Parse) を grep して確定した
  // 固定文言。serde の生エラー（フィールド名・解析位置・型情報・レスポンス本文）は
  // 画面へ出さない契約を検証する。
  const statsParseErrorText = '集計の解析に失敗しました。';
  // client/src/lib.rs の PUBLIC_STATS_UNAUTHORIZED（公開集計 401 専用の固定文言）。
  // 403 / 5xx / network は従来どおり「集計の取得に失敗しました。」を維持する。
  const publicStatsUnauthorizedText = '再ログインが必要です。';
  const publicStatsReloginHintText = 'LOG OUT からログアウトし、再度 JPKI でログインしてください。';
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

    // 投票前に本文・監査を確認できることを検証するためのカード／監査モック。
    const targetCid = reportFixture(summarizedReason).target_cid;
    const targetBody = 'Body to confirm before vote';
    const targetChecksum = crypto.createHash('sha256').update(targetBody).digest('hex');
    const targetTitle = 'Deep Link Fixture';
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: targetCid,
          checksum: targetChecksum,
          type: 'Novel',
          status: 'Reported',
          price: 0,
          title: targetTitle,
          description: 'Content to inspect before voting',
          tags: ['review'],
          is_favorite: false,
        }]),
      });
    });
    await page.route(`**/content/${targetCid}`, async route => {
      await route.fulfill({ status: 200, contentType: 'text/markdown', body: targetBody });
    });
    await page.route(`**/content/${targetCid}/audit`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          log_id: 1,
          old_status: null,
          new_status: 'Reported',
          changed_by: 'SYSTEM',
          reason: 'Initial report',
          created_at: '2026-11-18T00:00:00Z',
          entry_hash: 'abc123',
        }]),
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

    // 投票キーの target_cid が BROWSE の該当カードへのディープリンクになっていること。
    const targetLink = page.getByTestId('report-target-link');
    await expect(targetLink).toHaveAttribute(
      'href',
      /cid=QmRRTBnaNs7jwYiWadPrEDgrs9a199VZUD3ftVLEJ5DJw5#dashboard$/,
    );
    await targetLink.click();
    await expect(page).toHaveURL(/#dashboard$/);
    await expect(page).toHaveURL(/cid=QmRRTBnaNs7jwYiWadPrEDgrs9a199VZUD3ftVLEJ5DJw5/);
    const linkedCard = page.locator(`[data-testid="content-card"][data-cid="${targetCid}"]`);
    await expect(linkedCard).toBeVisible({ timeout: 10000 });
    // 本文（検証済みコンテンツ）と監査タイムラインを投票前に確認できること。
    await expect(linkedCard).toContainText(targetTitle);
    await expect(linkedCard).toContainText(targetBody);
    await linkedCard.getByRole('button', { name: 'Audit' }).click();
    await expect(page.getByText('FORENSIC_TIMELINE')).toBeVisible();
    await expect(page.getByText('Initial report')).toBeVisible();
    // 確認後は MODERATION へ戻り、投票可能な状態が維持されていること。
    await page.getByRole('button', { name: 'MODERATION' }).click();
    await expect(page.getByRole('button', { name: 'APPROVE (RESTORE)' })).toBeVisible();

    // A11Y: 通報フォームは既存 aria-label を維持したうえで <label for> でも関連付ける。
    const cidInput = page.locator('input[placeholder="TARGET CID"]');
    const reasonInput = page.locator('textarea[placeholder="REASON"]');
    await expect(page.locator('label[for="report-target-cid"]')).toHaveText('TARGET CID');
    await expect(cidInput).toHaveAttribute('id', 'report-target-cid');
    await expect(cidInput).toHaveAttribute('aria-label', 'TARGET CID');
    await expect(page.getByLabel('TARGET CID', { exact: true })).toHaveAttribute('id', 'report-target-cid');
    await expect(page.locator('label[for="report-reason"]')).toHaveText('REASON');
    await expect(reasonInput).toHaveAttribute('id', 'report-reason');
    await expect(reasonInput).toHaveAttribute('aria-label', 'REASON');
    await expect(page.getByLabel('REASON', { exact: true })).toHaveAttribute('id', 'report-reason');

    await cidInput.fill('QmTtdWPtHiGyv92HS8cisWMXVh8afyrRUJPDYUraxfCFm2');
    await reasonInput.fill('Illegal content detected');

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
    // 公開集計も同じケース内で 200 → 不正 JSON → 401 → 403 へ切り替える
    // （Parse / 401 / 403 分類の回帰）。403/5xx/network は従来文言を維持する。
    let brokenPublicStats = false;
    let unauthorizedPublicStats = false;
    let forbiddenPublicStats = false;
    // 鮮度警告の検証用: 生成時刻を「現在」に差し替えて警告が消えることを確認する。
    let freshPublicStats = false;
    // 遅延レスポンスの順序逆転検証用（R2-CLIENT 世代ガード）。
    // マウント取得を保留中に「更新」で最新を取得し、古い応答が後から届いても
    // 画面が最新のままであることを同じ R5-09 ケース内で確認する。
    let delayedPublicStats = false;
    let delayedPublicStatsCount = 0;
    let releaseOldPublicStats = (): void => {};
    const oldPublicStatsGate = new Promise<void>(resolve => {
      releaseOldPublicStats = resolve;
    });
    let releaseNewPublicStats = (): void => {};
    const newPublicStatsGate = new Promise<void>(resolve => {
      releaseNewPublicStats = resolve;
    });
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
    // R2-REFRESH-ENABLE: refresh は失敗側でモックする。失敗時は元の 401 を返す契約を維持する。
    await page.route('**/auth/refresh', async route => {
      await route.fulfill({
        status: 401,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'unauthorized' }),
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
      if (delayedPublicStats) {
        // 1本目（マウント取得）を保留して古いレスポンスとして後から返し、
        // 2本目（更新ボタン）を先に完了させる。generation guard の順序逆転オラクル。
        delayedPublicStatsCount += 1;
        if (delayedPublicStatsCount === 1) {
          await oldPublicStatsGate;
          await route.fulfill({
            status: 200,
            contentType: 'application/json',
            body: JSON.stringify({
              ...publicStatsFixture,
              generated_at: '2026-11-01T00:00:00Z',
            }),
          });
          return;
        }
        await newPublicStatsGate;
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            ...publicStatsFixture,
            generated_at: '2026-11-19T00:00:00Z',
          }),
        });
        return;
      }
      if (unauthorizedPublicStats) {
        // 401 は dedicated 固定文言＋LOG OUT からの再ログイン案内を出す。
        await route.fulfill({
          status: 401,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'UNAUTHORIZED_SENTINEL_DO_NOT_SHOW' }),
        });
        return;
      }
      if (forbiddenPublicStats) {
        // 403 は 401 専用文言へ切り替えず、従来の取得失敗文言を維持する。
        await route.fulfill({
          status: 403,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'FORBIDDEN_SENTINEL_DO_NOT_SHOW' }),
        });
        return;
      }
      if (brokenPublicStats) {
        // totals を文字列にして PublicModerationStats の deserialization を失敗させる。
        // センチネルは生エラーに含まれ得る値で、画面へ出ないことを検証する。
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            generated_at: publicStatsFixture.generated_at,
            totals: 'RAW_SERDE_ERROR_SENTINEL_DO_NOT_SHOW',
          }),
        });
        return;
      }
      if (freshPublicStats) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            ...publicStatsFixture,
            generated_at: new Date().toISOString(),
          }),
        });
        return;
      }
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
    await assertGeneratedAt(page.getByTestId('public-mod-stats'), 'public-mod-stats-generated-at');
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
    await assertGeneratedAt(page.getByTestId('moderation-stats'), 'moderation-stats-generated-at');
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
    await assertGeneratedAt(page.getByTestId('public-mod-stats'), 'public-mod-stats-generated-at');
    // 公開集計はマウントごとに 1 本増える（初期表示 1 本 + 再マウント 1 本 = 2 本）。
    expect(publicStatsRequests).toHaveLength(2);

    // フェーズ4: 公開集計が不正 JSON のときも運用者向け /mod/stats と同じ Parse 分類の
    // 固定文言を出し、serde の生エラー（センチネル）は出さない。既存ケース内で完結する。
    brokenPublicStats = true;
    await page.getByRole('button', { name: 'BROWSE' }).click();
    await page.getByRole('button', { name: 'MODERATION' }).click();
    const publicStatsError = page.getByTestId('public-mod-stats-error');
    await expect(publicStatsError).toBeVisible({ timeout: 10000 });
    await expect(publicStatsError).toHaveAttribute('role', 'alert');
    await expect(publicStatsError).toHaveText(statsParseErrorText);
    await expect(publicStatsError).not.toContainText('RAW_SERDE_ERROR_SENTINEL_DO_NOT_SHOW');
    await expect(page.getByTestId('public-mod-stats-content')).toHaveCount(0);
    await expect(page.getByTestId('public-mod-stats-generated-at')).toHaveCount(0);
    // オプイン押下はしていないので /mod/stats は増えず、公開集計だけ3本目が送られる。
    expect(statsRequests).toHaveLength(2);
    expect(publicStatsRequests).toHaveLength(3);

    // フェーズ5: 公開集計が 401 のときだけ dedicated 固定文言と LOG OUT からの
    // 再ログイン案内を出し、403 / 5xx / network の従来文言へは切り替えない。
    // サーバー本文（センチネル）は画面へ出さない。既存ケース内で完結する。
    brokenPublicStats = false;
    unauthorizedPublicStats = true;
    await page.getByRole('button', { name: 'BROWSE' }).click();
    await page.getByRole('button', { name: 'MODERATION' }).click();
    await expect(publicStatsError).toBeVisible({ timeout: 10000 });
    await expect(publicStatsError).toHaveText(publicStatsUnauthorizedText);
    await expect(publicStatsError).not.toContainText('UNAUTHORIZED_SENTINEL_DO_NOT_SHOW');
    await expect(page.getByTestId('public-mod-stats-relogin-hint')).toHaveText(publicStatsReloginHintText);
    await expect(page.getByTestId('public-mod-stats-content')).toHaveCount(0);
    expect(statsRequests).toHaveLength(2);
    expect(publicStatsRequests).toHaveLength(4);

    // 403 は 401 専用文言へ切り替えず、従来の「集計の取得に失敗しました。」を維持する（回帰）。
    unauthorizedPublicStats = false;
    forbiddenPublicStats = true;
    await page.getByRole('button', { name: 'BROWSE' }).click();
    await page.getByRole('button', { name: 'MODERATION' }).click();
    await expect(publicStatsError).toHaveText('集計の取得に失敗しました。', { timeout: 10000 });
    await expect(publicStatsError).not.toContainText('FORBIDDEN_SENTINEL_DO_NOT_SHOW');
    await expect(page.getByTestId('public-mod-stats-relogin-hint')).toHaveCount(0);

    // フェーズ6: 公開集計の鮮度警告と「更新」ボタン。GENERATED_AT が古い fixture では
    // dedicated 警告を出し、ボタン押下で公開集計だけ再取得する。認証付きオプイン集計の
    // /mod/stats は一切増えない（状態変更なし）。
    forbiddenPublicStats = false;
    freshPublicStats = false;
    await page.getByRole('button', { name: 'BROWSE' }).click();
    await page.getByRole('button', { name: 'MODERATION' }).click();
    await assertGeneratedAt(page.getByTestId('public-mod-stats'), 'public-mod-stats-generated-at');
    const staleWarning = page.getByTestId('public-mod-stats-stale');
    await expect(staleWarning).toBeVisible({ timeout: 10000 });
    // A11Y: 鮮度警告は色だけに依存せず role=status と ⚠ の dedicated 要素で告知する。
    await expect(staleWarning).toHaveAttribute('role', 'status');
    await expect(staleWarning.locator('[data-testid="public-mod-stats-stale-icon"]')).toHaveAttribute('aria-hidden', 'true');
    await expect(staleWarning.locator('[data-testid="public-mod-stats-stale-icon"]')).toContainText('⚠');
    const statsCountBeforeRefresh = statsRequests.length;
    const publicCountBeforeRefresh = publicStatsRequests.length;
    await page.getByTestId('public-mod-stats-refresh').click();
    await expect.poll(() => publicStatsRequests.length).toBe(publicCountBeforeRefresh + 1);
    expect(statsRequests).toHaveLength(statsCountBeforeRefresh);
    // 生成時刻を現在へ差し替えて更新すると鮮度警告が消える。
    freshPublicStats = true;
    await page.getByTestId('public-mod-stats-refresh').click();
    await expect(staleWarning).toHaveCount(0, { timeout: 10000 });
    // 属性セレクタでも role=status の警告が消えたことを確認する。
    await expect(page.locator('p[role="status"][data-testid="public-mod-stats-stale"]')).toHaveCount(0);
    expect(statsRequests).toHaveLength(statsCountBeforeRefresh);

    // フェーズ7: 遅延レスポンスの順序逆転（R2-CLIENT 世代ガード）。
    // マウント取得（古い GENERATED_AT）を保留中に「更新」で最新を取得し、
    // 古い応答が後から到着しても最新のまま。更新ボタンの disabled / aria-busy も検証する。
    delayedPublicStats = true;
    delayedPublicStatsCount = 0;
    const refreshButton = page.getByTestId('public-mod-stats-refresh');
    const statsCountBeforeRace = statsRequests.length;
    const publicCountBeforeRace = publicStatsRequests.length;
    await page.getByRole('button', { name: 'BROWSE' }).click();
    await page.getByRole('button', { name: 'MODERATION' }).click();
    await expect.poll(() => publicStatsRequests.length).toBe(publicCountBeforeRace + 1);
    // マウント取得は busy を立てないため、更新ボタンは押せる。
    await expect(refreshButton).toBeEnabled();
    await expect(refreshButton).toHaveAttribute('aria-busy', 'false');
    await refreshButton.click();
    await expect.poll(() => publicStatsRequests.length).toBe(publicCountBeforeRace + 2);
    // 更新リクエストの保留中は連打抑止が効く。
    await expect(refreshButton).toBeDisabled();
    await expect(refreshButton).toHaveAttribute('aria-busy', 'true');
    // 最新レスポンスを先に到着させる。
    releaseNewPublicStats();
    await expect(page.getByTestId('public-mod-stats-generated-at'))
      .toHaveAttribute('title', '2026-11-19T00:00:00Z', { timeout: 10000 });
    await expect(refreshButton).toBeEnabled();
    await expect(refreshButton).toHaveAttribute('aria-busy', 'false');
    // 古いレスポンスが後から到着しても、一覧と同じ世代ガードで破棄される。
    // waitForResponse で古い応答の到着を確定させてから最新のままであることを確認する。
    const oldStatsResponse = page.waitForResponse(
      response => response.url().includes('/mod/stats/public') && response.status() === 200,
      { timeout: 10000 },
    );
    releaseOldPublicStats();
    await oldStatsResponse;
    await expect.poll(() => page.getByTestId('public-mod-stats-generated-at').getAttribute('title'))
      .toBe('2026-11-19T00:00:00Z');
    await expect(publicStatsError).toHaveCount(0);
    expect(statsRequests).toHaveLength(statsCountBeforeRace);

    // 日本語側の lang 宣言（error_lang 統一）を既存ケース内で検証する。
    await expect(page.getByTestId('public-mod-stats-stale')).toHaveAttribute('lang', 'ja');

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
    // 後続フェーズで成功／壊れたフィールド型の JSON へ差し替えるためのフラグ。
    let brokenPayload = false;
    let serveSuccess = false;
    await page.route('**/mod/reports', async route => {
      reportRequests.push({
        method: route.request().method(),
        path: new URL(route.request().url()).pathname,
        authorization: route.request().headers()['authorization'],
      });
      if (serveSuccess) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify([reportFixture(summarizedReason)]),
        });
        return;
      }
      if (brokenPayload) {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          // report_id が文字列、reason が数値など Vec<Report> として読めない値。
          // 生の serde エラーもサーバー本文（センチネル）も出さないことを検証する。
          body: JSON.stringify([{
            report_id: 'not-a-number',
            target_cid: 42,
            reason: 123,
            created_at: [],
            status: true,
            debug: 'SERVER_DETAIL_MUST_NOT_BE_SHOWN',
          }]),
        });
        return;
      }
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

    // 追加フェーズ: 同じケース内でステータス200だがフィールド型が壊れた JSON へ差し替え、
    // 再マウントして初回フェッチをやり直す。固定文言のみを出し、サーバー本文と
    // 「保留投票なし」の誤表示は出さない（REPORTS-ERR / REPORTS_PARSE_ERROR）。
    brokenPayload = true;
    await openModeration(page);

    await expect(reportsError).toHaveText(reportsParseErrorText, { timeout: 10000 });
    await expect(reportsError).toHaveAttribute('role', 'alert');
    await expect(reportsError).not.toContainText('SERVER_DETAIL_MUST_NOT_BE_SHOWN');
    await expect(page.getByText('NO_PENDING_VOTES')).toHaveCount(0);

    // 追加フェーズ（R2-CLIENT-01）: 成功 → 500 切替 → 再マウント。
    // 同一マウント内で投票成功をトリガに一覧を再取得させ、失敗後も古いカードが
    // 残っていても投票ボタンが disabled になることを検証する。
    serveSuccess = true;
    brokenPayload = false;
    await openModeration(page);

    const approve = page.getByRole('button', { name: 'APPROVE (RESTORE)' });
    const reject = page.getByRole('button', { name: 'REJECT (VOTE)' });
    await expect(approve).toBeEnabled({ timeout: 10000 });
    await expect(reject).toBeEnabled();
    await expect(page.getByText(summarizedReason)).toBeVisible();

    // 投票成功レスポンスを返し、do_vote から fetch_reports を再実行させる。
    let voteRequests = 0;
    await page.route('**/mod/vote', async route => {
      voteRequests += 1;
      await route.fulfill({ status: 200, contentType: 'application/json', body: 'true' });
    });
    serveSuccess = false;
    await approve.click();

    await expect(reportsError).toHaveText(reportsHttpErrorText(500), { timeout: 10000 });
    // 古いカードは維持されるが、取得失敗中は投票ボタンを操作できない。
    await expect(page.getByText(summarizedReason)).toBeVisible();
    await expect(approve).toBeDisabled();
    await expect(reject).toBeDisabled();
    expect(voteRequests).toBe(1);

    // 再マウント後は失敗状態が新しいカード／ボタンへ引き継がれない。
    await openModeration(page);
    await expect(reportsError).toHaveText(reportsHttpErrorText(500), { timeout: 10000 });
    await expect(approve).toHaveCount(0);
    await expect(reject).toHaveCount(0);
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

    // R2-CLIENT-REPORT-REQUIRED: native required は外し、aria-required + 独自 role=alert に統一。
    const cidField = page.locator('input[placeholder="TARGET CID"]');
    const reasonField = page.locator('textarea[placeholder="REASON"]');
    expect(await cidField.getAttribute('required')).toBeNull();
    expect(await reasonField.getAttribute('required')).toBeNull();
    await expect(cidField).toHaveAttribute('aria-required', 'true');
    await expect(reasonField).toHaveAttribute('aria-required', 'true');
    // <form on:submit> の Enter 経路も同じ検証へ集約され、POST は増えない。
    await cidField.fill('');
    await reasonField.fill('Illegal content detected');
    await cidField.press('Enter');
    await expect(validationError).toHaveText('対象 CID を入力してください。');
    await expect(validationError).toHaveAttribute('role', 'alert');
    await expect(validationError).toHaveAttribute('lang', 'ja');
    expect(reportRequests).toEqual([]);

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
    // 失敗時は role="status" の送信中進捗（通報を送信中...）を消し、do_vote と完了処理を揃える。
    // 失敗告知は report-failed の role="alert" に残る。
    const reportStatus = page.locator('p[role="status"]:not([data-testid])');
    await expect(reportStatus).toHaveCount(0, { timeout: 10000 });
    await expect(page.locator('body')).not.toContainText('通報を送信中...');
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
    // ネットワーク失敗でも role="status" は空になり、送信中進捗は残らない。
    const reportStatus = page.locator('p[role="status"]:not([data-testid])');
    await expect(reportStatus).toHaveCount(0, { timeout: 10000 });
    await expect(page.locator('body')).not.toContainText('通報を送信中...');
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
