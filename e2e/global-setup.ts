import { chromium, firefox, webkit, type BrowserType, type FullConfig } from '@playwright/test';

/**
 * E2E グローバルセットアップ（R2-P4: console.error / pageerror 監視）。
 *
 * ブラウザを一度だけ起動し、主要画面を訪れて console.error と pageerror を収集する。
 * 許可リスト外のメッセージがあればテスト開始前に失敗させる（新規 test() は追加しない）。
 * メッセージ本文はブラウザ由来の信頼できないデータとして扱い、固定文言の判定にのみ使う。
 */

// 既存の履歴警告（economy.spec.ts の fail-closed オラクル）は許可する。
// favicon の 404 等、ブラウザ既定のリソースエラーもアプリ由来ではないため除外する。
const ALLOWED_MESSAGES: RegExp[] = [
  /履歴の更新に失敗しました。/,
  /history update failed; view unchanged/,
  /favicon\.ico/,
];

// 主要画面に絞る（未認証でもレンダリングされるルートのみ、待機は短く）。
const SMOKE_HASHES = ['', '#dashboard', '#ledger'];

const isAllowed = (text: string): boolean =>
  ALLOWED_MESSAGES.some(pattern => pattern.test(text));

async function waitForServer(url: string): Promise<void> {
  const deadline = Date.now() + 30_000;
  for (;;) {
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(2_000) });
      if (response.ok) return;
    } catch {
      // webServer の起動待ち。取得失敗は次回試行へ。
    }
    if (Date.now() >= deadline) {
      throw new Error(`[E2E global setup] server did not become ready: ${url}`);
    }
    await new Promise(resolve => setTimeout(resolve, 250));
  }
}

export default async function globalSetup(config: FullConfig): Promise<void> {
  const project = config.projects.find(candidate => candidate.use?.baseURL);
  const baseURL = project?.use?.baseURL as string | undefined;
  if (!baseURL) {
    throw new Error('[E2E global setup] baseURL is not configured');
  }

  await waitForServer(baseURL);

  // 選択された全プロジェクトのエンジンを監視する（chrome 固定だと firefox/webkit の
  // エンジン固有例外が静かに通る）。channel は chromium の chrome のみに適用する。
  const names = new Set<string>();
  for (const project of config.projects) {
    const name = (project.use?.browserName ?? 'chromium') as string;
    names.add(name);
  }
  if (names.size === 0) names.add('chromium');
  const types: Record<string, BrowserType> = { chromium, firefox, webkit };
  const offenders: string[] = [];

  for (const name of names) {
    const type = types[name];
    if (!type) {
      throw new Error(`[E2E global setup] unsupported browserName: ${name}`);
    }
    const browser = await type.launch(
      name === 'chromium' ? { channel: 'chrome', headless: true } : { headless: true },
    );
    try {
      const context = await browser.newContext({ viewport: { width: 1280, height: 720 } });
      const page = await context.newPage();
      page.on('console', message => {
        if (message.type() !== 'error') return;
        const text = message.text();
        if (!isAllowed(text)) offenders.push(`${name} console.error: ${text}`);
      });
      page.on('pageerror', error => {
        const text = error.message;
        if (!isAllowed(text)) offenders.push(`${name} pageerror: ${text}`);
      });
      for (const hash of SMOKE_HASHES) {
        await page.goto(`${baseURL}/${hash}`, {
          waitUntil: 'domcontentloaded',
          timeout: 15_000,
        });
        await page.waitForTimeout(300);
      }
      await context.close();
    } finally {
      await browser.close();
    }
  }

  if (offenders.length > 0) {
    throw new Error(
      `[E2E global setup] unexpected browser errors (allowlist: history warning only):\n${offenders.join('\n')}`,
    );
  }
}
