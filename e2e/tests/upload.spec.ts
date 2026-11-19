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

    await expect(page.locator('text=Failed to upload content (HTTP 400).')).toBeVisible({ timeout: 10000 });
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

    // ローディング文言のまま固まらず、ボタンが再活性化されること。
    await expect(submitButton).toBeEnabled({ timeout: 10000 });
    await expect(submitButton).not.toHaveText(/処理中/);
  });

  test('R2: file-upload label is keyboard operable and price input has bounds', async ({ page }) => {
    await page.getByRole('button', { name: 'UPLOAD' }).click();

    const label = page.locator('label[for="file-upload"]');
    await expect(label).toBeVisible();
    await expect(label).toHaveAttribute('tabindex', '0');

    // ラベルの keydown ハンドラが input.click() を呼び、filechooser が発火する（Enter ケースは維持）。
    // 待機自体は waitForEvent のタイムアウトで失敗するため、ここでは発火した chooser が
    // 実際に #file-upload へファイルを設定できることをオラクルにする（単純な toBeTruthy はしない）。
    const fileChooserPromise = page.waitForEvent('filechooser', { timeout: 10000 });
    await label.focus();
    await page.keyboard.press('Enter');
    const fileChooser = await fileChooserPromise;
    await fileChooser.setFiles({
      name: 'keyboard-enter.jxl',
      mimeType: 'image/jxl',
      buffer: Buffer.from('keyboard enter file'),
    });
    expect(
      await page
        .locator('#file-upload')
        .evaluate(input => Array.from((input as HTMLInputElement).files ?? []).map(file => file.name)),
    ).toEqual(['keyboard-enter.jxl']);

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
    await spaceChooser.setFiles({
      name: 'keyboard-space.jxl',
      mimeType: 'image/jxl',
      buffer: Buffer.from('keyboard space file'),
    });
    expect(
      await page
        .locator('#file-upload')
        .evaluate(input => Array.from((input as HTMLInputElement).files ?? []).map(file => file.name)),
    ).toEqual(['keyboard-space.jxl']);
    expect(
      await page.evaluate(
        () => (window as unknown as { __spaceDefaultPrevented: boolean }).__spaceDefaultPrevented,
      ),
    ).toBe(true);

    const priceInput = page.locator('#upload-price');
    await expect(priceInput).toHaveAttribute('min', '1');
    await expect(priceInput).toHaveAttribute('max', '1000000');
  });

  test('R2 (price boundary): 境界値をモック応答で送信し、丸めずに送って上限超過のみ拒否される', async ({ page }) => {
    await page.getByRole('button', { name: 'UPLOAD' }).click();

    const submittedPrices: string[] = [];
    await page.route('**/content/upload', async route => {
      // multipart 本文から price フィールドを取り出し、クライアントが入力値を
      // 丸めていないことを後で検証する。latin1 でバイト列を保持し ASCII を確実に読む。
      const body = (route.request().postDataBuffer() ?? Buffer.from(route.request().postData() ?? '')).toString('latin1');
      const price = /name="price"\r?\n\r?\n([^\r\n]*)/.exec(body)?.[1] ?? '';
      submittedPrices.push(price);

      // ここはモック応答による送信。実サーバー（server/src/lib.rs:3472-3483,
      // payments.rs:26）と同じ境界をテスト内で再現しているだけなので、サーバー側の
      // 境界変更は検出できない。<0 と >1000000 のみ拒否し、0 と 1000000 は受理する。
      const numeric = Number(price);
      if (!Number.isFinite(numeric) || numeric < 0 || numeric > 1000000) {
        await route.fulfill({
          status: 400,
          contentType: 'application/json',
          body: JSON.stringify({ error: 'price must not exceed 1000000' }),
        });
      } else {
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify('QmXU4R1WzwFggZbSRrrgtL31sQFzHFqEZaydErQUwhCm8e'),
        });
      }
    });

    const fileInput = page.locator('input[type="file"]');
    await fileInput.setInputFiles({
      name: 'price-boundary.jxl',
      mimeType: 'image/jxl',
      buffer: Buffer.from('mock jxl data'),
    });

    const priceInput = page.locator('#upload-price');
    const submitButton = page.getByRole('button', { name: /Sign & Submit/i });
    const status = page.getByTestId('upload-status');

    // 0: parse().unwrap_or(0) の既定値と一致し、モックの下限（<0 のみ拒否）でも受理。
    await priceInput.fill('0');
    await submitButton.click();
    await expect(status).toContainText('SUCCESS: Content registered', { timeout: 15000 });

    // 1000001: 上限超過はモック応答で拒否され、固定の失敗文言のみ表示。
    await priceInput.fill('1000001');
    await submitButton.click();
    await expect(status).toContainText('Failed to upload content (HTTP 400).', { timeout: 15000 });
    await expect(status).not.toContainText('price must not exceed');

    // 1000000: 上限境界は受理。
    await priceInput.fill('1000000');
    await submitButton.click();
    await expect(status).toContainText('SUCCESS: Content registered', { timeout: 15000 });

    // クライアントが min/max 属性で値を丸めず、入力値をそのまま price として送信すること。
    expect(submittedPrices).toEqual(['0', '1000001', '1000000']);
  });
});
