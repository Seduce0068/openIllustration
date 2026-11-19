import { test, expect } from '@playwright/test';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { gzipSync } from 'node:zlib';

// ci-e2e.sh の trunk checksum ゲートの回帰テスト。
// 静的な構造確認に加え、一時ディレクトリで実バイナリ（tar.gz）を改ざんし、
// 共通ゲートを実際に実行して fail-open しないことを観測する。
test.describe('Build gate (BG-01)', () => {
  const e2eDir = path.resolve(__dirname, '..');
  const scriptPath = path.join(e2eDir, 'ci-e2e.sh');
  const gatePath = path.join(e2eDir, 'verify_trunk_archive.sh');
  const CANARY = 'BUILD-GATE-CANARY-GOOD';
  const TAMPERED_CANARY = 'BUILD-GATE-CANARY-BAD!'; // 同一長の内容差し替え

  const tempDirs: string[] = [];
  test.afterEach(() => {
    for (const dir of tempDirs.splice(0)) {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  type Fixtures = {
    tempDir: string;
    goodArchive: string;
    tamperedArchive: string;
    expectedSha256: string;
  };

  // 正しいアーカイブと、その内容バイトだけを差し替えた改ざんアーカイブを作る。
  // いずれも有効な tar.gz なので、tar -xzf が走れば trunk が展開される。
  const createFixtures = (): Fixtures => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'build-gate-'));
    tempDirs.push(tempDir);

    expect(TAMPERED_CANARY).toHaveLength(CANARY.length);
    const payload = path.join(tempDir, 'trunk');
    fs.writeFileSync(payload, CANARY);

    const rawTar = path.join(tempDir, 'trunk-good.tar');
    execFileSync('tar', ['-cf', rawTar, '-C', tempDir, 'trunk']);

    const goodTar = fs.readFileSync(rawTar);
    const offset = goodTar.indexOf(Buffer.from(CANARY, 'utf8'));
    expect(offset).toBeGreaterThan(0);
    const tamperedTar = Buffer.from(goodTar);
    tamperedTar.write(TAMPERED_CANARY, offset, 'utf8');
    expect(tamperedTar.equals(goodTar)).toBe(false);

    const goodGzip = gzipSync(goodTar);
    const tamperedGzip = gzipSync(tamperedTar);
    const goodArchive = path.join(tempDir, 'trunk-good.tar.gz');
    const tamperedArchive = path.join(tempDir, 'trunk-tampered.tar.gz');
    fs.writeFileSync(goodArchive, goodGzip);
    fs.writeFileSync(tamperedArchive, tamperedGzip);

    const expectedSha256 = createHash('sha256').update(goodGzip).digest('hex');
    expect(expectedSha256).toMatch(/^[0-9a-f]{64}$/);
    expect(createHash('sha256').update(tamperedGzip).digest('hex')).not.toBe(expectedSha256);

    // 改ざんアーカイブも展開可能であることを先に確認しておく。これにより
    // 後続の「展開されない」観測が、単なる不正 gzip による失敗ではないと分かる。
    const probeDir = path.join(tempDir, 'probe');
    fs.mkdirSync(probeDir);
    execFileSync('tar', ['-xzf', tamperedArchive, '-C', probeDir]);
    expect(fs.readFileSync(path.join(probeDir, 'trunk'), 'utf8')).toBe(TAMPERED_CANARY);

    return { tempDir, goodArchive, tamperedArchive, expectedSha256 };
  };

  type GateResult = { status: number | null; stdout: string; stderr: string };

  const runGate = (
    archive: string,
    expectedSha256: string,
    extractDir: string,
  ): GateResult => {
    const result = spawnSync('bash', [gatePath], {
      encoding: 'utf8',
      env: {
        ...process.env,
        TRUNK_ARCHIVE: archive,
        TRUNK_SHA256: expectedSha256,
        TRUNK_VERSION: 'v0-test',
        TRUNK_EXTRACT_DIR: extractDir,
      },
    });
    if (result.error) throw result.error;
    return { status: result.status, stdout: result.stdout, stderr: result.stderr };
  };

  test('checksum gate is wired before extraction in ci-e2e.sh', () => {
    const script = fs.readFileSync(scriptPath, 'utf8');
    const gate = fs.readFileSync(gatePath, 'utf8');
    const lines = script.split(/\r?\n/);
    const gateLines = gate.split(/\r?\n/);

    // 固定 checksum リテラルは変更せず、環境変数として共通ゲートへ渡す。
    expect(script).toMatch(/trunk_sha256="[0-9a-f]{64}"/);
    expect(script).toContain('TRUNK_ARCHIVE="$trunk_archive"');
    expect(script).toContain('TRUNK_SHA256="$trunk_sha256"');

    // コメント行ではなく実際の呼び出し行を特定する（呼び出し自体の存在を検証）。
    const gateCall = 'bash "${repo_root}/e2e/verify_trunk_archive.sh"';
    expect(script).toContain(gateCall);
    const gateIndex = lines.findIndex((line) => line.includes(gateCall));
    expect(gateIndex).toBeGreaterThanOrEqual(0);

    const cacheIfIndex = lines.findIndex((line) => line.includes('if [[ -f "$trunk_archive" ]]'));
    expect(cacheIfIndex).toBeGreaterThanOrEqual(0);
    const cacheFiIndex = lines.findIndex(
      (line, index) => index > cacheIfIndex && line.trim() === 'fi',
    );
    expect(cacheFiIndex).toBeGreaterThan(cacheIfIndex);
    expect(gateIndex).toBeGreaterThan(cacheFiIndex); // キャッシュの有無にかかわらず検証

    const chmodIndex = lines.findIndex((line) => line.includes('chmod +x "${workdir}/bin/trunk"'));
    expect(chmodIndex).toBeGreaterThan(gateIndex); // 展開前に必ずゲートを通す

    // 共通ゲート側: checksum 検証 → 失敗時 exit 1 → 成功時にのみ tar -xzf。
    expect(gate).toContain('sha256sum -c');
    expect(gate).toContain('exit 1');
    const verifyIndex = gateLines.findIndex((line) => line.includes('sha256sum -c'));
    const extractIndex = gateLines.findIndex((line) => line.includes('tar -xzf'));
    expect(verifyIndex).toBeGreaterThanOrEqual(0);
    expect(extractIndex).toBeGreaterThan(verifyIndex);
  });

  test('genuine archive passes the gate and is extracted', () => {
    const { tempDir, goodArchive, expectedSha256 } = createFixtures();
    const extractDir = path.join(tempDir, 'extract-good');

    const result = runGate(goodArchive, expectedSha256, extractDir);

    expect(result.status).toBe(0);
    expect(fs.readFileSync(path.join(extractDir, 'trunk'), 'utf8')).toBe(CANARY);
  });

  test('tampered archive fails with exit 1 and is never extracted', () => {
    const { tempDir, tamperedArchive, expectedSha256 } = createFixtures();
    const extractDir = path.join(tempDir, 'extract-tampered');

    const result = runGate(tamperedArchive, expectedSha256, extractDir);

    expect(result.status).toBe(1);
    // 失敗原因が checksum 不一致であることを確認（別理由の exit 1 を排除）。
    expect(result.stderr).toContain('sha256 が一致しない');
    // 展開可能だった改ざんアーカイブでも、ゲート失敗後は tar -xzf が走らない。
    expect(fs.existsSync(path.join(extractDir, 'trunk'))).toBe(false);
  });
});
