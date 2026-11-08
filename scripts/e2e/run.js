// E2E · 촬영 · 번역 확인을 한 줄로 (Windows · ROCm): 앱을 띄우고, 엔진이 준비되면 스크립트를 돌리고, 앱을 내립니다.
// 예전에는 rocm-env · CARGO_TARGET_DIR · 디버그 포트 · 스캐너 폴더 · 모델 기록을 손으로 맞추고(문서의 7줄), 앱을 띄운 뒤
// 상태 줄을 보고 기다렸다가 스크립트를 돌리고, 끝나면 앱을 PID 로 찾아 내렸습니다.
//
//   node scripts/e2e/run.js                        console.js 17 흐름 (한국어 화면)
//   node scripts/e2e/run.js --lang en --only P2    console.js 인자는 그대로 넘깁니다 (--shots · --only · --lang · --audit)
//   node scripts/e2e/run.js --record [--lang en]   record.js (모델 기록 trace 를 녹화 폴더에 남김, --out 은 그대로 넘김)
//   node scripts/e2e/run.js --i18n                 한국어 --audit → build_en.py → (앱 다시 띄움) → 영어 E2E
//   node scripts/e2e/run.js --app-only             앱만 띄우고 엔진 준비까지 기다린 뒤 그대로 둡니다 (--stop 으로 내림)
//   node scripts/e2e/run.js --stop                 --app-only 로 띄운 앱을 내립니다
//   --jobs N        Rust 빌드 병렬 수 (CARGO_BUILD_JOBS). 다른 세션이 빌드 중이면 메모리가 모자라지 않게 줄입니다.
//   --no-wait-cargo 다른 cargo · rustc 가 돌고 있어도 기다리지 않습니다 (기본은 끝날 때까지 기다림, 최대 60분).
//   --keep          끝나도 앱을 내리지 않습니다.
//   --rules         모델 없이 규칙만으로 띄웁니다 (GPU 를 쓰지 않음: 다른 세션이 GPU 를 쓰는 동안 화면 · 호출 수만 볼 때).
//                   스캔 판독 · Zoro-1 판단이 없으니 E2E 전체는 통과하지 않습니다.
//   --share-gpu MB  다른 프로세스가 같은 GPU 를 쓰는 동안 이 앱이 VRAM 을 대략 MB 안에서 쓰게 합니다:
//                   가중치 상한 BLUEPRINT_VRAM_BUDGET_MB = MB − 1200, ROCm 버퍼 캐시 0, 공존 끔.
//                   상한은 가중치에만 걸립니다. 스캔 판독(SigLIP2 · gocr) 중 활성값이 1.6 GB 까지 더 붙는 것을 2026-11-09 에
//                   쟀습니다 (가중치 상한 1432 MB 에서 앱 최대 3083 MB). 그래서 1200 MB 를 활성값 · HIP 문맥 몫으로 남깁니다.
//                   Zoro-1 판단은 블록을 상한에 맞게 나눠 올려 돌고, 초안 글쓰기는 모든 블록이 함께 있어야 해서 실패하고
//                   템플릿 초안으로 대신합니다 (사건 · 결재 흐름은 같음). 판단이 느려져 물리엔진 흐름의 트립 고점 검사는 빗나갈 수 있습니다.
//                   실행 중 2초마다 이 앱 · 어댑터 전체 VRAM 을 out/e2e/vram.csv 에 남기고, 끝에 최댓값을 알립니다.
//   --gpu-guard MB  어댑터 전체 VRAM 이 MB 이상인 표본이 두 번 이어지면 이 앱을 바로 내립니다 (다른 프로세스 보호, 기본: --share-gpu 를
//                   주면 어댑터 총량 − 300). 내리면 그 실행의 E2E 는 실패로 끝납니다.
//
// 앱 출력은 out/e2e/app.log, 띄운 프로세스 번호는 out/e2e/app.pid 에 남깁니다.
const { spawn, spawnSync } = require("child_process");
const fs = require("fs");
const http = require("http");
const path = require("path");

const ROOT = path.resolve(__dirname, "../..");
const OUT = path.join(ROOT, "out", "e2e");
const PID_FILE = path.join(OUT, "app.pid");
const args = process.argv.slice(2);
const has = (name) => args.includes(name);
const flag = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : null;
};
const OWN = new Set(["--record", "--i18n", "--app-only", "--stop", "--keep", "--no-wait-cargo", "--rules"]);
const OWN_VALUE = new Set(["--jobs", "--share-gpu", "--gpu-guard"]);
/** 스크립트로 넘길 인자 (run.js 자기 인자는 뺌) */
function passArgs(list) {
  const out = [];
  for (let i = 0; i < list.length; i++) {
    if (OWN.has(list[i])) continue;
    if (OWN_VALUE.has(list[i])) {
      i++;
      continue;
    }
    out.push(list[i]);
  }
  return out;
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (m) => console.log(`[run] ${new Date().toLocaleTimeString()} ${m}`);

function processes() {
  const r = spawnSync("powershell", ["-NoProfile", "-Command", "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,CommandLine | ConvertTo-Json -Compress"], { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  try {
    const v = JSON.parse(r.stdout || "[]");
    return Array.isArray(v) ? v : [v];
  } catch {
    return [];
  }
}

function descendants(all, root) {
  const out = [];
  const queue = [root];
  while (queue.length) {
    const p = queue.shift();
    for (const x of all) {
      if (x.ParentProcessId === p && !out.some((o) => o.ProcessId === x.ProcessId)) {
        out.push(x);
        queue.push(x.ProcessId);
      }
    }
  }
  return out;
}

/** 다른 세션의 Rust 빌드 (이 실행기가 띄운 것이 아닌 cargo · rustc) */
function foreignBuilds(mine = []) {
  return processes().filter((p) => /^(cargo|rustc)\.exe$/i.test(p.Name || "") && !mine.includes(p.ProcessId));
}

async function waitForeignBuilds() {
  if (has("--no-wait-cargo")) return;
  const end = Date.now() + 60 * 60 * 1000;
  let said = false;
  for (;;) {
    const busy = foreignBuilds();
    if (!busy.length) return;
    if (!said) {
      log(`다른 Rust 빌드가 돌고 있어 끝나기를 기다립니다 (${busy.map((b) => `${b.Name}#${b.ProcessId}`).join(", ")})`);
      said = true;
    }
    if (Date.now() > end) {
      log("60분을 기다려도 끝나지 않아 그대로 진행합니다 (--jobs 로 병렬 수를 줄이는 것을 권합니다)");
      return;
    }
    await sleep(10000);
  }
}

function cdpReady() {
  return new Promise((resolve) => {
    const req = http.get("http://127.0.0.1:9222/json/version", (res) => {
      res.resume();
      resolve(res.statusCode === 200);
    });
    req.on("error", () => resolve(false));
    req.setTimeout(2000, () => {
      req.destroy();
      resolve(false);
    });
  });
}

/** 앱을 띄웁니다. 콘솔 창의 디버그 포트가 열릴 때까지 기다립니다 (처음 빌드면 오래 걸림, 최대 60분). */
async function startApp(extraEnv = {}) {
  if (await cdpReady()) throw new Error("포트 9222 에 이미 앱(또는 다른 WebView2)이 떠 있습니다. --stop 으로 내리거나 직접 닫아 주세요");
  await waitForeignBuilds();
  fs.mkdirSync(OUT, { recursive: true });
  const scanner = process.env.BLUEPRINT_SCANNER_DIR || path.join(ROOT, "out", "scanner");
  fs.mkdirSync(scanner, { recursive: true });
  const env = {
    ...process.env,
    CARGO_TARGET_DIR: path.join(ROOT, "target-rocm"),
    BLUEPRINT_DEVICE: "rocm",
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9222",
    BLUEPRINT_SCANNER_DIR: scanner,
    ...extraEnv,
  };
  if (flag("--jobs")) env.CARGO_BUILD_JOBS = flag("--jobs");
  if (has("--rules")) env.BLUEPRINT_ENGINE = "rules";
  const share = Number(flag("--share-gpu") || 0);
  if (share > 0) {
    // 엔진의 기본 상한(시작 때 여유 − 1 GB)은 다른 프로세스가 마침 적게 쓸 때 크게 잡히므로 직접 고정합니다.
    env.BLUEPRINT_VRAM_BUDGET_MB = String(Math.max(900, share - 1200));
    env.CANDLE_ROCM_ALLOC_CACHE_MB = "0";
    env.BLUEPRINT_COEXIST = "off";
    log(`GPU 나눠 쓰기: 이 앱 VRAM 약 ${share} MB (가중치 상한 ${env.BLUEPRINT_VRAM_BUDGET_MB} MB · 버퍼 캐시 0)`);
  }
  const logFile = fs.openSync(path.join(OUT, "app.log"), "w");
  const cmd = `call scripts\\rocm-env.bat && cd app && npx tauri dev --release --no-watch --features rocm -- --no-default-features`;
  // --app-only 는 이 실행기가 끝나도 앱이 남아야 해서 따로 떼어 띄웁니다.
  const detached = has("--app-only");
  const child = spawn("cmd.exe", ["/d", "/s", "/c", cmd], { cwd: ROOT, env, stdio: ["ignore", logFile, logFile], windowsHide: true, detached });
  if (detached) child.unref();
  fs.writeFileSync(PID_FILE, String(child.pid));
  log(`앱을 띄웁니다 (pid ${child.pid}, 출력 out/e2e/app.log, 스캐너 ${scanner})`);
  let exited = null;
  child.on("exit", (code) => (exited = code));
  const end = Date.now() + 60 * 60 * 1000;
  while (!(await cdpReady())) {
    if (exited !== null) throw new Error(`앱이 시작 전에 끝났습니다 (코드 ${exited}). out/e2e/app.log 를 보세요`);
    if (Date.now() > end) throw new Error("60분 안에 앱 창이 뜨지 않았습니다");
    await sleep(3000);
  }
  log("앱 창이 떴습니다 (엔진 준비는 스크립트가 상태 줄로 기다립니다)");
  startVramLog(child.pid);
  return { env, scanner };
}

/** 2초마다 이 앱(blueprint-app)의 VRAM 과 어댑터 전체 사용량을 out/e2e/vram.csv 에 남깁니다 (GPU 를 나눠 쓸 때 상한 확인). */
let sampler = null;
function startVramLog(root) {
  const app = descendants(processes(), root).find((p) => /^blueprint-app\.exe$/i.test(p.Name || ""));
  if (!app) return;
  const ps1 = path.join(OUT, "vram_sampler.ps1");
  // 지킴 한도: 어댑터 전체가 이 값 이상인 표본이 두 번 이어지면 앱 창에 닫기 신호를 두 번 보내 내립니다 (0 이면 기록만)
  const guard = Number(flag("--gpu-guard") || 0) || (Number(flag("--share-gpu") || 0) > 0 ? adapterTotalMb() - 300 : 0);
  if (guard > 0) log(`GPU 지킴: 어댑터 전체 VRAM ${guard} MB 이상이 이어지면 이 앱을 내립니다`);
  fs.writeFileSync(
    ps1,
    [
      "param([int]$AppPid, [string]$Out, [int]$Guard)",
      "'time,app_mb,adapter_mb' | Set-Content -Path $Out -Encoding ascii",
      "$over = 0",
      "while ($true) {",
      "  $p = (Get-Counter '\\GPU Process Memory(*)\\Dedicated Usage' -ErrorAction SilentlyContinue).CounterSamples",
      "  $app = [math]::Round((($p | Where-Object { $_.InstanceName -like \"pid_$($AppPid)_*\" }).CookedValue | Measure-Object -Sum).Sum / 1MB)",
      "  $ad = [math]::Round(((Get-Counter '\\GPU Adapter Memory(*)\\Dedicated Usage' -ErrorAction SilentlyContinue).CounterSamples | Measure-Object -Property CookedValue -Maximum).Maximum / 1MB)",
      "  '{0},{1},{2}' -f (Get-Date -Format 'HH:mm:ss'), $app, $ad | Add-Content -Path $Out -Encoding ascii",
      "  if ($Guard -gt 0 -and $ad -ge $Guard) { $over++ } else { $over = 0 }",
      "  if ($over -ge 2) {",
      "    'GUARD,{0},{1}' -f $app, $ad | Add-Content -Path $Out -Encoding ascii",
      "    taskkill /PID $AppPid | Out-Null; Start-Sleep -Seconds 2; taskkill /PID $AppPid | Out-Null",
      "    break",
      "  }",
      "  Start-Sleep -Seconds 2",
      "}",
    ].join("\r\n"),
  );
  sampler = spawn("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", ps1, "-AppPid", String(app.ProcessId), "-Out", path.join(OUT, "vram.csv"), "-Guard", String(guard)], { stdio: "ignore", windowsHide: true, detached: has("--app-only") });
  if (has("--app-only")) sampler.unref();
}

/** 어댑터 VRAM 총량(MB): 디스플레이 어댑터 레지스트리의 HardwareInformation.qwMemorySize 중 가장 큰 값 */
function adapterTotalMb() {
  const r = spawnSync(
    "powershell",
    [
      "-NoProfile",
      "-Command",
      "(Get-ChildItem 'HKLM:\\SYSTEM\\ControlSet001\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}' -ErrorAction SilentlyContinue | ForEach-Object { (Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue).'HardwareInformation.qwMemorySize' } | Measure-Object -Maximum).Maximum",
    ],
    { encoding: "utf8" },
  );
  const bytes = Number((r.stdout || "").trim());
  return bytes > 0 ? Math.round(bytes / 1048576) : 8192;
}

function vramSummary() {
  try {
    const lines = fs.readFileSync(path.join(OUT, "vram.csv"), "utf8").trim().split(/\r?\n/).slice(1);
    if (lines.some((l) => l.startsWith("GUARD"))) log("GPU 지킴이 어댑터 VRAM 이 한도를 넘어 이 앱을 내렸습니다 (out/e2e/vram.csv 의 GUARD 줄)");
    const rows = lines.filter((l) => !l.startsWith("GUARD")).map((l) => l.split(",").map(Number));
    if (!rows.length) return;
    const app = Math.max(...rows.map((r) => r[1]));
    const adapter = Math.max(...rows.map((r) => r[2]));
    const cap = Number(flag("--share-gpu") || 0);
    log(`VRAM: 이 앱 최대 ${app} MB${cap ? ` (허용 ${cap} MB ${app <= cap ? "안" : "넘음"})` : ""} · 어댑터 전체 최대 ${adapter} MB · 표본 ${rows.length}개 (out/e2e/vram.csv)`);
  } catch {}
}

/** 앱을 내립니다: 창 닫기 신호(두 번) → 남은 빌드 · vite 프로세스는 강제 종료 */
async function stopApp() {
  if (!fs.existsSync(PID_FILE)) {
    log("내릴 앱이 없습니다 (out/e2e/app.pid 없음)");
    return;
  }
  const root = Number(fs.readFileSync(PID_FILE, "utf8"));
  if (sampler) sampler.kill();
  for (const p of processes().filter((x) => /powershell/i.test(x.Name || "") && /vram_sampler\.ps1/i.test(x.CommandLine || ""))) spawnSync("taskkill", ["/PID", String(p.ProcessId), "/F"], { stdio: "ignore" });
  vramSummary();
  const all = processes();
  const tree = descendants(all, root);
  const apps = tree.filter((p) => /^blueprint-app\.exe$/i.test(p.Name || ""));
  for (let i = 0; i < 2; i++) {
    for (const a of apps) spawnSync("taskkill", ["/PID", String(a.ProcessId)], { stdio: "ignore" });
    await sleep(3000);
  }
  for (let i = 0; i < 20 && processes().some((p) => apps.some((a) => a.ProcessId === p.ProcessId)); i++) await sleep(1000);
  const left = descendants(processes(), root).map((p) => p.ProcessId);
  if (left.length || processes().some((p) => p.ProcessId === root)) spawnSync("taskkill", ["/PID", String(root), "/T", "/F"], { stdio: "ignore" });
  fs.rmSync(PID_FILE, { force: true });
  log("앱을 내렸습니다");
  await sleep(2000);
}

function runNode(script, list, env) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [path.join(__dirname, script), ...list], { cwd: ROOT, env, stdio: "inherit" });
    child.on("exit", (code) => resolve(code ?? 1));
  });
}

function runPython(list) {
  const r = spawnSync("python", list, { cwd: ROOT, stdio: "inherit" });
  return r.status ?? 1;
}

(async () => {
  if (has("--stop")) return stopApp();
  const pass = passArgs(args);
  let code = 0;
  if (has("--app-only")) {
    await startApp();
    log("앱을 띄워 둡니다. 끝나면 node scripts/e2e/run.js --stop");
    return;
  }
  if (has("--i18n")) {
    // 1) 한국어 화면에서 백엔드 한글 글을 모두 모으고 2) 번역 표에 없는 글을 찾고 3) 영어 화면으로 같은 흐름을 돕니다.
    const ko = path.join("out", "shots_ko");
    const en = path.join("out", "shots_en");
    let env = (await startApp()).env;
    code = await runNode("console.js", ["--audit", "--shots", ko, ...pass], env);
    await stopApp();
    const misses = runPython([path.join("scripts", "i18n", "build_en.py"), path.join(ko, "i18n_seen.json")]);
    env = (await startApp()).env;
    const enCode = await runNode("console.js", ["--lang", "en", "--shots", en, ...pass], env);
    await stopApp();
    log(`한국어 감사 ${code === 0 ? "통과" : "실패"} · 번역 표 대조 ${misses === 0 ? "끝" : "실패"} (i18n_misses.txt) · 영어 E2E ${enCode === 0 ? "통과" : "실패"} (${en}/i18n_visible.json)`);
    process.exit(code || enCode || misses);
  }
  const record = has("--record");
  const extra = {};
  if (record) {
    const out = path.resolve(ROOT, flag("--out") || path.join("out", "rec", flag("--lang") === "en" ? "master_en" : "master_ko"));
    fs.mkdirSync(out, { recursive: true });
    extra.BLUEPRINT_TRACE = path.join(out, "trace.jsonl");
    extra.BLUEPRINT_TRACE_TEXT = "1";
  }
  try {
    const { env } = await startApp(extra);
    code = await runNode(record ? "record.js" : "console.js", pass, env);
  } catch (e) {
    console.error(`[run] ${e.message || e}`);
    code = 1;
  } finally {
    if (!has("--keep")) await stopApp();
  }
  process.exit(code);
})();
