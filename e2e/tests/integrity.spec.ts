import { test, expect } from '@playwright/test';
import * as crypto from 'crypto';

test.describe('Forensic Integrity Verification (INT-01)', () => {
  const MOCK_CID = 'QmTFfE7GUfHHzrJDcUQycwRqHhxMCJ2Z1nJ8cqu2uoerhp';
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
          price: 0,
          title: 'Forensic Evidence Log',
          description: 'Integrity verification fixture',
          tags: ['forensic', 'test'],
          is_favorite: false
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
          price: 0,
          title: 'Forensic Evidence Log',
          description: 'Integrity verification fixture',
          tags: ['forensic', 'test'],
          is_favorite: false
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

  test('should not mark VERIFIED when checksum matches but JXL decoding fails (R1-13)', async ({ page }) => {
    // チェックサムは一致するが JXL コーデックストリームとして不正なバイト列を返すモックゲートウェイ。
    // HTTP 200・チェックサム一致でも、デコードに失敗した場合は VERIFIED にしないことを検証する。
    const invalidJxl = 'not a jxl codestream';
    const hash = computeHash(invalidJxl);
    const gatewayStatuses: number[] = [];

    page.on('response', response => {
      if (response.url().endsWith(`/content/${MOCK_CID}`)) {
        gatewayStatuses.push(response.status());
      }
    });

    // 1. コンテンツインデックスのモック（ログイン前に登録）
    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: MOCK_CID,
          checksum: hash,
          type: 'Illustration',
          status: 'Approved',
          price: 0,
          title: 'Invalid JXL Evidence',
          description: 'Checksum matches but the codestream is invalid',
          tags: ['forensic', 'jxl'],
          is_favorite: false
        }])
      });
    });

    // 2. 不正な JXL バイト列を 200 で返す（チェックサムは一致）
    await page.route(`**/content/${MOCK_CID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/octet-stream',
        headers: {
          'x-forensic-checksum': hash,
          'content-type': 'application/octet-stream'
        },
        body: invalidJxl
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

    // デコード失敗表示は検証完了後に出るため、詳細文言の出現を待つ。
    await expect(page.getByTestId('jxl-decode-error-detail')).toBeVisible({ timeout: 30000 });

    // 実装側と確定した文言・testid（client/src/lib.rs の decode_error 分岐）。
    await expect(page.getByTestId('content-verification-error')).toBeVisible();
    // role="alert" はライブリージョンでアクセシブル名が本文から決まらないため、
    // 名前ではなく role 属性と testid の両方をセレクタで確認する。
    await expect(page.locator('[data-testid="jxl-decode-error"][role="alert"]')).toBeVisible();
    await expect(page.getByTestId('jxl-decode-error')).toHaveText('JXL Decode Failed');
    await expect(page.getByTestId('jxl-decode-error-detail')).toHaveText('JXL のデコードに失敗しました。');
    await expect(page.getByTestId('content-verification-badge')).toHaveText('× DECODE FAILED');

    // R1-13 の回帰: デコード失敗時に VERIFIED や改ざん検知文言が出ないこと。
    await expect(page.locator('span', { hasText: '● VERIFIED' })).toHaveCount(0);
    await expect(page.getByText('Integrity Breach Detected')).toHaveCount(0);
    await expect(page.getByText('Wasm checksum does not match forensic registry.')).toHaveCount(0);

    // ゲートウェイ取得は 200 で 1 回だけ（チェックサム一致によるデコード試行）。
    expect(gatewayStatuses).toEqual([200]);
  });
});
