import { test, expect } from '@playwright/test';
import * as crypto from 'crypto';

test.describe('Forensic Integrity Verification (INT-01)', () => {
  const MOCK_CID = 'QmTestForensicIntegrity12345';
  const ORIGINAL_CONTENT = 'This is a genuine forensic evidence content.';
  const TAMPERED_CONTENT = 'This is a TAMPERED forensic evidence content.';
  
  // SHA-256ハッシュの生成ヘルパー
  const computeHash = (data: string) => {
    return crypto.createHash('sha256').update(data).digest('hex');
  };

  test('should verify genuine content successfully', async ({ page }) => {
    const hash = computeHash(ORIGINAL_CONTENT);

    // 1. コンテンツインデックスのモック (ログイン前に登録)
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: MOCK_CID,
          checksum: hash,
          type: 'Novel',
          status: 'Approved',
          price: 0
        }])
      });
    });

    // 2. コンテンツ実体のモック
    await page.route(`**/content/${MOCK_CID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        headers: {
          'x-forensic-checksum': hash,
          'content-type': 'text/markdown'
        },
        body: ORIGINAL_CONTENT
      });
    });

    await page.route(`**/content/${MOCK_CID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });

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

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    
    // 検証は非同期で即完了し得るため、中間状態の PENDING は待たない。
    // ダッシュボードで最終的な「VERIFIED」が表示されることを確認する。
    const verifiedBadge = page.locator('span', { hasText: '● VERIFIED' });
    await expect(verifiedBadge).toBeVisible({ timeout: 30000 });
  });

  test('should detect and block tampered content', async ({ page }) => {
    const originalHash = computeHash(ORIGINAL_CONTENT);

    // 1. コンテンツインデックスのモック (ログイン前に登録)
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: MOCK_CID,
          checksum: originalHash,
          type: 'Novel',
          status: 'Approved',
          price: 0
        }])
      });
    });

    // 2. コンテンツ実体のモック
    await page.route(`**/content/${MOCK_CID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        headers: {
          'x-forensic-checksum': originalHash,
          'content-type': 'text/markdown'
        },
        body: TAMPERED_CONTENT
      });
    });

    await page.route(`**/content/${MOCK_CID}/audit`, async route => {
      await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
    });

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

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    
    // 検証は非同期で即完了し得るため、中間状態の PENDING は待たない。
    // 「COMPROMISED」が表示されることを確認
    const compromisedBadge = page.locator('span', { hasText: '× COMPROMISED' });
    await expect(compromisedBadge).toBeVisible({ timeout: 30000 });
    
    // 詳細メッセージの確認
    await expect(page.locator('text=Integrity Breach Detected')).toBeVisible();
    await expect(page.locator('text=Wasm checksum does not match forensic registry.')).toBeVisible();
  });
});
