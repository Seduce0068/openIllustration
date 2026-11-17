import { defineConfig } from '@playwright/test';
import * as os from 'node:os';
import * as path from 'node:path';

// ポートは server/src/lib.rs（3000）と client/Trunk.toml（8080）に合わせる。
// 8080 が他プロセス（例: code-server）に使われている場合は、ローカル実行に
// 限り E2E_CLIENT_PORT でクライアントの待受ポートを変更できる。
const serverUrl = 'http://127.0.0.1:3000';
const clientPort = process.env.E2E_CLIENT_PORT ?? '8080';
const clientUrl = `http://127.0.0.1:${clientPort}`;

// E2E 専用の開発用固定値。モック認証と IPv4 ループバック例外を明示的に
// 有効化する。`E2E_JWT_SECRET` で上書きでき、CI では e2e/ci-e2e.sh が
// 実行ごとにランダム生成した値を渡す。本番の JWT_SECRET へ流用しない。
const e2eJwtSecret =
  process.env.E2E_JWT_SECRET ?? 'test_secret_for_e2e_0123456789abcdef';

// ローカルは既存プロセスと `server/local.db` を再利用する。CI は常に
// 起動し直し、`E2E_REUSE=0` でローカルでも再利用を無効化できる。
const reuseExistingServer = process.env.E2E_REUSE !== '0' && !process.env.CI;

// サーバーを新規起動する場合、未指定なら一時DBを使用してローカル状態の
// 持ち越しを避ける（CIは e2e/ci-e2e.sh が DATABASE_URL を設定）。
const serverDatabaseUrl = reuseExistingServer
  ? undefined
  : (process.env.DATABASE_URL ??
    `file:${path.join(os.tmpdir(), `openillustration-e2e-${Date.now()}-${process.pid}.db`)}`);

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
        ...(serverDatabaseUrl ? { DATABASE_URL: serverDatabaseUrl } : {}),
        JWT_SECRET: e2eJwtSecret,
        ALLOW_MOCK_AUTH: '1',
        ALLOW_IPV4_LOCALHOST: '1',
        // クライアントのポートを変えた場合も CORS で許可する。
        // 既定値（server/src/lib.rs の DEV_ALLOWED_ORIGINS）と同じ3種。
        ALLOWED_ORIGINS: [
          `http://127.0.0.1:${clientPort}`,
          `http://localhost:${clientPort}`,
          `http://[::1]:${clientPort}`,
        ].join(','),
      },
      url: `${serverUrl}/health`,
      timeout: 120 * 1000,
      reuseExistingServer,
    },
    {
      command: `trunk serve --address 127.0.0.1 --port ${clientPort}`,
      cwd: '../client',
      url: clientUrl,
      timeout: 120 * 1000,
      reuseExistingServer,
    },
  ],
});
