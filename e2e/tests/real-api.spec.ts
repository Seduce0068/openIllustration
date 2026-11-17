import { test, expect } from '@playwright/test';

/**
 * API をモックせず実サーバーへ送信するスモークテスト。
 * R1-09: クライアントが file パートへ MIME を設定しないと実サーバーは 400 を返す。
 * C1: 主要画面の実サーバー認証も同時にカバーする。
 */
test.describe('Real server integration', () => {
  test('uploads JXL through the real API without mocks', async ({ page }) => {
    await page.goto('/');

    // 実サーバーの /auth/login + /auth/verify を通る（ルートをモックしない）。
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'UPLOAD' })).toBeVisible({ timeout: 10000 });

    await page.getByRole('button', { name: 'UPLOAD' }).click();

    // CID の一意性のため毎回異なるバイト列を使う（FF 0A = JXL raw codestream）。
    const body = Buffer.concat([
      Buffer.from([0xff, 0x0a]),
      Buffer.from(`real-api smoke ${Date.now()}`),
    ]);
    await page.locator('input[type="file"]').setInputFiles({
      name: 'smoke.jxl',
      mimeType: 'image/jxl',
      buffer: body,
    });

    await page.getByRole('button', { name: /Sign & Submit/i }).click();

    await expect(page.locator('text=SUCCESS: Content registered')).toBeVisible({ timeout: 15000 });
  });
});
