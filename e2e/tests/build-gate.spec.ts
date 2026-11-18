import { test, expect } from '@playwright/test';
import * as fs from 'node:fs';
import * as path from 'node:path';

// ci-e2e.sh の trunk checksum ゲートに関する静的回帰テスト。
// キャッシュ済みアーカイブが改ざんされた場合に fail-open しないことを担保する。
// 実バイナリを改ざんして exit 1 を観測するテストは準備コストが高いため、
// 「固定 checksum を sha256sum -c に必ず通す構造」をソースから確認する。
test.describe('Build gate (BG-01)', () => {
  const scriptPath = path.resolve(__dirname, '..', 'ci-e2e.sh');

  test('trunk archive is verified with a fixed sha256 on every run', () => {
    const script = fs.readFileSync(scriptPath, 'utf8');
    const lines = script.split(/\r?\n/);

    // 固定 checksum（64 桁の hex）がリテラルとして定義されていること。
    expect(script).toMatch(/trunk_sha256="[0-9a-f]{64}"/);

    // その固定値を echo して sha256sum -c に渡していること。
    const verifyIndex = lines.findIndex((line) => line.includes('sha256sum -c'));
    expect(verifyIndex).toBeGreaterThanOrEqual(0);
    const verifyLine = lines[verifyIndex];
    expect(verifyLine).toContain('${trunk_sha256}');
    expect(verifyLine).toContain('${trunk_archive}');

    // キャッシュの有無にかかわらず検証されるよう、キャッシュ分岐の fi より後、
    // アーカイブ展開より前に検証が置かれていること。
    const cacheIfIndex = lines.findIndex((line) =>
      line.includes('if [[ -f "$trunk_archive" ]]'),
    );
    expect(cacheIfIndex).toBeGreaterThanOrEqual(0);
    const cacheFiIndex = lines.findIndex(
      (line, index) => index > cacheIfIndex && line.trim() === 'fi',
    );
    expect(cacheFiIndex).toBeGreaterThan(cacheIfIndex);
    expect(verifyIndex).toBeGreaterThan(cacheFiIndex);

    const extractIndex = lines.findIndex((line) => line.includes('tar -xzf'));
    expect(extractIndex).toBeGreaterThan(verifyIndex);

    // 検証失敗時に exit 1 で終了すること。
    const failureBlock = lines.slice(verifyIndex, verifyIndex + 6).join('\n');
    expect(failureBlock).toContain('exit 1');
  });
});
