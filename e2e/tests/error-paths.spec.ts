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
 * 以下の2ケースは、現行実装にエラー表示が無いためテストを追加しない
 * （コード変更は担当外）。詳細はエージェントの報告を参照。
 * - /auth/verify が 500: Login コンポーネント（L664-720）は loading を戻すだけで
 *   エラー signal / role="alert" を持たない。UPLOAD が出ないことは確認できるが
 *   「エラーが表示される」ことを検証できない。
 * - /content/{cid}/audit が 500: ContentViewer（L1081-1088）は if let の
 *   失敗時に何も表示せず、Audit パネルには空の FORENSIC_TIMELINE が出るだけ。
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
});
