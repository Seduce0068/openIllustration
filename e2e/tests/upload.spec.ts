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
    // BROWSE ビューの一覧件数 sr-only ライブリージョン（UPLOAD へ遷移する前に検証）。
    // 実装: client/src/lib.rs:1177 <p role="status" aria-live="polite" class="sr-only">。
    const itemCountStatus = page.locator('p.sr-only[role="status"]');
    await expect(itemCountStatus).toHaveCount(1);
    await expect(itemCountStatus).toHaveAttribute('role', 'status');
    await expect(itemCountStatus).toHaveAttribute('aria-live', 'polite');
    await expect(itemCountStatus).toContainText('content items displayed');

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

  test('R1-14: shows an error and re-enables the button when arrayBuffer() rejects', async ({ page }) => {
    await page.getByRole('button', { name: 'UPLOAD' }).click();

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'broken.jxl',
      mimeType: 'image/jxl',
      buffer: Buffer.from('mock jxl data'),
    });

    // wasm-bindgen が呼ぶ Blob.prototype.arrayBuffer を失敗させ、
    // 準備フェーズの unwrap パニック（Processing... 固まり）を再現する。
    await page.evaluate(() => {
      Blob.prototype.arrayBuffer = function (): Promise<ArrayBuffer> {
        return Promise.reject(new Error('forced arrayBuffer failure'));
      };
    });

    const submitButton = page.getByRole('button', { name: /Sign & Submit/i });
    await submitButton.click();

    const status = page.getByTestId('upload-status');
    await expect(status).toContainText('アップロードの準備に失敗しました。', { timeout: 15000 });

    // ライブリージョン属性を明示的に検証（client/src/lib.rs:1900）。
    await expect(status).toHaveAttribute('role', 'status');
    await expect(status).toHaveAttribute('aria-live', 'polite');

    // "Processing..." のまま固まらず、ボタンが再活性化されること。
    await expect(submitButton).toBeEnabled({ timeout: 10000 });
    await expect(submitButton).not.toHaveText(/Processing/i);
  });

  test('R2: file-upload label is keyboard operable and price input has bounds', async ({ page }) => {
    await page.getByRole('button', { name: 'UPLOAD' }).click();

    const label = page.locator('label[for="file-upload"]');
    await expect(label).toBeVisible();
    await expect(label).toHaveAttribute('tabindex', '0');

    // ラベルの keydown ハンドラが input.click() を呼び、filechooser が発火する（Enter ケースは維持）。
    const fileChooserPromise = page.waitForEvent('filechooser', { timeout: 10000 });
    await label.focus();
    await page.keyboard.press('Enter');
    const fileChooser = await fileChooserPromise;
    expect(fileChooser).toBeTruthy();

    // Space (' ') でも filechooser が発火することを検証（client/src/lib.rs:1681-1683, 1823-1825）。
    // is_file_picker_activation_key(" ") が ev.prevent_default() を呼ぶため、
    // Space 既定のページスクロールは抑制される。バブル phase の window リスナーで
    // defaultPrevented を記録し、レイアウトに依存せず決定論的に確認する。
    await page.evaluate(() => {
      const w = window as unknown as { __spaceDefaultPrevented: boolean | null };
      w.__spaceDefaultPrevented = null;
      window.addEventListener('keydown', event => {
        if (event.key === ' ') {
          w.__spaceDefaultPrevented = event.defaultPrevented;
        }
      });
    });
    const spaceChooserPromise = page.waitForEvent('filechooser', { timeout: 10000 });
    await label.focus();
    await page.keyboard.press('Space');
    const spaceChooser = await spaceChooserPromise;
    expect(spaceChooser).toBeTruthy();
    expect(
      await page.evaluate(
        () => (window as unknown as { __spaceDefaultPrevented: boolean }).__spaceDefaultPrevented,
      ),
    ).toBe(true);

    const priceInput = page.locator('#upload-price');
    await expect(priceInput).toHaveAttribute('min', '1');
    await expect(priceInput).toHaveAttribute('max', '1000000');
  });
});
