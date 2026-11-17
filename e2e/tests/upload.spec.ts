import { test, expect } from '@playwright/test';

test.describe('Content Upload Flow', () => {
  test.beforeEach(async ({ page }) => {
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
    await expect(page.getByRole('button', { name: 'UPLOAD' })).toBeVisible({ timeout: 10000 });
  });

  test('should upload JXL content successfully', async ({ page }) => {
    await page.getByRole('button', { name: 'UPLOAD' }).click();
    
    const fileInput = page.locator('input[type="file"]');
    const mockFile = Buffer.from('mock jxl data');
    await fileInput.setInputFiles({
      name: 'test.jxl',
      mimeType: 'image/jxl',
      buffer: mockFile,
    });

    await page.route('**/content/upload', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify('QmXU4R1WzwFggZbSRrrgtL31sQFzHFqEZaydErQUwhCm8e'),
      });
    });

    const submitButton = page.getByRole('button', { name: /Sign & Submit/i });
    await submitButton.click();

    await expect(page.locator('text=SUCCESS: Content registered')).toBeVisible({ timeout: 15000 });
  });

  test('should reject files over 10 MiB before upload', async ({ page }) => {
    await page.getByRole('button', { name: 'UPLOAD' }).click();

    let uploadAttempted = false;
    await page.route('**/content/upload', async route => {
      uploadAttempted = true;
      await route.abort();
    });

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'oversized.jxl',
      mimeType: 'image/jxl',
      buffer: Buffer.alloc(10 * 1024 * 1024 + 1),
    });

    await expect(page.locator('text=ERROR: File exceeds the 10 MiB limit.')).toBeVisible({ timeout: 10000 });

    // 拒否後はファイルが保持されず、送信ボタンは無効になる。
    await expect(page.getByRole('button', { name: /Sign & Submit/i })).toBeDisabled();
    expect(uploadAttempted).toBe(false);
  });

  test('INT-02: should reject ZIP files', async ({ page }) => {
    await page.getByRole('button', { name: 'UPLOAD' }).click();

    await page.route('**/content/upload', async route => {
      await route.fulfill({
        status: 400,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Disallowed content type' }),
      });
    });

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'malicious.zip',
      mimeType: 'application/zip',
      buffer: Buffer.from('PK'),
    });

    const submitButton = page.getByRole('button', { name: /Sign & Submit/i });
    await submitButton.click();

    await expect(page.locator('text=ERROR: Upload failed')).toBeVisible({ timeout: 10000 });
  });
});
