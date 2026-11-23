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
    await expect(page.getByRole('link', { name: 'UPLOAD' })).toBeVisible({ timeout: 10000 });
  });

  test('should upload JXL content successfully', async ({ page }) => {
    // BROWSE ビューの一覧件数 sr-only ライブリージョン（UPLOAD へ遷移する前に検証）。
    // 実装: Dashboard の件数ライブリージョン <p role="status" aria-live="polite" class="sr-only">。
    const itemCountStatus = page.locator('p.sr-only[role="status"]');
    await expect(itemCountStatus).toHaveCount(1);
    await expect(itemCountStatus).toHaveAttribute('role', 'status');
    await expect(itemCountStatus).toHaveAttribute('aria-live', 'polite');
    await expect(itemCountStatus).toContainText('件のコンテンツを表示中');

    await page.getByRole('link', { name: 'UPLOAD' }).click();
    
    const fileInput = page.locator('input[type="file"]');
    const mockFile = Buffer.from('mock jxl data');
    await fileInput.setInputFiles({
      name: 'test.jxl',
      mimeType: 'image/jxl',
      buffer: mockFile,
    });

    // 送信中フェーズを観測するため、レスポンスを明示的に解放するまで保留する。
    let releaseUpload: () => void = () => {};
    const uploadHeld = new Promise<void>(resolve => { releaseUpload = resolve; });
    let uploadRequestCount = 0;
    await page.route('**/content/upload', async route => {
      uploadRequestCount += 1;
      await uploadHeld;
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify('QmXU4R1WzwFggZbSRrrgtL31sQFzHFqEZaydErQUwhCm8e'),
      });
    });

    const submitButton = page.getByTestId('upload-submit');
    // 送信前は aria-busy="false"（UPLOAD ビューで唯一の aria-busy 標的になる）。
    await expect(submitButton).toHaveAttribute('aria-busy', 'false');
    await submitButton.click();

    // 送信中: aria-busy="true" になり、ボタンは disabled になる。
    await expect(submitButton).toHaveAttribute('aria-busy', 'true', { timeout: 10000 });
    await expect(submitButton).toBeDisabled();
    // R2-BUSY: レスポンス保留中に disabled を迂回した2回目の click イベントを
    // dispatchEvent で届かせても、claim_submission が同期で排除する。
    await submitButton.dispatchEvent('click');
    expect(uploadRequestCount).toBe(1);
    releaseUpload();

    // 完了: aria-busy="false" に戻り、ボタンが再活性化する。
    await expect(submitButton).toHaveAttribute('aria-busy', 'false', { timeout: 10000 });
    await expect(submitButton).toBeEnabled({ timeout: 10000 });
    await expect(page.locator('text=SUCCESS: Content registered')).toBeVisible({ timeout: 15000 });
    // R2-BUSY: ガードが無い場合は2本目が到達するため、完了後も1本であることを固定する。
    await expect.poll(() => uploadRequestCount).toBe(1);
  });

  test('should reject files over 10 MiB before upload', async ({ page }) => {
    await page.getByRole('link', { name: 'UPLOAD' }).click();

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
    // 英文の固定エラーには error_lang 経由で lang="en" が付与される（SEC-01/ID-06）。
    await expect(page.getByTestId('upload-failed')).toHaveAttribute('lang', 'en');

    // 拒否後はファイルが保持されず、送信ボタンは無効になる。
    await expect(page.getByRole('button', { name: /Sign & Submit/i })).toBeDisabled();
    expect(uploadAttempted).toBe(false);

    // サイズ拒否時に input.value がクリアされ、同じファイルを再選択できること。
    await expect(fileInput).toHaveValue('');
    await fileInput.setInputFiles({
      name: 'retry.jxl',
      mimeType: 'image/jxl',
      buffer: Buffer.from('mock jxl data'),
    });
    await expect(page.getByTestId('upload-failed')).toHaveCount(0);
    await expect(page.getByRole('button', { name: /Sign & Submit/i })).toBeEnabled();
    expect(uploadAttempted).toBe(false);
  });

  test('INT-02: should reject ZIP files', async ({ page }) => {
    await page.getByRole('link', { name: 'UPLOAD' }).click();

    // R2-ACCEPT: file input に受理 MIME/拡張子の accept 属性を宣言する。
    // 禁止ファイルはブラウザのピッカー段階で弾むが、setInputFiles は accept を
    // 迂回して直接ファイルを設定できるため、送信前ゲートも併せて検証する。
    const fileInput = page.locator('input[type="file"]');
    await expect(fileInput).toHaveAttribute(
      'accept',
      '.jxl,.md,.markdown,.txt,image/jxl,text/markdown',
    );

    let uploadAttempted = false;
    await page.route('**/content/upload', async route => {
      uploadAttempted = true;
      await route.fulfill({
        status: 400,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Disallowed content type' }),
      });
    });

    await fileInput.setInputFiles({
      name: 'malicious.zip',
      mimeType: 'application/zip',
      buffer: Buffer.from('PK'),
    });

    const submitButton = page.getByRole('button', { name: /Sign & Submit/i });
    await submitButton.click();

    // 送信前ゲートが dedicated role=alert の固定文言を出し、HTTP 400 へ遠回りしない。
    const failed = page.getByTestId('upload-failed');
    await expect(failed).toBeVisible({ timeout: 10000 });
    await expect(failed).toHaveAttribute('role', 'alert');
    await expect(failed).toContainText('ERROR: Unsupported file type. Select a JXL or Markdown file.');
    expect(uploadAttempted).toBe(false);
  });

  test('R1-14: shows an error and re-enables the button when arrayBuffer() rejects', async ({ page }) => {
    await page.getByRole('link', { name: 'UPLOAD' }).click();

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
    const failed = page.getByTestId('upload-failed');
    // 失敗は dedicated role="alert" の upload-failed に既存固定文言で表示される。
    await expect(failed).toContainText('アップロードの準備に失敗しました。', { timeout: 15000 });
    await expect(failed).toHaveAttribute('role', 'alert');

    // 既存の status ライブリージョン検証は維持（成功・進捗は status のまま）。
    await expect(status).toHaveAttribute('role', 'status');
    await expect(status).toHaveAttribute('aria-live', 'polite');

    // ローディング文言のまま固まらず、ボタンが再活性化されること。
    await expect(submitButton).toBeEnabled({ timeout: 10000 });
    await expect(submitButton).not.toHaveText(/処理中/);
  });

  test('R2: file-upload label is keyboard operable and price input has bounds', async ({ page }) => {
    await page.getByRole('link', { name: 'UPLOAD' }).click();

    const label = page.locator('label[for="file-upload"]');
    await expect(label).toBeVisible();
    await expect(label).toHaveAttribute('tabindex', '0');
    // R2-CLIENT-UX: ファイル選択はラベル側だけをタブ停止点にする（新規ケースなし）。
    await expect(page.locator('#file-upload')).toHaveAttribute('tabindex', '-1');
    await expect(page.locator('#file-upload')).toHaveAttribute('aria-hidden', 'true');
    await expect(label).toHaveClass(/focus-visible:outline/);
    // ラベルへフォーカスして Tab を1回押すと次の停止点（価格入力）へ移ること。
    // すなわち input がタブ停止点として介在しないことをオラクルにする。
    await label.focus();
    await expect(label).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(page.locator('#upload-price')).toBeFocused();

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

    // Space (' ') でも filechooser が発火することを検証（is_file_picker_activation_key の Space 受理）。
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
    await expect(priceInput).toHaveAttribute('min', '0');
    await expect(priceInput).toHaveAttribute('max', '1000000');
  });

  test('R2 (price boundary): 送信前ゲートで範囲外を拒否し、0/1000000 を丸めずに送信する', async ({ page }) => {
    await page.getByRole('link', { name: 'UPLOAD' }).click();

    const submittedPrices: string[] = [];
    let uploadRouteCalls = 0;
    await page.route('**/content/upload', async route => {
      uploadRouteCalls += 1;
      // multipart 本文から price フィールドを取り出し、クライアントが入力値を
      // 丸めていないことを後で検証する。latin1 でバイト列を保持し ASCII を確実に読む。
      const body = (route.request().postDataBuffer() ?? Buffer.from(route.request().postData() ?? '')).toString('latin1');
      const price = /name="price"\r?\n\r?\n([^\r\n]*)/.exec(body)?.[1] ?? '';
      submittedPrices.push(price);

      // モック応答。範囲外はクライアントの送信前ゲートで届かないため、
      // もし届いた場合にだけ失敗を返す（ゲート破りの検出用）。
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
    const failed = page.getByTestId('upload-failed');
    const clientPriceError = 'Price must be between 0 and 1000000 JPY.';

    // -1: クライアントの送信前ゲートで拒否され、失敗用 role="alert" に固定文言のみ表示。route は0回。
    await priceInput.fill('-1');
    await submitButton.click();
    await expect(failed).toContainText(clientPriceError, { timeout: 15000 });
    await expect(failed).toHaveAttribute('role', 'alert');
    // 価格入力を aria-invalid とし、エラー要素を aria-describedby で参照する。
    await expect(priceInput).toHaveAttribute('aria-invalid', 'true');
    await expect(priceInput).toHaveAttribute('aria-describedby', 'upload-failed');
    expect(uploadRouteCalls).toBe(0);

    // 1000001: 上限超過も同じゲートで拒否され、引き続き route は0回。
    await priceInput.fill('1000001');
    await submitButton.click();
    await expect(failed).toContainText(clientPriceError, { timeout: 15000 });
    expect(uploadRouteCalls).toBe(0);
    await expect(failed).not.toContainText('Failed to upload content');

    // 0: 購入不可を表すが、サーバー契約では下限として受理される。
    await priceInput.fill('0');
    await submitButton.click();
    // 受理されたら aria-invalid / aria-describedby を解除する。
    await expect(priceInput).toHaveAttribute('aria-invalid', 'false');
    await expect(priceInput).not.toHaveAttribute('aria-describedby');
    await expect(status).toContainText('SUCCESS: Content registered', { timeout: 15000 });

    // 1000000: 上限境界は受理。
    await priceInput.fill('1000000');
    await submitButton.click();
    await expect(status).toContainText('SUCCESS: Content registered', { timeout: 15000 });

    // 範囲外は通信に到達せず、0 / 1000000 だけが丸めずに送られたことを確認する。
    await expect.poll(() => uploadRouteCalls).toBe(2);
    expect(submittedPrices).toEqual(['0', '1000000']);
  });
});
