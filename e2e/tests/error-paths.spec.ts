import { test, expect, type Page } from '@playwright/test';

/**
 * エラーパス E2E（監査指摘: network error / HTTP 500 の表示テストが無い）。
 * API 自体の挙動ではなく、クライアントが失敗を画面に表示することを検証する。
 *
 * 期待文言は client/src/lib.rs の実装を根拠にしている。
 * - /content の通信失敗: L761 `Err(_) => set_error.set("Network error loading content.")`
 *   → L881-885 の空一覧プレースホルダーに表示。
 * - /me/export の通信失敗: L914 `.map_err(|_| "ネットワークエラーが発生しました。")`
 *   → L959 の `<p role="alert" data-testid="export-error">` に表示。
 *
 * 以下の2ケースは、ログイン/監査のエラー表示が実装済みのため追加している。
 * - /auth/verify が 500: `<p role="alert" data-testid="login-error">` に
 *   HTTP 500 を含む文言が表示され、UPLOAD は出ない。
 * - /content/{cid}/audit が 500: Audit パネル内の role="alert" にエラーが出る。
 */
test.describe('Error paths (network error / HTTP 500)', () => {
  /** モック認証でログインし、ダッシュボード（BROWSE ナビ）まで進む。 */
  async function loginWithMockAuth(page: Page): Promise<void> {
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

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
  }

  test('DASH-ERR: GET /content が network error のときダッシュボードにエラー表示が出ること', async ({ page }) => {
    let contentAborted = false;
    // 同じケース内で network error → 401 へ切り替える（再ログイン導線の回帰）。
    let unauthorized = false;
    await page.route('**/content', async route => {
      if (unauthorized) {
        await route.fulfill({
          status: 401,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'SERVER_DETAIL_MUST_NOT_BE_SHOWN' }),
        });
        return;
      }
      contentAborted = true;
      await route.abort();
    });

    await loginWithMockAuth(page);

    // 実装文言（CONTENT_INDEX_NETWORK_ERROR）: "Network error loading content."
    // 別の role=alert が増えても strict mode 違反にならないよう testid で特定する。
    const dashboardError = page.getByTestId('dashboard-error');
    await expect(dashboardError).toBeVisible({ timeout: 10000 });
    await expect(dashboardError).toHaveText('Network error loading content.', { timeout: 10000 });
    await expect(dashboardError).toHaveAttribute('role', 'alert');
    // R2-CLIENT-UX: エラー時は role=status が古い件数を読み上げず、
    // 取得失敗の告知へ切り替わる（既存ケース内へ統合、新規ケースなし）。
    const liveStatus = page.locator('[role="status"][aria-live="polite"]');
    await expect(liveStatus).toContainText('コンテンツを取得できませんでした');
    await expect(liveStatus).not.toContainText('件のコンテンツを表示中');
    expect(contentAborted).toBe(true);

    // 追加フェーズ: 401 のときだけ専用文言と LOG OUT 経由の再ログイン案内を出す。
    // 他エラーの固定文言（CONTENT_INDEX_LOAD_ERROR / CONTENT_INDEX_NETWORK_ERROR）は不変。
    unauthorized = true;
    await loginWithMockAuth(page);
    await expect(dashboardError).toBeVisible({ timeout: 10000 });
    await expect(dashboardError).toHaveText('再ログインが必要です。', { timeout: 10000 });
    await expect(dashboardError).toHaveAttribute('role', 'alert');
    await expect(page.getByTestId('dashboard-relogin-hint')).toContainText('LOG OUT');
    await expect(page.getByTestId('logout-button')).toBeVisible();
    await expect(dashboardError).not.toHaveText('Network error loading content.');
    await expect(page.locator('body')).not.toContainText('SERVER_DETAIL_MUST_NOT_BE_SHOWN');
  });

  test('R5-04: GET /me/export が network error のとき role=alert にネットワークエラー文言が出ること', async ({ page }) => {
    // ダッシュボード自身の /content は成功させ、export の失敗だけを切り出す。
    await page.route('**/content', async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify([]) });
    });

    let exportAborted = false;
    await page.route('**/me/export', async route => {
      exportAborted = true;
      await route.abort();
    });

    await loginWithMockAuth(page);
    await page.getByTestId('export-my-data').click();

    // 実装文言（client/src/lib.rs L914）: "ネットワークエラーが発生しました。"
    // 別の role=alert が増えても strict mode 違反にならないよう testid で特定する。
    const exportAlert = page.getByTestId('export-error');
    await expect(exportAlert)
      .toHaveText('ネットワークエラーが発生しました。', { timeout: 10000 });
    await expect(exportAlert).toHaveAttribute('role', 'alert');
    await expect(page.getByTestId('export-status')).toHaveCount(0);
    expect(exportAborted).toBe(true);
  });

  test('AUTH-500: /auth/verify が HTTP 500 のとき login-error に文言が出て UPLOAD が出ないこと', async ({ page }) => {
    await page.route('**/auth/login', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ nonce: 'test-nonce' }),
      });
    });
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'verify failed' }),
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();

    // 実装文言（client/src/lib.rs, Login の verify 失敗時）:
    //   "認証に失敗しました (HTTP 500)."
    const loginAlert = page.getByTestId('login-error');
    await expect(loginAlert)
      .toHaveText('認証に失敗しました (HTTP 500).', { timeout: 10000 });
    await expect(loginAlert).toHaveAttribute('role', 'alert');
    await expect(page.getByRole('button', { name: 'UPLOAD' })).toHaveCount(0);
  });

  test('AUDIT-500: /content/{cid}/audit が HTTP 500 のとき監査パネルに role=alert が出ること', async ({ page }) => {
    const cid = 'QmW6DWQWmUGYuNy9iFErbYjGdorPNjMevh6fgfrbDQqhy2';
    // ダッシュボードに監査対象のカードを1件だけ出す。
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid,
          checksum: 'mock_checksum',
          type: 'Novel',
          status: 'Approved',
          price: 0,
          title: 'Audit Error Fixture',
          description: 'audit 500 fixture',
          tags: ['audit'],
          is_favorite: false,
        }]),
      });
    });
    await page.route(`**/content/${cid}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        body: 'Test content',
      });
    });
    await page.route(`**/content/${cid}/audit`, async route => {
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'audit failed' }),
      });
    });

    await loginWithMockAuth(page);
    await page.getByRole('button', { name: 'BROWSE' }).click();
    await expect(page.getByText('Audit Error Fixture')).toBeVisible({ timeout: 20000 });

    await page.locator('button:has-text("Audit")').first().click();
    await expect(page.locator('text=FORENSIC_TIMELINE')).toBeVisible();

    // 実装文言（client/src/lib.rs, ContentViewer の audit 取得失敗時）:
    //   "監査ログの取得に失敗しました (HTTP 500)."
    // 監査パネルの <div> 自体には data-testid が無く、文言で絞り込むと祖先要素も
    // 一致するため .last() に依存していた。パネル内で一意な
    // <p role="alert" data-testid="audit-error"> を直接特定する（client/src/lib.rs
    // は担当外のため、パネル側に testid は追加しない）。
    const auditAlert = page.getByTestId('audit-error');
    await expect(auditAlert)
      .toHaveText('監査ログの取得に失敗しました (HTTP 500).', { timeout: 10000 });
    await expect(auditAlert).toHaveAttribute('role', 'alert');
    await expect(auditAlert).toBeVisible();
  });

  /**
   * CONTENT-ERR: /content/{cid} の HTTP エラー（404 / 500）。
   * 実装（content_load_error_message / data-testid="content-load-error"）は role="alert" の content-load-error に
   * "Content Load Failed" を、content-load-error-detail に固定文言を出す。
   * verified=false でも取得失敗が優先され、バッジは "× LOAD FAILED"、
   * "Integrity Breach Detected" は出ない（:1309-1323, :1591-1594, :1636-1645）。
   * サーバーエラー本文は画面へ出さない。
   */
  const CONTENT_ERR_CID = 'QmYwAPJzv5CZsnAztHZk3RQ3m2YgFf9pRnvNsYQ7nBTeS';

  async function setupContentHttpErrorFixture(page: Page, status: 404 | 500): Promise<void> {
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: CONTENT_ERR_CID,
          checksum: 'mock_checksum',
          type: 'Novel',
          status: 'Approved',
          price: 0,
          title: 'Content Load Error Fixture',
          description: 'content http error fixture',
          tags: ['error'],
          is_favorite: false,
        }]),
      });
    });
    await page.route(`**/content/${CONTENT_ERR_CID}`, async route => {
      await route.fulfill({
        status,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'SERVER_DETAIL_MUST_NOT_BE_SHOWN' }),
      });
    });
    await page.route(`**/content/${CONTENT_ERR_CID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });
    await loginWithMockAuth(page);
    await expect(page.getByText('Content Load Error Fixture')).toBeVisible({ timeout: 20000 });
  }

  async function expectContentLoadError(page: Page, status: 404 | 500): Promise<void> {
    const loadError = page.getByTestId('content-load-error');
    await expect(loadError).toBeVisible({ timeout: 10000 });
    await expect(loadError).toHaveAttribute('role', 'alert');
    await expect(loadError).toHaveText('Content Load Failed');
    await expect(page.getByTestId('content-load-error-detail'))
      .toHaveText(`コンテンツの取得に失敗しました (HTTP ${status}).`);

    const badge = page.getByTestId('content-verification-badge');
    await expect(badge).toHaveText('× LOAD FAILED', { timeout: 10000 });
    // 上のバッジ文言と load-error 詳細の肯定アサーションで取得失敗状態が確定済み。
    // 否定は expect.poll に置き、描画前の瞬間に成立する単発 toHaveCount(0) を避ける。
    await expect(badge).not.toContainText('COMPROMISED');
    await expect
      .poll(() => page.getByText('Integrity Breach Detected').count(), { timeout: 5000 })
      .toBe(0);
    // HTTP エラー本文（サーバー詳細）を画面へ出さない。
    await expect
      .poll(() => page.getByText('SERVER_DETAIL_MUST_NOT_BE_SHOWN').count(), { timeout: 5000 })
      .toBe(0);
  }

  test('CONTENT-ERR-404: /content/{cid} が HTTP 404 のとき取得失敗文言を出し改ざん検知と誤表示しないこと', async ({ page }) => {
    await setupContentHttpErrorFixture(page, 404);
    await expectContentLoadError(page, 404);
  });

  test('CONTENT-ERR-500: /content/{cid} が HTTP 500 のとき取得失敗文言を出し改ざん検知と誤表示しないこと', async ({ page }) => {
    await setupContentHttpErrorFixture(page, 500);
    await expectContentLoadError(page, 500);
  });
});
