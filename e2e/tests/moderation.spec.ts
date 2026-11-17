import { test, expect } from '@playwright/test';

test.describe('Moderation & Governance (GOV-01, GOV-02)', () => {
  const mockCID = 'QmAuditTestCID';

  test('GOV-01: should report content and transition to Reported status', async ({ page }) => {
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

    await page.route('**/mod/report', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(true),
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'MODERATION' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'MODERATION' }).click();

    await page.locator('input[placeholder="TARGET CID"]').fill('QmTestReportCID');
    await page.locator('textarea[placeholder="REASON"]').fill('Illegal content detected');

    await page.getByRole('button', { name: 'SUBMIT FORENSIC REPORT' }).click();

    await expect(page.locator('text=Report accepted. Content marked Reported')).toBeVisible({ timeout: 10000 });
  });

  test('should display audit trail for content', async ({ page }) => {
    const mockAuditLogs = [
      {
        log_id: 1,
        old_status: null,
        new_status: 'Approved',
        changed_by: 'SYSTEM',
        reason: 'Initial upload',
        created_at: '2026-06-08T00:00:00Z',
        entry_hash: 'abc123',
      },
      {
        log_id: 2,
        old_status: 'Approved',
        new_status: 'Reported',
        changed_by: 'user_hash_123',
        reason: 'User report: suspicious content',
        created_at: '2026-06-08T01:00:00Z',
        entry_hash: 'def456',
      },
    ];

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

    await page.route('**/content', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([{
          cid: mockCID,
          checksum: 'mock_checksum',
          type: 'Novel',
          status: 'Reported',
          price: 100,
          title: 'Audit Fixture',
          description: 'Content with a forensic timeline',
          tags: ['audit', 'mock']
        }]),
      });
    });

    await page.route(`**/content/${mockCID}`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        body: 'Test content',
      });
    });

    await page.route(`**/content/${mockCID}/audit`, async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(mockAuditLogs),
      });
    });

    await page.goto('/');
    await page.getByRole('button', { name: 'CONNECT WITH JPKI' }).click();
    await expect(page.getByRole('button', { name: 'BROWSE' })).toBeVisible({ timeout: 10000 });
    await page.getByRole('button', { name: 'BROWSE' }).click();
    
    const auditButton = page.locator('button:has-text("Audit")').first();
    await expect(auditButton).toBeVisible({ timeout: 20000 });
    await auditButton.click();

    await expect(page.locator('text=FORENSIC_TIMELINE')).toBeVisible();
    await expect(page.locator('text=Initial upload')).toBeVisible();
    await expect(page.locator('text=User report: suspicious content')).toBeVisible();
  });
});
