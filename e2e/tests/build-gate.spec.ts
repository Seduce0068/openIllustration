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

// ci-e2e.sh の major バージョン検査の実挙動テスト（VG-01）。
// E2E_LIB_ONLY=1 で ci-e2e.sh を source すると、ダウンロード/ビルド等の
// 副作用を一切起こさずに assert_major_numeric の定義だけが読み込める。
// node/google-chrome のスタブは不要（検査対象の関数は外部コマンドを呼ばない）。
test.describe('Version gate (VG-01)', () => {
  const e2eDir = path.resolve(__dirname, '..');
  const scriptPath = path.join(e2eDir, 'ci-e2e.sh');

  type AssertResult = { status: number | null; stderr: string };

  // bash -c の位置パラメータで値を渡すため、ラベル/値の quoting に依存しない。
  // $1=scriptPath, $2=label, $3=value, $4=allow_empty(任意)。
  const runAssertMajorNumeric = (
    label: string,
    value: string,
    allowEmpty?: string,
  ): AssertResult => {
    const args = [
      '-c',
      'source "$1"; shift; assert_major_numeric "$@"',
      '_',
      scriptPath,
      label,
      value,
    ];
    if (allowEmpty !== undefined) args.push(allowEmpty);
    const result = spawnSync('bash', args, {
      encoding: 'utf8',
      env: { ...process.env, E2E_LIB_ONLY: '1' },
    });
    if (result.error) throw result.error;
    return { status: result.status, stderr: result.stderr };
  };

  test('non-numeric extracted major exits 1', () => {
    // (a) node --version 等から抽出された値が非数値なら即座に失敗する。
    const result = runAssertMajorNumeric('Node major version', 'abc');
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("Node major version is not numeric: 'abc'.");
  });

  test('non-numeric threshold exits 1 even when empty is allowed', () => {
    // (b) MIN_NODE_MAJOR=abc 等、閾値側の非数値も allow_empty=1 でも失敗する。
    const result = runAssertMajorNumeric('MIN_NODE_MAJOR', 'abc', '1');
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("MIN_NODE_MAJOR is not numeric: 'abc'.");
  });

  test('empty MAX_* with allow_empty is skipped without warning', () => {
    // (c) MAX_* の空文字は従来どおり警告なしでスキップされる。
    const result = runAssertMajorNumeric('MAX_NODE_MAJOR', '', '1');
    expect(result.status).toBe(0);
    expect(result.stderr).toBe('');
  });

  test('empty value with allow_empty is accepted', () => {
    // (d) 空文字は allow_empty=1 のとき「未設定」として受理される。
    // (c) が MAX_* のケースを覆うのに対し、ここは script 内で allow_empty=1 が
    // 渡される MIN_* と同じラベルで分岐を直接確認する（ラベル非依存の確認）。
    const result = runAssertMajorNumeric('MIN_NODE_MAJOR', '', '1');
    expect(result.status).toBe(0);
    expect(result.stderr).toBe('');
  });

  test('empty value without allow_empty exits 1', () => {
    // (e) allow_empty 無しの空文字は必須の抽出値として扱われ即座に失敗する。
    // これで (a)〜(c) と合わせ assert_major_numeric の 5 分岐をすべて覆う。
    const result = runAssertMajorNumeric('Node major version', '');
    expect(result.status).toBe(1);
    expect(result.stderr).toContain(
      'Node major version is empty: expected a numeric major version.',
    );
  });

  test('leading-zero major exits 1', () => {
    // (f) 先頭ゼロは 8 進数解釈による fail-open を防ぐため拒否する。
    // 既存の "is not numeric" 文言（値を含む）をそのまま利用する。
    const result = runAssertMajorNumeric('Node major version', '08');
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("Node major version is not numeric: '08'.");
  });

  test('2^64+1 major exits 1 without arithmetic wrap', () => {
    // (g) 18446744073709551617 (2^64+1) は算術すると 1 にラップし得るため、
    // 桁数＋辞書順の上限チェックで拒否される必要がある。
    const result = runAssertMajorNumeric('Node major version', '18446744073709551617');
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('Node major version exceeds the supported major version range.');
    // 固定エラーであり、拒否した値がエコーされないこと。
    expect(result.stderr).not.toContain('18446744073709551617');
  });

  test('ordinary major is accepted', () => {
    // (h) 通常の 22 は従来どおり受理される（Node 下限 22 と整合）。
    const result = runAssertMajorNumeric('Node major version', '22');
    expect(result.status).toBe(0);
    expect(result.stderr).toBe('');
  });

  test('E2E_LIB_ONLY=1 direct execution of ci-e2e.sh is refused', () => {
    // E2E_LIB_ONLY=1 は source 専用。直接実行された場合はダウンロード/ビルド等
    // の副作用へ進まず、ゲート回避を防ぐため exit 1 で明示拒否される。
    const result = spawnSync('bash', [scriptPath], {
      encoding: 'utf8',
      env: { ...process.env, E2E_LIB_ONLY: '1' },
    });
    if (result.error) throw result.error;
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('direct execution is refused');
    // 拒否より前にアーカイブ導入・ビルド等の副作用出力が出ていないこと。
    expect(result.stdout).toBe('');
  });
});

// ci-e2e.sh のプロジェクト選択ロジックの単体テスト（EPS-01）。
// E2E_LIB_ONLY=1 で source するとダウンロード/ビルド等の副作用なしで
// select_e2e_projects が呼べる。明示指定の厳守・両利用可で3プロジェクト・
// 片方不可で chromium＋警告 の3ケースを確認する。
test.describe('E2E project selection (EPS-01)', () => {
  const e2eDir = path.resolve(__dirname, '..');
  const scriptPath = path.join(e2eDir, 'ci-e2e.sh');

  type SelectionResult = { status: number | null; stdout: string; stderr: string };

  // bash -c の位置パラメータで純粋関数へ入力を渡す。
  // $1=scriptPath, $2=E2E_PROJECTS 明示値, $3=firefox 可否, $4=webkit 可否。
  const runSelection = (
    explicit: string,
    firefoxOk: string,
    webkitOk: string,
  ): SelectionResult => {
    const inner = [
      'source "$1"; shift',
      'declare -a projects=()',
      'declare warn=0',
      'select_e2e_projects "$@" projects warn || exit $?',
      'printf "PROJECTS=%s\\n" "${projects[*]}"',
      'printf "WARN=%s\\n" "$warn"',
    ].join('\n');
    const result = spawnSync(
      'bash',
      ['-c', inner, '_', scriptPath, explicit, firefoxOk, webkitOk],
      {
        encoding: 'utf8',
        env: { ...process.env, E2E_LIB_ONLY: '1' },
      },
    );
    if (result.error) throw result.error;
    return { status: result.status, stdout: result.stdout, stderr: result.stderr };
  };

  test('explicit E2E_PROJECTS is honored even when browsers are unavailable', () => {
    // (a) 明示指定は実行可能性に関わらず厳守され、警告も出ない。
    const result = runSelection('firefox', '0', '0');
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('PROJECTS=firefox');
    expect(result.stdout).toContain('WARN=0');

    // 既存の未知プロジェクト拒否も維持される。
    const unknown = runSelection('bogus', '1', '1');
    expect(unknown.status).toBe(1);
    expect(unknown.stderr).toContain('unknown E2E project: bogus');
  });

  test('both browsers executable selects all three projects without warning', () => {
    // (b) 未指定かつ firefox/webkit が両方実行可能なら3プロジェクト。
    const result = runSelection('', '1', '1');
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('PROJECTS=chromium firefox webkit');
    expect(result.stdout).toContain('WARN=0');
  });

  test('one browser unavailable falls back to chromium with a warning', () => {
    // (c) 未指定で片方でも実行不可なら chromium のみ＋警告フラグ。
    const noWebkit = runSelection('', '1', '0');
    expect(noWebkit.status).toBe(0);
    expect(noWebkit.stdout).toContain('PROJECTS=chromium');
    expect(noWebkit.stdout).not.toContain('PROJECTS=chromium firefox');
    expect(noWebkit.stdout).toContain('WARN=1');

    const noFirefox = runSelection('', '0', '1');
    expect(noFirefox.status).toBe(0);
    expect(noFirefox.stdout).toContain('PROJECTS=chromium');
    expect(noFirefox.stdout).toContain('WARN=1');
  });
});
