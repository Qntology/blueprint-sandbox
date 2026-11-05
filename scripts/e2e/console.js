// 실제 앱의 주제어실 콘솔 · 현장 단말 두 창을 끝까지 돌립니다 (zoro-gravity 의 l4.js 와 같은 방식).
//
//   npm i --no-save playwright-core@1.55.0
//   node scripts/e2e/console.js [--shots 폴더] [--only P2,P1,P3]
//
// 앱은 먼저 띄워 둡니다. WebView2 에 디버그 포트를 열고, 스캐너 폴더를 같은 값으로 줍니다.
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
//   BLUEPRINT_SCANNER_DIR=<폴더>
// 현장 스캔 이미지는 `python scripts/gen_pid_drawings.py` 가 out/field 에 만듭니다.
const { chromium } = require("playwright-core");
const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "../..");
const SCAN_DIR = process.env.BLUEPRINT_SCANNER_DIR;
const args = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : null;
};
const SHOTS = flag("--shots") || path.join(ROOT, "out", "shots");
const ONLY = (flag("--only") || "P2,P1,P3,P3F,P4,A0,SEARCH,PHYS").split(",");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// 현장 단말 화면에 나오면 안 되는 내부 정보 (core/src/privacy.rs FIELD_FORBIDDEN).
// "원장 수치" 만 뺍니다. 기획안의 소명 화면 안내문 "(… 내부 원장 수치 및 판정 코드는 화면에 표시되지 않습니다)" 에 들어 있습니다.
const FORBIDDEN = ["G1_", "G2_", "G3_", "G4_", "SEC_", "SDS_", "확신도", "Zoro-1 판단", "Zoro-1 글쓰기", "jev_head", "Lattice", "마인드맵", "근본 원인", "고위험", "보팅", "DCS 표시값"];
const results = [];

function assert(cond, message) {
  if (!cond) throw new Error(message);
}

async function connect() {
  for (let i = 0; i < 150; i++) {
    try {
      return await chromium.connectOverCDP("http://127.0.0.1:9222");
    } catch {
      await sleep(2000);
    }
  }
  throw new Error("CDP 연결 시간 초과");
}

async function pages(browser) {
  for (let i = 0; i < 150; i++) {
    const all = browser.contexts().flatMap((c) => c.pages());
    let consolePage = null;
    let fieldPage = null;
    for (const p of all) {
      const text = await p.evaluate(() => document.body.innerText).catch(() => "");
      if (text.includes("발전소 관제 콘솔")) consolePage = p;
      if (text.includes("현장 순시")) fieldPage = p;
    }
    if (consolePage && fieldPage) return { consolePage, fieldPage };
    await sleep(2000);
  }
  throw new Error("두 창을 찾지 못했습니다");
}

async function shot(page, name) {
  fs.mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: path.join(SHOTS, `${name}.png`) });
}

async function noLeak(page, where) {
  const text = await page.evaluate(() => document.body.innerText);
  const found = FORBIDDEN.filter((w) => text.includes(w));
  assert(found.length === 0, `현장 단말에 내부 정보가 보입니다 (${where}): ${found.join(", ")}`);
}

async function waitText(page, text, timeout = 240000) {
  await page.waitForFunction((t) => document.body.innerText.includes(t), text, { timeout });
}

async function inject(page, id) {
  await page.selectOption("select.sim", id);
}

async function openIncident(page, scenario) {
  const item = page.locator(".queue-item", { hasText: `${scenario} · ` }).first();
  await item.waitFor({ timeout: 240000 });
  await item.click();
  await sleep(1500);
}

async function stage(page, n) {
  await page.locator(".stage-card").nth(n - 1).click();
  await sleep(800);
}

async function closeDrawer(page) {
  await page.locator(".drawer header button", { hasText: "닫기" }).click();
}

// 확정하면 콘솔이 보관 문서 서랍을 스스로 엽니다. 열린 뒤에 닫아야 다음 흐름의 클릭을 가리지 않습니다.
async function closeArchive(page) {
  await waitText(page, "보관 문서", 30000);
  await closeDrawer(page);
}

function placeScan(file) {
  assert(SCAN_DIR, "BLUEPRINT_SCANNER_DIR 를 주세요");
  fs.mkdirSync(SCAN_DIR, { recursive: true });
  fs.copyFileSync(path.join(ROOT, "out", "field", file), path.join(SCAN_DIR, file));
}

// 앞 흐름이 어느 화면에서 멈췄든 첫 화면으로 되돌립니다 (소명 화면에는 "처음으로" 가 없고 "이전 화면" 만 있습니다).
async function fieldReset(page) {
  for (const label of ["이전 화면", "처음으로"]) {
    const button = page.locator("button", { hasText: label }).first();
    if (await button.count()) {
      await button.click();
      await sleep(500);
    }
  }
  await page.locator("button", { hasText: "도면/계기 스캔" }).first().waitFor({ timeout: 30000 });
}

async function p2(c) {
  await inject(c, "P2");
  await openIncident(c, "P2");
  await shot(c, "P2-1-drawing");
  await c.locator(".tab", { hasText: "Mermaid" }).click();
  await c.waitForSelector(".mermaid-host svg", { timeout: 60000 });
  await shot(c, "P2-2-mermaid");
  await c.locator(".tab", { hasText: "마인드맵" }).click();
  await sleep(2500);
  await shot(c, "P2-3-mindmap");
  await c.locator(".mining-toggle input").check();
  await sleep(2500);
  await shot(c, "P2-4-mindmap-mining");
  await c.locator(".tab", { hasText: "P&ID" }).click();
  await sleep(1500);
  await shot(c, "P2-5-drawing-mining");
  await c.locator(".mining-toggle input").uncheck();
  await stage(c, 4);
  await shot(c, "P2-6-stage4");
  await closeDrawer(c);
  await c.locator("button", { hasText: "시계열 · SDS 분석기" }).click();
  await sleep(800);
  await shot(c, "P2-7-sds");
  await closeDrawer(c);
  await stage(c, 5);
  await c.locator("button", { hasText: "공동 결재 없이 확정 시도" }).click();
  await waitText(c, "차단");
  await shot(c, "P2-8-rubber-stamp");
  // 선행 절차 인터록 (Part 8-4): Step 1 은 DCS 값으로 통과, Step 2 (바이패스 개방) 가 남아 확정이 잠깁니다.
  await c.locator(".drawer label.check", { hasText: "공동 결재" }).locator("input").check();
  await c.locator(".drawer button", { hasText: "선행 절차를 건너뛰고 확정 시도" }).click();
  await waitText(c, "Step 2");
  await shot(c, "P2-8b-interlock-blocked");
  await c.locator(".il-step", { hasText: "Step 2" }).locator("input").fill("65");
  await c.locator(".il-step", { hasText: "Step 2" }).locator("button", { hasText: "현장 값 확인" }).click();
  await waitText(c, "남은 선행 절차: Step 3");
  await c.locator(".il-step", { hasText: "Step 3" }).locator("button", { hasText: "무전 확인" }).click();
  await waitText(c, "모두 확인");
  await sleep(800);
  await shot(c, "P2-8c-interlock-done");
  const cosign = c.locator(".drawer label.check", { hasText: "공동 결재" }).locator("input");
  if (!(await cosign.isChecked())) await cosign.check();
  await c.locator(".drawer button", { hasText: "조치확정" }).click();
  await waitText(c, "확정했습니다");
  await waitText(c, "보관 문서", 30000);
  await shot(c, "P2-9-approved");
  await closeDrawer(c);
  results.push({ id: "P2", ok: true });
}

async function p1(c, f) {
  await inject(c, "P1");
  await openIncident(c, "P1");
  await waitText(c, "현장 대기");
  await shot(c, "P1-1-awaiting");
  placeScan("P1_gauge.png");
  await fieldReset(f);
  await f.locator("button", { hasText: "도면/계기 스캔" }).click();
  await f.waitForSelector(".form-grid", { timeout: 300000 });
  await noLeak(f, "점검표 확인");
  await shot(f, "P1-2-field-form");
  await f.locator("button", { hasText: "주제어실로 전송하기" }).click();
  await waitText(f, "편차가 있어");
  await noLeak(f, "소명");
  await shot(f, "P1-3-field-excuse");
  await f.locator("input[value=transmitter_drift]").check();
  await f.locator("button", { hasText: "소명 포함 전송하기" }).click();
  await waitText(f, "접수 번호");
  await noLeak(f, "전송");
  await sleep(1500);
  await openIncident(c, "P1");
  await stage(c, 3);
  await shot(c, "P1-4-stage3-truth");
  await closeDrawer(c);
  await stage(c, 5);
  await c.locator(".drawer button", { hasText: "조치확정" }).click();
  await waitText(c, "확정했습니다");
  await closeArchive(c);
  await waitText(f, "처리가 끝났습니다", 60000);
  await noLeak(f, "확정 안내");
  await shot(f, "P1-5-field-finished");
  results.push({ id: "P1", ok: true });
}

async function p3(c, f, forged) {
  const id = forged ? "P3F" : "P3";
  await inject(c, id);
  placeScan(forged ? "P3F_redline.png" : "P3_redline.png");
  await fieldReset(f);
  await f.locator("button", { hasText: "도면/계기 스캔" }).click();
  await f.waitForSelector(".form-grid", { timeout: 300000 });
  await shot(f, `${id}-1-field-redline`);
  const permit = f.locator("label.check", { hasText: "PTW" }).locator("input");
  await permit.check();
  await f.locator("button", { hasText: "주제어실로 전송하기" }).click();
  await waitText(f, "접수 번호");
  await noLeak(f, "Red-line 전송");
  await shot(f, `${id}-2-field-sent`);
  await sleep(1500);
  const item = c.locator(".queue-item", { hasText: "현장 보고" }).first();
  await item.click();
  await sleep(1500);
  await stage(c, 3);
  await shot(c, `${id}-3-stage3`);
  await closeDrawer(c);
  if (!forged) {
    await stage(c, 5);
    await c.locator(".drawer button", { hasText: "조치확정" }).click();
    await waitText(c, "확정했습니다");
    await closeArchive(c);
    await waitText(f, "반영되었습니다", 60000);
    await shot(f, `${id}-4-field-finished`);
  }
  results.push({ id, ok: true });
}

// P4: 상류 이상 없는 베어링 진동 단독 고장. 판정은 고위험이고 절차서(AOP-102-TB01)에 감발 · 정지가 있어 공동 결재로만 확정합니다.
async function p4(c) {
  await inject(c, "P4");
  await openIncident(c, "P4");
  await stage(c, 4);
  await shot(c, "P4-1-stage4");
  await closeDrawer(c);
  await stage(c, 5);
  await shot(c, "P4-2-stage5");
  await c.locator(".drawer label.check", { hasText: "공동 결재" }).locator("input").check();
  await c.locator(".drawer button", { hasText: "조치확정" }).click();
  await waitText(c, "확정했습니다");
  await waitText(c, "보관 문서", 30000);
  await shot(c, "P4-3-approved");
  await closeDrawer(c);
  results.push({ id: "P4", ok: true });
}

// A0 종합 도면 (Part 8-4): 4×4 타일 격자, 선택 설비 타일 강조, 타일 확대
// 앞 흐름이 남긴 검색 목록 · 서랍을 닫습니다.
async function tidy(c) {
  if (await c.locator(".search-results").count()) await c.locator(".search-input").press("Escape");
  const close = c.locator(".drawer header button", { hasText: "닫기" });
  if (await close.count()) await close.first().click();
  await sleep(300);
}

async function a0(c) {
  await tidy(c);
  await c.locator(".tab", { hasText: "P&ID" }).click();
  await c.locator(".chip", { hasText: "A0 종합" }).click();
  await c.waitForSelector(".a0-sheet img", { timeout: 30000 });
  await sleep(1200);
  await shot(c, "A0-1-sheet");
  await c.locator(".a0-tile", { hasText: "B4" }).first().click({ position: { x: 30, y: 30 } });
  await sleep(1000);
  await shot(c, "A0-2-zoom-B4");
  await c.locator("button", { hasText: "전체 보기" }).click();
  await c.locator(".chip", { hasText: "PID-103-LAB" }).click();
  results.push({ id: "A0", ok: true });
}

// 자연어 검색 (logis-center 방식)
/** 질의를 넣고 결과 첫 줄을 돌려줍니다. 결과가 이 질의의 것으로 바뀔 때까지(첫 줄에 기대값이 나오거나 10초) 기다립니다. */
async function searchTop(c, q, want) {
  const box = c.locator(".search-input");
  await box.fill(q);
  await box.press("Enter");
  await c.waitForSelector(".search-results", { timeout: 15000 });
  let first = "";
  for (let i = 0; i < 20; i++) {
    first = await c.locator(".search-hit").first().innerText().catch(() => "");
    if (first.includes(want)) break;
    await sleep(500);
  }
  return first;
}

async function searchFlow(c) {
  await tidy(c);
  const queries = [
    ["103동 2층 밸브", "10LAB20AA10"],
    ["주급수 유량조절밸브 상류 1홉", "10LAB10AP001"],
    ["105동 밸브 유인통풍기", "10HLB10AN001"],
    ["승인 대기 MOC", "MOC-2026-1021"],
  ];
  let k = 0;
  for (const [q, want] of queries) {
    k += 1;
    const box = c.locator(".search-input");
    const first = await searchTop(c, q, want);
    assert(first.includes(want), `검색 '${q}' 첫 결과에 ${want} 가 없습니다: ${first}`);
    await shot(c, `SEARCH-${k}`);
    await box.press("Escape");
  }
  await tidy(c);
  results.push({ id: "SEARCH", ok: true });
}

// 물리엔진 폐루프 (Part 8-5): 정상 운전 → 밸브 고착 주입 → SDS 추적 → 사건 → 완화 조치 (보조 모드면 교대감독 실행) → 회복
// 엔진이 사건을 모델로 분석하는 동안 상태 줄 명령(console_status, 엔진 잠금을 잡음)이 얼마나 기다리는지 잽니다.
async function statusLatency(c, ms) {
  return await c.evaluate(async (dur) => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const took = [];
    const end = performance.now() + dur;
    while (performance.now() < end) {
      const t = performance.now();
      await invoke("console_status");
      took.push(performance.now() - t);
      await new Promise((r) => setTimeout(r, 100));
    }
    took.sort((a, b) => a - b);
    return { n: took.length, median: took[Math.floor(took.length / 2)], max: took[took.length - 1] };
  }, ms);
}

async function phys(c) {
  await tidy(c);
  await c.locator("button", { hasText: "물리엔진 · 자율 대응" }).click();
  await c.locator(".drawer button", { hasText: "시뮬레이터 시작" }).click();
  await sleep(4000);
  await c.locator(".drawer button", { hasText: "급수밸브 18% 고착 주입" }).click();
  await waitText(c, "인과 사슬 잠금", 90000);
  // 사건이 열리면 엔진이 판단 질문(수 초)을 돌립니다. 그동안에도 상태 줄 조회가 바로 돌아와야 합니다 (엔진 잠금은 준비 · 반영 때만).
  const status = await statusLatency(c, 5000);
  await shot(c, "PHYS-1-chain");
  // SDS 고장 코드는 원장 인터록의 고장 코드와 같은 꼴입니다.
  await waitText(c, "고장 코드 VALVE_STUCK_LOW", 30000);
  // 고장 중 질적 상태 검색 (개선안 예시): 물리엔진 링버퍼 값으로 상태 조건이 필터로 걸리고, "왜" 에 지금 값이 나옵니다.
  const live = [
    ["고착된 밸브", "10LAB20AA102", "고착"],
    ["103동 압력 높은 급수펌프", "10LAB10AP001", "압력 높음"],
  ];
  let n = 0;
  for (const [q, want, cond] of live) {
    n += 1;
    const first = await searchTop(c, q, want);
    assert(first.includes(want) && first.includes(cond), `고장 중 검색 '${q}' 첫 결과에 ${want} · ${cond} 가 없습니다: ${first}`);
    await shot(c, `PHYS-1${"bc"[n - 1]}-search`);
    await c.locator(".search-input").press("Escape");
  }
  await waitText(c, "Tier 1 자율 대응", 240000);
  await shot(c, "PHYS-2-mitigation");
  const run = c.locator(".drawer button", { hasText: "완화 조치 실행" });
  if (await run.count()) await run.click();
  // 유량은 곧 돌아오지만 과열기는 이류 지연(4.2초) 뒤에 꺾입니다. 과열기 자신이 회복으로 넘어간 뒤 고점을 읽습니다.
  await waitText(c, "10HAH10CT001 완화 조치 → 회복 중", 180000);
  await sleep(2000);
  await shot(c, "PHYS-3-recovering");
  const text = await c.locator(".physics").innerText();
  const peak = Number((text.match(/과열기 고점 ([0-9.]+)/) || [])[1]);
  const proposed = Number((text.match(/제안 t = ([0-9.]+)s/) || [])[1]);
  const locked = Number((text.match(/t = ([0-9.]+)s · 10HAH10CT001 SLEW \(변곡\) → 인과 사슬 잠금/) || [])[1]);
  const minimum = (text.match(/최솟값\(확신도\) ([0-9.]+|없음)/) || [])[1];
  const record = { id: "PHYS", peak, locked, proposed, minimum, status_ms: status };
  assert(peak > 0 && peak < 645, `과열기 고점 ${peak} ℃ 가 트립(645) 아래여야 합니다 (${JSON.stringify(record)})`);
  assert(status.max < 1000, `분석 중 상태 줄 조회가 ${status.max.toFixed(0)} ms 기다렸습니다 (${JSON.stringify(status)})`);
  await c.locator(".drawer header button", { hasText: "닫기" }).click();
  results.push({ ...record, ok: true });
}

(async () => {
  const browser = await connect();
  const { consolePage: c, fieldPage: f } = await pages(browser);
  // 화면의 단계 이름에도 "Zoro-1" 이 있으니, 아래 상태 줄이 모델을 다 연 뒤의 문구로 바뀔 때까지 기다립니다.
  await waitText(c, "엔진: Zoro-1", 900000);
  for (const id of ONLY) {
    try {
      if (id === "P2") await p2(c);
      if (id === "P1") await p1(c, f);
      if (id === "P3") await p3(c, f, false);
      if (id === "P3F") await p3(c, f, true);
      if (id === "P4") await p4(c);
      if (id === "A0") await a0(c);
      if (id === "SEARCH") await searchFlow(c);
      if (id === "PHYS") await phys(c);
    } catch (e) {
      results.push({ id, ok: false, error: String(e) });
      await shot(c, `${id}-error-console`).catch(() => {});
      await shot(f, `${id}-error-field`).catch(() => {});
    }
  }
  // --only 로 일부만 다시 돌리면 앞 실행의 다른 시나리오 결과는 남기고 같은 시나리오만 바꿉니다.
  const file = path.join(SHOTS, "results.json");
  let merged = results;
  try {
    const before = JSON.parse(fs.readFileSync(file, "utf8"));
    merged = [...before.filter((r) => !results.some((x) => x.id === r.id)), ...results];
  } catch {}
  fs.writeFileSync(file, JSON.stringify(merged, null, 2));
  console.log(JSON.stringify(results, null, 2));
  await browser.close().catch(() => {});
  process.exit(results.every((r) => r.ok) ? 0 : 1);
})();
