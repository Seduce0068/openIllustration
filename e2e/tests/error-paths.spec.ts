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
    await page.route('**/content', async route => {
      contentAborted = true;
      await route.abort();
    });

    await loginWithMockAuth(page);

    // 実装文言（client/src/lib.rs L761）: "Network error loading content."
    await expect(page.getByText('Network error loading content.', { exact: true }))
      .toBeVisible({ timeout: 10000 });
    expect(contentAborted).toBe(true);
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
    await expect(page.getByTestId('export-error'))
      .toHaveText('ネットワークエラーが発生しました。', { timeout: 10000 });
    await expect(page.getByRole('alert')).toHaveText('ネットワークエラーが発生しました。');
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
    await expect(page.getByTestId('login-error'))
      .toHaveText('認証に失敗しました (HTTP 500).', { timeout: 10000 });
    await expect(page.getByRole('alert'))
      .toHaveText('認証に失敗しました (HTTP 500).');
    await expect(page.getByRole('button', { name: 'UPLOAD' })).toHaveCount(0);
  });

  test('AUDIT-500: /content/{cid}/audit が HTTP 500 のとき監査パネルに role=alert が出ること', async ({ page }) => {
    const cid = 'QmErrorPathsAudit500';
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
    // FORENSIC_TIMELINE を持つ監査パネル内の role="alert" を検証する。
    const auditPanel = page.locator('div').filter({ hasText: 'FORENSIC_TIMELINE' }).last();
    const auditAlert = auditPanel.getByRole('alert');
    await expect(auditAlert)
      .toHaveText('監査ログの取得に失敗しました (HTTP 500).', { timeout: 10000 });
    await expect(auditPanel.getByTestId('audit-error')).toBeVisible();
  });
});
