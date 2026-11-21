import { test, expect, type Page, type Route } from '@playwright/test';
import * as crypto from 'crypto';

test.describe('Support & Ledger Flow (ECO-01)', () => {
  const computeHash = (data: string | Buffer) => {
    return crypto.createHash('sha256').update(data).digest('hex');
  };

  test('should keep mock support disabled and the ledger read-only', async ({ page }) => {
    const mockCID = 'QmXgZ73Cr8k75MwkR9e3MwP94bnFx1bgvRRdA9rfd7D6JU';
    const mockPrice = 500;
    const mockContent = Buffer.from('fake-jxl-data');
    const mockHash = computeHash(mockContent);
    let supportRequested = false;

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

    // 1. Mock content with price
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: mockCID,
          checksum: mockHash,
          // 支援フローの検証であり JXL デコードは本質ではないため、
          // デコーダを呼ばない Novel をフィクスチャとして使う（R1-13 の表示変更対応）。
          type: 'Novel',
          status: 'Approved',
          price: mockPrice,
          title: 'Gilded Sigil',
          description: 'Mock content for the ledger flow',
          tags: ['novel', 'mock'],
          is_favorite: false
        }])
      });
    });

    // 2. Mock content fetch (verified)
    await page.route(`**/content/${mockCID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        headers: { 'x-forensic-checksum': mockHash },
        body: mockContent
      });
    });

    // 3. Mock audit logs (empty for this test)
    await page.route(`**/content/${mockCID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });

    // 4. Support API must never be called while the mock endpoint is disabled
    await page.route(`**/content/${mockCID}/support`, async route => {
      supportRequested = true;
      await route.fulfill({
        status: 501,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Support API is disabled until JPYC settlement is implemented' })
      });
    });

    // 5. Mock transactions for ledger
    await page.route('**/transactions', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          tx_id: 1,
          cid: mockCID,
          sender_hash: 'SENDER_HASH_123456789',
          receiver_hash: 'RECEIVER_HASH_987654321',
          amount: mockPrice,
          tx_hash: 'mock_tx_hash_789012345678',
          created_at: '2026-06-10T12:00:00Z'
        }])
      });
    });

    await page.goto('/');

    // R2 a11y: 先頭のスキップリンクは 1 回目の Tab で可視化され、
    // Enter で main へ直接フォーカスする（hash は view 同期と衝突するため書き換えない）。
    const skipLink = page.getByRole('link', { name: '本文へスキップ' });
    await expect(skipLink).toBeAttached();
    await page.keyboard.press('Tab');
    await expect(skipLink).toBeFocused();
    // sr-only（1px）ではなく focus:not-sr-only で実サイズへ広がったことを検証する。
    const skipBox = await skipLink.boundingBox();
    expect(skipBox).not.toBeNull();
    expect(skipBox!.width).toBeGreaterThan(20);
    expect(skipBox!.height).toBeGreaterThan(10);
    await page.keyboard.press('Enter');
    await expect(page.locator('#main-content')).toBeFocused();

    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'BROWSE' }).click();

    // 支援オーバーレイは group-focus-within:opacity-100 でも表示される。
    // Tailwind の .group に依存せず、クライアント側の testid で特定する。
    const contentArea = page.getByTestId('content-card').first();
    await expect(contentArea).toBeVisible({ timeout: 20000 });
    const supportOverlay = page.getByTestId('support-overlay');
    await supportOverlay.focus();
    // フォーカスがオーバーレイへ実際に当たったことを activeElement で確認する。
    expect(
      await page.evaluate(() => (document.activeElement as HTMLElement | null)?.getAttribute('data-testid'))
    ).toBe('support-overlay');

    const pendingButton = page.getByRole('button', { name: 'Settlement Pending' });
    // オーバーレイは opacity-0 → group-hover / group-focus-within:opacity-100 の遷移がある。
    // toBeVisible() は opacity:0 でも通るため、親のオーバーレイが完全に
    // 表示される（opacity:1）まで待ってからボタンを検証する。
    await expect(pendingButton.locator('xpath=..')).toHaveCSS('opacity', '1');
    await expect(pendingButton).toBeVisible();
    await expect(pendingButton).toBeDisabled();
    await expect(page.locator('text=JPYC settlement verification is in development')).toBeVisible();
    expect(supportRequested).toBe(false);

    // The ledger stays readable for legacy mock records, labeled as such
    await page.getByRole('button', { name: 'LEDGER' }).click();
    await expect(page.locator('text=Forensic Transaction Ledger')).toBeVisible();
    await expect(page.locator('text=LEGACY MOCK RECORDS')).toBeVisible();
    await expect(page.locator('text=Record_Hash')).toBeVisible();
    await expect(page.locator('text=500 JPYC')).toBeVisible();
    await expect(page.locator('text=mock_tx_hash_789012345678')).toBeVisible();

    // R2-URL: タブ切替は pushState なので、戻るで直前の BROWSE が復元される。
    expect(new URL(page.url()).hash).toBe('#ledger');
    await page.goBack();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toHaveAttribute(
      'aria-current',
      'page',
    );
    await expect(page.getByTestId('export-section')).toBeVisible();
  });

  type TransactionsHandler = (route: Route) => Promise<void>;

  // 台帳の取得失敗ケース用の共通セットアップ。一覧は空にしてカード依存を排除し、
  // LEDGER タブを開いた時点で /transactions が発行されるようにする。
  async function setupLedgerFailurePage(page: Page, handleTransactions: TransactionsHandler) {
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
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });
    await page.route('**/transactions', handleTransactions);

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'LEDGER' }).click();

    return {
      error: page.getByTestId('ledger-error'),
      noTransactions: page.getByText('NO_TRANSACTIONS_RECORDED'),
    };
  }

  test('ledger shows a fixed error and no empty-state on HTTP 500', async ({ page }) => {
    const { error, noTransactions } = await setupLedgerFailurePage(page, async route => {
      await route.fulfill({
        status: 500,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'ledger unavailable' }),
      });
    });
    await expect(error).toHaveAttribute('role', 'alert');
    await expect(error).toContainText('取引履歴の取得に失敗しました。');
    await expect(noTransactions).toHaveCount(0);
  });

  test('ledger shows a fixed error and no empty-state on network abort', async ({ page }) => {
    const { error, noTransactions } = await setupLedgerFailurePage(page, async route => {
      await route.abort();
    });
    await expect(error).toHaveAttribute('role', 'alert');
    await expect(error).toContainText('取引履歴の取得に失敗しました。');
    await expect(noTransactions).toHaveCount(0);
  });
});
