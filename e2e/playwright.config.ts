import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: 'html',
  use: {
    baseURL: 'http://127.0.0.1:8080',
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { 
        channel: 'chrome',
        viewport: { width: 1280, height: 720 },
      },
    },
  ],
  // 開発サーバーを自動起動する設定
  webServer: [
    {
      command: 'export JWT_SECRET=test_secret_for_e2e && export ALLOW_MOCK_AUTH=1 && export ALLOW_IPV4_LOCALHOST=1 && cd ../server && cargo run',
      url: 'http://127.0.0.1:3000/health',
      timeout: 120 * 1000,
      reuseExistingServer: !process.env.CI,
    },
    {
      command: 'cd ../client && /home/koppe/.cargo/bin/trunk serve --address 127.0.0.1 --port 8080',
      url: 'http://127.0.0.1:8080',
      timeout: 120 * 1000,
      reuseExistingServer: !process.env.CI,
    }
  ],
});
