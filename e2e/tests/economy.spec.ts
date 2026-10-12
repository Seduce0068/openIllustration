import { test, expect } from '@playwright/test';
import * as crypto from 'crypto';

test.describe('Support & Ledger Flow (ECO-01)', () => {
  const computeHash = (data: string | Buffer) => {
    return crypto.createHash('sha256').update(data).digest('hex');
  };

  test('should support creator and see transaction in ledger', async ({ page }) => {
    const mockCID = 'QmSupportTest123';
    const mockPrice = 500;
    const mockContent = Buffer.from('fake-jxl-data');
    const mockHash = computeHash(mockContent);

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
          type: 'Illustration',
          status: 'Approved',
          price: mockPrice
        }])
      });
    });

    // 2. Mock content fetch (verified)
    await page.route(`**/content/${mockCID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'image/jxl',
        headers: { 'x-forensic-checksum': mockHash },
        body: mockContent
      });
    });

    // 3. Mock audit logs (empty for this test)
    await page.route(`**/content/${mockCID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });

    // 4. Mock support API
    await page.route(`**/content/${mockCID}/support`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify('mock_tx_hash_789012345678')
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
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'BROWSE' }).click();
    
    // Hover over content to see support button
    const contentArea = page.locator('.group').first();
    await expect(contentArea).toBeVisible({ timeout: 20000 });
    await contentArea.hover();
    
    const supportButton = page.getByRole('button', { name: 'Confirm Support' });
    await expect(supportButton).toBeVisible();
    await supportButton.click();

    await expect(page.locator('text=SUCCESS: Tx recorded')).toBeVisible({ timeout: 10000 });

    // Go to Ledger
    await page.getByRole('button', { name: 'LEDGER' }).click();
    await expect(page.locator('text=Forensic Transaction Ledger')).toBeVisible();
    await expect(page.locator('text=500 JPYC')).toBeVisible();
    await expect(page.locator('text=mock_tx_hash_789012345678')).toBeVisible();
  });
});
