import { defineConfig } from '@playwright/test';

// ポートは server/src/lib.rs（3000）と client/Trunk.toml（8080）に合わせる。
const serverUrl = 'http://127.0.0.1:3000';
const clientUrl = 'http://127.0.0.1:8080';

// E2E 専用の開発用固定値。モック認証と IPv4 ループバック例外を明示的に
// 有効化する。`E2E_JWT_SECRET` で上書きでき、CI では e2e/ci-e2e.sh が
// 実行ごとにランダム生成した値を渡す。本番の JWT_SECRET へ流用しない。
const e2eJwtSecret =
  process.env.E2E_JWT_SECRET ?? 'test_secret_for_e2e_0123456789abcdef';

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  // CI では失敗内容をジョブログへ出す（HTML レポートは自動で開かない）。
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'html',
  use: {
    baseURL: clientUrl,
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
  // 開発サーバーを自動起動する。`cwd` は設定ファイルからの相対パスで解決され、
  // `env` は process.env へマージされる。trunk は PATH 上のものを使用する。
  webServer: [
    {
      command: 'cargo run --locked',
      cwd: '../server',
      env: {
        JWT_SECRET: e2eJwtSecret,
        ALLOW_MOCK_AUTH: '1',
        ALLOW_IPV4_LOCALHOST: '1',
      },
      url: `${serverUrl}/health`,
      timeout: 120 * 1000,
      reuseExistingServer: !process.env.CI,
    },
    {
      command: 'trunk serve --address 127.0.0.1 --port 8080',
      cwd: '../client',
      url: clientUrl,
      timeout: 120 * 1000,
      reuseExistingServer: !process.env.CI,
    },
  ],
});
