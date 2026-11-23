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

  // 入力検証（TRUNK_* 未設定 / 不正値 / アーカイブ欠落 / 展開先未指定）を
  // 環境変数の差し替えだけで実行する（既存 runGate と同じ spawn 契約）。
  const runGateWithEnv = (
    overrides: Record<string, string | undefined>,
  ): GateResult => {
    const env: NodeJS.ProcessEnv = { ...process.env };
    for (const [key, value] of Object.entries(overrides)) {
      if (value === undefined) delete env[key];
      else env[key] = value;
    }
    const result = spawnSync('bash', [gatePath], { encoding: 'utf8', env });
    if (result.error) throw result.error;
    return { status: result.status, stdout: result.stdout, stderr: result.stderr };
  };

  test('checksum gate is wired before extraction in ci-e2e.sh', () => {
    // PWA アセット（第73ラウンド）の配信契約を静的に固定する（新規 test() なし）。
    const clientRoot = path.resolve(__dirname, '../../client');
    const manifestPath = path.join(clientRoot, 'manifest.webmanifest');
    const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
    expect(manifest.name).toBeTruthy();
    expect(manifest.short_name).toBeTruthy();
    expect(manifest.start_url).toBe('/');
    expect(manifest.display).toBe('standalone');
    expect(Array.isArray(manifest.icons) && manifest.icons.length > 0).toBe(true);
    expect(fs.existsSync(path.join(clientRoot, 'favicon.svg'))).toBe(true);
    const html = fs.readFileSync(path.join(clientRoot, 'index.html'), 'utf8');
    expect(html).toContain('rel="manifest"');
    expect(html).toContain('rel="icon"');
    expect(html).toContain('name="theme-color"');

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

    // R2-P4: E2E グローバル setup で console.error / pageerror を収集し、
    // 許可リスト外で失敗する構成になっていることの静的オラクル（新規 test() なし）。
    const configPath = path.join(e2eDir, 'playwright.config.ts');
    const setupPath = path.join(e2eDir, 'global-setup.ts');
    const configText = fs.readFileSync(configPath, 'utf8');
    const setupText = fs.readFileSync(setupPath, 'utf8');
    expect(configText).toContain("globalSetup: './global-setup.ts'");
    expect(setupText).toContain("page.on('console'");
    expect(setupText).toContain("page.on('pageerror'");
    expect(setupText).toContain('message.type() !== \'error\'');
    // 既存の履歴警告は許可リストへ（fail-closed オラクル economy.spec.ts と同じ文言）。
    expect(setupText).toContain('履歴の更新に失敗しました。');
    expect(setupText).toContain('offenders.length > 0');
  });

  test('genuine archive passes the gate and is extracted', () => {
    const { tempDir, goodArchive, expectedSha256 } = createFixtures();
    const extractDir = path.join(tempDir, 'extract-good');

    const result = runGate(goodArchive, expectedSha256, extractDir);

    expect(result.status).toBe(0);
    expect(fs.readFileSync(path.join(extractDir, 'trunk'), 'utf8')).toBe(CANARY);
  });

  test('tampered archive fails with exit 1 and is never extracted', () => {
    const { tempDir, goodArchive, tamperedArchive, expectedSha256 } = createFixtures();
    const extractDir = path.join(tempDir, 'extract-tampered');

    const result = runGate(tamperedArchive, expectedSha256, extractDir);

    expect(result.status).toBe(1);
    // 失敗原因が checksum 不一致であることを確認（別理由の exit 1 を排除）。
    expect(result.stderr).toContain('sha256 が一致しない');
    // 展開可能だった改ざんアーカイブでも、ゲート失敗後は tar -xzf が走らない。
    expect(fs.existsSync(path.join(extractDir, 'trunk'))).toBe(false);

    // --- 入力検証（ fail-closed ）---------------------------------------
    // 同じフィクスチャの正規アーカイブと checksum を使い、入力の違いだけを検証する。

    // TRUNK_ARCHIVE 未設定。
    const unsetArchive = runGateWithEnv({
      TRUNK_ARCHIVE: undefined,
      TRUNK_SHA256: expectedSha256,
      TRUNK_EXTRACT_DIR: path.join(tempDir, 'extract-unset-archive'),
    });
    expect(unsetArchive.status).toBe(1);
    expect(unsetArchive.stderr).toContain('TRUNK_ARCHIVE is not set.');

    // TRUNK_SHA256 が 64 桁に満たない。
    const shortHash = runGateWithEnv({
      TRUNK_ARCHIVE: goodArchive,
      TRUNK_SHA256: 'abc123',
      TRUNK_EXTRACT_DIR: path.join(tempDir, 'extract-short-hash'),
    });
    expect(shortHash.status).toBe(1);
    expect(shortHash.stderr).toContain('TRUNK_SHA256 must be a 64-digit lowercase hex value.');

    // TRUNK_SHA256 が大文字（小文字 hex のみ受理）。
    const upperHash = runGateWithEnv({
      TRUNK_ARCHIVE: goodArchive,
      TRUNK_SHA256: expectedSha256.toUpperCase(),
      TRUNK_EXTRACT_DIR: path.join(tempDir, 'extract-upper-hash'),
    });
    expect(upperHash.status).toBe(1);
    expect(upperHash.stderr).toContain('TRUNK_SHA256 must be a 64-digit lowercase hex value.');

    // アーカイブ欠落（checksum 検証より前に fail-fast）。
    const missingArchive = runGateWithEnv({
      TRUNK_ARCHIVE: path.join(tempDir, 'missing.tar.gz'),
      TRUNK_SHA256: expectedSha256,
      TRUNK_EXTRACT_DIR: path.join(tempDir, 'extract-missing'),
    });
    expect(missingArchive.status).toBe(1);
    expect(missingArchive.stderr).toContain('trunk archive is missing:');

    // TRUNK_EXTRACT_DIR 未指定は検証のみを行い、展開せず成功する。
    const noExtractDir = runGateWithEnv({
      TRUNK_ARCHIVE: goodArchive,
      TRUNK_SHA256: expectedSha256,
      TRUNK_EXTRACT_DIR: undefined,
    });
    expect(noExtractDir.status).toBe(0);
    expect(noExtractDir.stdout).toContain(': OK');
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

    // 境界値: 9 桁の上限 999999999 は受理、+1 の 1000000000 は拒否。
    const atLimit = runAssertMajorNumeric('Node major version', '999999999');
    expect(atLimit.status).toBe(0);
    expect(atLimit.stderr).toBe('');

    const overLimit = runAssertMajorNumeric('Node major version', '1000000000');
    expect(overLimit.status).toBe(1);
    expect(overLimit.stderr).toContain('Node major version exceeds the supported major version range.');
    expect(overLimit.stderr).not.toContain('1000000000');
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
// select_e2e_projects と、その選択を可視化する formatter / writer が呼べる。
// 明示指定の厳守・両利用可で3プロジェクト・片方不可で chromium＋警告 の3ケースを確認する。
test.describe('E2E project selection (EPS-01)', () => {
  const e2eDir = path.resolve(__dirname, '..');
  const scriptPath = path.join(e2eDir, 'ci-e2e.sh');

  const tempDirs: string[] = [];
  test.afterEach(() => {
    for (const dir of tempDirs.splice(0)) {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  type SelectionResult = { status: number | null; stdout: string; stderr: string };

  // bash -c の位置パラメータで純粋関数へ入力を渡す。
  // $1=scriptPath, $2=E2E_PROJECTS 明示値, $3=firefox 可否, $4=webkit 可否。
  // source した formatter にも同じ入力を渡し、annotation / summary 文字列を検証する。
  const runSelection = (
    explicit: string,
    firefoxOk: string,
    webkitOk: string,
  ): SelectionResult => {
    const inner = [
      'source "$1"; shift',
      'declare firefox_ok="$2" webkit_ok="$3"',
      'declare -a projects=()',
      'declare warn=0',
      'select_e2e_projects "$@" projects warn || exit $?',
      'declare annotation="" summary=""',
      'format_e2e_browser_selection projects "$firefox_ok" "$webkit_ok" "$warn" annotation summary',
      'printf "PROJECTS=%s\\n" "${projects[*]}"',
      'printf "WARN=%s\\n" "$warn"',
      'printf "ANNOTATION=%s\\n" "$annotation"',
      'printf "SUMMARY>>%s<<\\n" "$summary"',
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

  type WriterResult = {
    status: number | null;
    stdout: string;
    summaryFile: string;
  };

  // writer を実際に呼び、annotation が stdout へ、summary が要約ファイルへ出ることを確認する。
  // githubActions / writeSummary を渡さない場合は GitHub Actions 外の no-op となる。
  const runWriter = (
    annotation: string,
    summary: string,
    options: {
      githubActions?: string;
      writeSummary?: boolean;
      summaryMode?: 'normal' | 'missing' | 'unwritable';
    } = {},
  ): WriterResult => {
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'e2e-writer-'));
    tempDirs.push(tempDir);
    const summaryPath = path.join(tempDir, 'summary.md');

    const env: NodeJS.ProcessEnv = { ...process.env, E2E_LIB_ONLY: '1' };
    delete env.GITHUB_ACTIONS;
    delete env.GITHUB_STEP_SUMMARY;
    if (options.githubActions !== undefined) env.GITHUB_ACTIONS = options.githubActions;

    // 異常系: 要約ファイル欠落（存在しないパス）/ 書込不可（chmod 444）。
    // githubActions のみ（writeSummary なし）は GITHUB_STEP_SUMMARY 未設定のまま。
    const mode = options.summaryMode ?? 'normal';
    if (mode === 'normal') {
      fs.writeFileSync(summaryPath, '');
      if (options.writeSummary) env.GITHUB_STEP_SUMMARY = summaryPath;
    } else if (mode === 'missing') {
      env.GITHUB_STEP_SUMMARY = path.join(tempDir, 'missing-summary.md');
    } else {
      fs.writeFileSync(summaryPath, '');
      fs.chmodSync(summaryPath, 0o444);
      env.GITHUB_STEP_SUMMARY = summaryPath;
    }

    const inner = 'source "$1"; shift; write_e2e_browser_selection "$@"';
    const result = spawnSync('bash', ['-c', inner, '_', scriptPath, annotation, summary], {
      encoding: 'utf8',
      env,
    });
    if (result.error) throw result.error;
    return {
      status: result.status,
      stdout: result.stdout,
      summaryFile: fs.existsSync(summaryPath) ? fs.readFileSync(summaryPath, 'utf8') : '',
    };
  };

  // formatter の出力を runSelection の stdout から取り出す（SUMMARY は複数行）。
  const extract = (stdout: string) => ({
    annotation: /ANNOTATION=(.*)\n/.exec(stdout)?.[1] ?? '',
    summary: /SUMMARY>>([\s\S]*?)<<\n/.exec(stdout)?.[1] ?? '',
  });

  // COMPAT-01 のエビデンス用に、選択プロジェクトを stdout から取り出す。
  const selectedProjectsOf = (result: SelectionResult): string[] =>
    /PROJECTS=([^\n]*)/.exec(result.stdout)?.[1]?.split(/\s+/).filter(Boolean) ?? [];

  test('explicit E2E_PROJECTS is honored even when browsers are unavailable', async ({}, testInfo) => {
    // (a) 明示指定は実行可能性に関わらず厳守され、警告も出ない。
    const firefoxOk: string = '0';
    const webkitOk: string = '0';
    const result = runSelection('firefox', firefoxOk, webkitOk);
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('PROJECTS=firefox');
    expect(result.stdout).toContain('WARN=0');

    // 可視化: 明示指定では annotation は空、サマリには選択プロジェクト名が残る。
    const { annotation, summary } = extract(result.stdout);
    expect(annotation).toBe('');
    expect(result.stdout).toContain('ANNOTATION=\n');
    expect(summary).toContain('Running projects: firefox');
    expect(result.stdout).not.toContain('::warning::');

    // writer は GitHub Actions 外では no-op（終了コード0・stdout 空・要約ファイル未変更）。
    const noop = runWriter(annotation, summary);
    expect(noop.status).toBe(0);
    expect(noop.stdout).toBe('');
    expect(noop.summaryFile).toBe('');

    // 既存の未知プロジェクト拒否も維持される。
    const unknown = runSelection('bogus', '1', '1');
    expect(unknown.status).toBe(1);
    expect(unknown.stderr).toContain('unknown E2E project: bogus');

    await testInfo.attach('compat-selection', {
      body: JSON.stringify({
        case: 'explicit',
        firefoxOk: firefoxOk === '1',
        webkitOk: webkitOk === '1',
        selectedProjects: selectedProjectsOf(result),
      }),
      contentType: 'application/json',
    });
  });

  test('both browsers executable selects all three projects without warning', async ({}, testInfo) => {
    // (b) 未指定かつ firefox/webkit が両方実行可能なら3プロジェクト。
    const firefoxOk: string = '1';
    const webkitOk: string = '1';
    const result = runSelection('', firefoxOk, webkitOk);
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('PROJECTS=chromium firefox webkit');
    expect(result.stdout).toContain('WARN=0');

    // 可視化: 警告なしで、サマリに3プロジェクトすべてが記録される。
    const { annotation, summary } = extract(result.stdout);
    expect(annotation).toBe('');
    expect(summary).toContain('Running projects: chromium firefox webkit');

    await testInfo.attach('compat-selection', {
      body: JSON.stringify({
        case: 'both-available',
        firefoxOk: firefoxOk === '1',
        webkitOk: webkitOk === '1',
        selectedProjects: selectedProjectsOf(result),
      }),
      contentType: 'application/json',
    });
  });

  test('one browser unavailable falls back to chromium with a warning', async ({}, testInfo) => {
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

    // 可視化: フォールバック時は固定形の ::warning:: と理由（firefox/webkit 可否）が出る。
    const { annotation, summary } = extract(noWebkit.stdout);
    expect(annotation).toBe(
      '::warning::E2E browser fallback: firefox=1 webkit=0; running chromium',
    );
    expect(annotation).toContain('::warning::E2E browser fallback:');
    expect(summary).toContain('Fallback to chromium: 1');
    expect(summary).toContain('Running projects: chromium');

    // writer は GitHub Actions 設定時に annotation を stdout、summary を要約ファイルへ書く。
    const written = runWriter(annotation, summary, {
      githubActions: 'true',
      writeSummary: true,
    });
    expect(written.status).toBe(0);
    expect(written.stdout).toContain(
      '::warning::E2E browser fallback: firefox=1 webkit=0; running chromium',
    );
    expect(written.summaryFile).toContain('Running projects: chromium');
    expect(written.summaryFile).toContain('Fallback to chromium: 1');

    // step summary 異常系: GITHUB_ACTIONS のみ / 要約ファイル欠落 / 書込不可は
    // いずれも no-op（終了コード 0・annotation なし・要約ファイル未変更）。
    const annotationOnly = runWriter(annotation, summary, { githubActions: 'true' });
    expect(annotationOnly.status).toBe(0);
    expect(annotationOnly.stdout).toBe('');
    expect(annotationOnly.summaryFile).toBe('');

    const missingSummary = runWriter(annotation, summary, {
      githubActions: 'true',
      summaryMode: 'missing',
    });
    expect(missingSummary.status).toBe(0);
    expect(missingSummary.stdout).toBe('');
    expect(missingSummary.summaryFile).toBe('');

    // 書込不可ガード（`-w "$summary_path"`）は常に構造的に存在する。
    // root は permission bit を無視して書けるため、挙動確認は非 root のみ行う。
    expect(fs.readFileSync(scriptPath, 'utf8')).toContain('-w "$summary_path"');
    if (typeof process.getuid === 'function' && process.getuid() !== 0) {
      const unwritableSummary = runWriter(annotation, summary, {
        githubActions: 'true',
        summaryMode: 'unwritable',
      });
      expect(unwritableSummary.status).toBe(0);
      expect(unwritableSummary.stdout).toBe('');
      expect(unwritableSummary.summaryFile).toBe('');
    }

    await testInfo.attach('compat-selection', {
      body: JSON.stringify([
        {
          case: 'webkit-unavailable',
          firefoxOk: true,
          webkitOk: false,
          selectedProjects: selectedProjectsOf(noWebkit),
        },
        {
          case: 'firefox-unavailable',
          firefoxOk: false,
          webkitOk: true,
          selectedProjects: selectedProjectsOf(noFirefox),
        },
      ]),
      contentType: 'application/json',
    });
  });
});
