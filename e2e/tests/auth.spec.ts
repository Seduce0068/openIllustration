import { test, expect } from '@playwright/test';

test.describe('Authentication Flow (SEC-01, SEC-03)', () => {
  test('should login successfully with mock JPKI token', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('h1')).toContainText('OPENILLUSTRATION');

    const loginButton = page.getByRole('button', { name: 'CONNECT WITH JPKI' });
    await expect(loginButton).toBeVisible();
    await loginButton.click();

    await expect(page.getByRole('button', { name: 'UPLOAD' })).toBeVisible({ timeout: 10000 });
    
    const userHash = page.locator('span.text-green-500');
    await expect(userHash).toBeVisible();
    const hashText = await userHash.innerText();
    expect(hashText.length).toBeGreaterThan(0);
  });

  test('SEC-03: should reject underage user', async ({ page }) => {
    await page.route('**/auth/verify', async route => {
      await route.fulfill({
        status: 403,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'Age verification failed for JP: age 9, required 18' })
      });
    });

    await page.goto('/');
    const loginButton = page.getByRole('button', { name: 'CONNECT WITH JPKI' });

    // 固定スリープではなく、インターセプトした 403 レスポンスの完了を待つ。
    const rejected = page.waitForResponse(
      response => response.url().includes('/auth/verify') && response.status() === 403,
      { timeout: 10000 }
    );
    await loginButton.click();
    await rejected;

    await expect(page.getByRole('button', { name: 'UPLOAD' })).not.toBeVisible();
  });
});
