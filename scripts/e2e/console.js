// 실제 앱의 주제어실 콘솔 · 현장 단말 두 창을 끝까지 돌립니다 (zoro-gravity 의 l4.js 와 같은 방식).
//
//   npm i --no-save playwright-core@1.55.0
//   node scripts/e2e/console.js [--shots 폴더] [--only P2,P1,P3] [--lang en] [--audit]
//
// --lang en  설정 팝오버에서 화면 언어를 English 로 바꾸고 영어 화면으로 같은 흐름을 돕니다 (질의도 영어로 칩니다).
//            스크린샷마다 화면에 남은 한글 줄을 모아 <shots>/i18n_visible.json 에 남깁니다 (번역 표에 없는 글).
// --audit    백엔드가 보낸 한글 글을 모두 모아 <shots>/i18n_seen.json 에 남깁니다 (번역 표를 만들 때, 한국어 화면에서).
//
// 흐름: 발전소 P2 · P1 · P3 · P3F · P4 · A0 · SEARCH · PHYS, CCTV 관제판 CCTV, 도메인 전환 뒤 AI 데이터센터 AIDC(전환) · D2 · D1 · D3 · D3F · D4 ·
// DPHYS, 그리고 PLANT(발전소로 되돌림). 기본은 전부 차례로 돕니다.
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
const ONLY = (flag("--only") || "P2,P1,P3,P3F,P4,A0,SEARCH,PHYS,CCTV,AIDC,D2,D1,D3,D3F,D4,DPHYS,PLANT").split(",");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const EN = flag("--lang") === "en";
const AUDIT = args.includes("--audit");
/** 화면 글 (한국어 화면, 영어 화면) */
const L = (ko, en) => (EN ? en : ko);
/** 영어 화면에서 스크린샷마다 모은 한글 줄 (창 · 장면별) */
const visible = {};
// 현장 단말 화면에 나오면 안 되는 내부 정보 (core/src/privacy.rs FIELD_FORBIDDEN).
// "원장 수치" 만 뺍니다. 기획안의 소명 화면 안내문 "(… 내부 원장 수치 및 판정 코드는 화면에 표시되지 않습니다)" 에 들어 있습니다.
const FORBIDDEN = L(
  ["G1_", "G2_", "G3_", "G4_", "SEC_", "SDS_", "확신도", "Zoro-1 판단", "Zoro-1 글쓰기", "jev_head", "Lattice", "마인드맵", "근본 원인", "고위험", "보팅", "DCS 표시값"],
  ["G1_", "G2_", "G3_", "G4_", "SEC_", "SDS_", "onfidence", "Zoro-1 Verdict", "Zoro-1 judgment", "Zoro-1 Action Procedure", "jev_head", "Lattice", "Mind Map", "oot cause", "oot Cause", "igh-risk", "igh-Risk", "Voting", "DCS reading"],
);
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
      // 콘솔 제목 · 단말 머리는 도메인마다 다릅니다 (발전소 관제 콘솔 · AI 데이터센터 자율 관제 콘솔).
      if ((text.includes("관제 콘솔") && text.includes("공간 Depth")) || (text.includes("Control Console") && text.includes("Spatial Depth"))) consolePage = p;
      if (text.includes("Field Patrol Kiosk")) fieldPage = p;
    }
    if (consolePage && fieldPage) return { consolePage, fieldPage };
    await sleep(2000);
  }
  throw new Error("두 창을 찾지 못했습니다");
}

/** 설정 팝오버의 화면 언어 단추로 언어를 고르고, 두 창이 그 언어로 바뀔 때까지 기다립니다. */
async function setLanguage(c, f, lang) {
  const now = await c.evaluate(() => document.documentElement.lang);
  if (now !== lang) {
    await c.locator(".settings-pill").click();
    await c.locator(`.settings-popover [data-lang="${lang}"]`).click();
    await c.locator(".settings-popover .ov-close").click();
  }
  for (const p of [c, f]) await p.waitForFunction((l) => document.documentElement.lang === l, lang, { timeout: 30000 });
  await sleep(1500);
}

async function shot(page, name) {
  fs.mkdirSync(SHOTS, { recursive: true });
  if (EN) {
    const lines = await page.evaluate(() => document.body.innerText.split("\n").filter((l) => /[가-힣]/.test(l))).catch(() => []);
    if (lines.length) visible[name] = lines;
  }
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

// 2차 개선안(풀스크린 오버레이): 사건 대기열 · 시뮬레이터는 좌상단 [🚨 사건 대기열] 알약의 팝오버 안에 있습니다.
async function openQueue(page) {
  if (await page.locator("#incident-popover").isHidden()) await page.locator(".queue-pill").click();
  await page.locator("#incident-popover").waitFor({ state: "visible", timeout: 10000 });
}

async function closeQueue(page) {
  if (await page.locator("#incident-popover").isVisible()) await page.locator("#incident-popover .ov-close").click();
}

async function inject(page, id) {
  await openQueue(page);
  await page.selectOption("select.sim", id);
}

// 시나리오 주입은 끝나면 새 사건을 스스로 고르고 팝오버를 닫습니다. 그 전에 목록에 뜨면 E2E 가 누릅니다 (어느 쪽이 먼저든 같은 사건).
async function openIncident(page, scenario) {
  const end = Date.now() + 240000;
  while (Date.now() < end) {
    if ((await page.locator(`main.console[data-scenario="${scenario}"]`).count()) > 0) break;
    await openQueue(page).catch(() => {});
    const item = page.locator(".queue-item", { hasText: `${scenario} · ` }).first();
    if (await item.isVisible().catch(() => false)) await item.click({ timeout: 3000 }).catch(() => {});
    await sleep(1500);
  }
  assert((await page.locator(`main.console[data-scenario="${scenario}"]`).count()) > 0, `${scenario} 사건을 열지 못했습니다`);
  await closeQueue(page);
  await sleep(1500);
}

async function clickQueueItem(page, text) {
  await openQueue(page);
  const item = page.locator(".queue-item", { hasText: text }).first();
  await item.waitFor({ timeout: 240000 });
  await item.click();
  await closeQueue(page);
  await sleep(1500);
}

async function stage(page, n) {
  await page.locator(".stage-card").nth(n - 1).click();
  await sleep(800);
}

async function closeDrawer(page) {
  await page.locator(".drawer header button", { hasText: L("닫기", "Close") }).click();
}

// 확정하면 콘솔이 보관 문서 서랍을 스스로 엽니다. 열린 뒤에 닫아야 다음 흐름의 클릭을 가리지 않습니다.
async function closeArchive(page) {
  await waitText(page, L("보관 문서", "Archived Record"), 30000);
  await closeDrawer(page);
}

function placeScan(file) {
  assert(SCAN_DIR, "BLUEPRINT_SCANNER_DIR 를 주세요");
  fs.mkdirSync(SCAN_DIR, { recursive: true });
  fs.copyFileSync(path.join(ROOT, "out", "field", file), path.join(SCAN_DIR, file));
}

// 앞 흐름이 어느 화면에서 멈췄든 첫 화면으로 되돌립니다 (소명 화면에는 "처음으로" 가 없고 "이전 화면" 만 있습니다).
async function fieldReset(page) {
  for (const label of L(["이전 화면", "처음으로"], ["Back", "Start Over"])) {
    const button = page.locator("button", { hasText: label }).first();
    if (await button.count()) {
      await button.click();
      await sleep(500);
    }
  }
  await page.locator("button", { hasText: L("도면/계기 스캔", "Scan Drawing/Gauge") }).first().waitFor({ timeout: 30000 });
}

async function p2(c) {
  await inject(c, "P2");
  await openIncident(c, "P2");
  await shot(c, "P2-1-drawing");
  await c.locator(".tab", { hasText: "Mermaid" }).click();
  await c.waitForSelector(".mermaid-host svg", { timeout: 60000 });
  await shot(c, "P2-2-mermaid");
  await c.locator(".tab", { hasText: L("마인드맵", "Mind Map") }).click();
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
  await c.locator("button", { hasText: L("시계열 · SDS 분석기", "Time-series · SDS Analyzer") }).click();
  await sleep(800);
  await shot(c, "P2-7-sds");
  await closeDrawer(c);
  await stage(c, 5);
  await c.locator("button", { hasText: L("공동 결재 없이 확정 시도", "without co-sign") }).click();
  await waitText(c, L("차단", "blocked"));
  await shot(c, "P2-8-rubber-stamp");
  // 선행 절차 인터록 (Part 8-4): Step 1 은 DCS 값으로 통과, Step 2 (바이패스 개방) 가 남아 확정이 잠깁니다.
  // 오른쪽 통합 결재창에도 같은 신호등 게이트가 있어서 서랍 안의 것을 고릅니다.
  await c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input").check();
  await c.locator(".drawer button", { hasText: L("선행 절차를 건너뛰고 확정 시도", "without prerequisite steps") }).click();
  await waitText(c, "Step 2");
  await shot(c, "P2-8b-interlock-blocked");
  await c.locator(".drawer .il-step", { hasText: "Step 2" }).locator("input").fill("65");
  await c.locator(".drawer .il-step", { hasText: "Step 2" }).locator("button", { hasText: L("현장 값 확인", "Check field value") }).click();
  await waitText(c, L("남은 선행 절차: Step 3", "Remaining prerequisite steps: Step 3"));
  await c.locator(".drawer .il-step", { hasText: "Step 3" }).locator("button", { hasText: L("무전 확인", "radio confirmation") }).click();
  await waitText(c, L("모두 확인", "prerequisite steps confirmed"));
  await sleep(800);
  await shot(c, "P2-8c-interlock-done");
  const cosign = c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input");
  if (!(await cosign.isChecked())) await cosign.check();
  await c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }).click();
  await waitText(c, L("확정했습니다", "Confirmed ("));
  await waitText(c, L("보관 문서", "Archived Record"), 30000);
  await shot(c, "P2-9-approved");
  await closeDrawer(c);
  results.push({ id: "P2", ok: true });
}

async function p1(c, f) {
  await inject(c, "P1");
  await openIncident(c, "P1");
  await waitText(c, L("현장 대기", "Awaiting Field Check"));
  await shot(c, "P1-1-awaiting");
  placeScan("P1_gauge.png");
  await fieldReset(f);
  await f.locator("button", { hasText: L("도면/계기 스캔", "Scan Drawing/Gauge") }).click();
  await f.waitForSelector(".form-grid", { timeout: 300000 });
  await noLeak(f, "점검표 확인");
  await shot(f, "P1-2-field-form");
  await f.locator("button", { hasText: L("주제어실로 전송하기", "Send to Main Control Room") }).click();
  await waitText(f, L("편차가 있어", "deviation between"));
  await noLeak(f, "소명");
  await shot(f, "P1-3-field-excuse");
  await f.locator("input[value=transmitter_drift]").check();
  await f.locator("button", { hasText: L("소명 포함 전송하기", "Send with Justification") }).click();
  await waitText(f, L("접수 번호", "Receipt No."));
  await noLeak(f, "전송");
  await sleep(1500);
  await openIncident(c, "P1");
  await stage(c, 3);
  await shot(c, "P1-4-stage3-truth");
  await closeDrawer(c);
  await stage(c, 5);
  await c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }).click();
  await waitText(c, L("확정했습니다", "Confirmed ("));
  await closeArchive(c);
  await waitText(f, L("처리가 끝났습니다", "Processing complete"), 60000);
  await noLeak(f, "확정 안내");
  await shot(f, "P1-5-field-finished");
  results.push({ id: "P1", ok: true });
}

async function p3(c, f, forged) {
  const id = forged ? "P3F" : "P3";
  await inject(c, id);
  placeScan(forged ? "P3F_redline.png" : "P3_redline.png");
  await fieldReset(f);
  await f.locator("button", { hasText: L("도면/계기 스캔", "Scan Drawing/Gauge") }).click();
  await f.waitForSelector(".form-grid", { timeout: 300000 });
  await shot(f, `${id}-1-field-redline`);
  const permit = f.locator("label.check", { hasText: "PTW" }).locator("input");
  await permit.check();
  await f.locator("button", { hasText: L("주제어실로 전송하기", "Send to Main Control Room") }).click();
  await waitText(f, L("접수 번호", "Receipt No."));
  await noLeak(f, "Red-line 전송");
  await shot(f, `${id}-2-field-sent`);
  await sleep(1500);
  await clickQueueItem(c, L("현장 보고", "Field report"));
  await stage(c, 3);
  await shot(c, `${id}-3-stage3`);
  await closeDrawer(c);
  if (!forged) {
    await stage(c, 5);
    await c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }).click();
    await waitText(c, L("확정했습니다", "Confirmed ("));
    await closeArchive(c);
    await waitText(f, L("반영되었습니다", "was applied"), 60000);
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
  await c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input").check();
  await c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }).click();
  await waitText(c, L("확정했습니다", "Confirmed ("));
  await waitText(c, L("보관 문서", "Archived Record"), 30000);
  await shot(c, "P4-3-approved");
  await closeDrawer(c);
  results.push({ id: "P4", ok: true });
}

// A0 종합 도면 (Part 8-4): 4×4 타일 격자, 선택 설비 타일 강조, 타일 확대
// 앞 흐름이 남긴 확대 창 · 서랍 · 팝오버 · 검색 결과를 닫습니다.
async function tidy(c) {
  if (await c.locator(".cz-card").count()) await c.locator(".cz-card .ov-close").click();
  const close = c.locator(".drawer header button", { hasText: L("닫기", "Close") });
  if (await close.count()) await close.first().click();
  await closeQueue(c);
  if (await c.locator(".settings-popover").isVisible()) await c.locator(".settings-popover .ov-close").click();
  if (await c.locator(".search-results").count()) await c.locator(".search-results .ov-close").click();
  if (await c.locator(".telemetry-dropup").count()) await c.locator(".telemetry-dropup .ov-close").click();
  await sleep(300);
}

/** 판이 모두 닫힌 뒤 ESC 를 누르면 검색 표시 → 설비 선택이 풀립니다 (말풍선이 타일 · 상자를 가리지 않게). */
async function deselect(c) {
  await tidy(c);
  await c.evaluate(() => document.activeElement && document.activeElement.blur && document.activeElement.blur());
  for (let i = 0; i < 4 && (await c.locator(".cctv").count()) > 0; i++) {
    await c.keyboard.press("Escape");
    await sleep(300);
  }
}

async function a0(c) {
  await deselect(c);
  await c.locator(".tab", { hasText: "P&ID" }).click();
  await c.locator(".chip", { hasText: L("A0 종합", "A0 composite") }).click();
  await c.waitForSelector(".a0-sheet img", { timeout: 30000 });
  await sleep(1200);
  await shot(c, "A0-1-sheet");
  await c.locator(".a0-tile", { hasText: "B4" }).first().click({ position: { x: 30, y: 30 } });
  await sleep(1000);
  await shot(c, "A0-2-zoom-B4");
  await c.locator("button", { hasText: L("전체 보기", "Show all") }).click();
  await c.locator(".chip", { hasText: "PID-103-LAB" }).click();
  results.push({ id: "A0", ok: true });
}

// 자연어 검색 (logis-center 방식, 2차 개선안: 하단 허브의 커맨드 바 · 결과 Drop-up · 한 줄 요약)
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
  // 셋째 줄은 선택 설비 문맥 질의: 둘째 질의의 1위(주급수펌프)가 자동으로 골라지고, "상류 1홉" 만 쳐도 그 설비 기준으로 찾습니다.
  const queries = L(
    [
      ["103동 2층 밸브", "10LAB20AA10"],
      ["주급수 유량조절밸브 상류 1홉", "10LAB10AP001"],
      ["상류 1홉", null],
      ["105동 밸브 유인통풍기", "10HLB10AN001"],
      ["승인 대기 MOC", "MOC-2026-1021"],
    ],
    [
      ["valves on floor 2 of building 103", "10LAB20AA10"],
      ["main feedwater flow control valve 1 hop upstream", "10LAB10AP001"],
      ["1 hop upstream", null],
      ["building 105 valve ID fan", "10HLB10AN001"],
      ["pending MOC", "MOC-2026-1021"],
    ],
  );
  let k = 0;
  const record = { id: "SEARCH" };
  for (const [q, want] of queries) {
    k += 1;
    const box = c.locator(".search-input");
    if (want === null) {
      const chip = await c.locator(".command-bar .unit-tag.on").innerText();
      assert(chip.includes("10LAB10AP001"), `앞 질의 1위가 선택 설비 문맥이 되지 않았습니다: ${chip}`);
      await box.fill(q);
      await box.press("Enter");
      await waitText(c, L("연결 기준 설비 10LAB10AP001", "Anchor equipment 10LAB10AP001"), 15000);
      const toast = await c.locator(".cmd-toast.ok, .cmd-toast.info").last().innerText();
      assert(toast.includes(L("문맥 10LAB10AP001", "context 10LAB10AP001")), `문맥 질의 요약에 기준 설비가 없습니다: ${toast}`);
      record.context = toast;
    } else {
      const first = await searchTop(c, q, want);
      assert(first.includes(want), `검색 '${q}' 첫 결과에 ${want} 가 없습니다: ${first}`);
    }
    if (k === 1) {
      // 1위 설비가 골라지고 도면 위 말풍선에 "왜" 리본이 붙습니다
      await c.locator(".cctv .cctv-why").first().waitFor({ timeout: 10000 });
      record.why = await c.locator(".cctv .cctv-why").first().innerText();
    }
    await shot(c, `SEARCH-${k}`);
    await box.fill("");
  }
  await tidy(c);
  results.push({ ...record, ok: true });
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
  await c.locator("button", { hasText: L("물리엔진 · 자율 대응", "Physics Engine") }).click();
  await c.locator(".drawer button", { hasText: L("시뮬레이터 시작", "Start simulator") }).click();
  await sleep(4000);
  await c.locator(".drawer button", { hasText: L("18% 고착 주입", "stuck at 18%") }).click();
  await waitText(c, L("인과 사슬 잠금", "Causal chain locked"), 90000);
  // 사건이 열리면 엔진이 판단 질문(수 초)을 돌립니다. 그동안에도 상태 줄 조회가 바로 돌아와야 합니다 (엔진 잠금은 준비 · 반영 때만).
  const status = await statusLatency(c, 5000);
  await shot(c, "PHYS-1-chain");
  // SDS 고장 코드는 원장 인터록의 고장 코드와 같은 꼴입니다.
  await waitText(c, L("고장 코드 VALVE_STUCK_LOW", "fault code VALVE_STUCK_LOW"), 30000);
  // 고장 중 질적 상태 검색 (개선안 예시): 물리엔진 링버퍼 값으로 상태 조건이 필터로 걸리고, "왜" 에 지금 값이 나옵니다.
  const live = L(
    [
      ["고착된 밸브", "10LAB20AA102", "고착"],
      ["103동 압력 높은 급수펌프", "10LAB10AP001", "압력 높음"],
    ],
    [
      ["stuck valve", "10LAB20AA102", "stuck"],
      ["feedwater pump in building 103 with high pressure", "10LAB10AP001", "pressure high"],
    ],
  );
  let n = 0;
  for (const [q, want, cond] of live) {
    n += 1;
    const first = await searchTop(c, q, want);
    assert(first.includes(want) && first.includes(cond), `고장 중 검색 '${q}' 첫 결과에 ${want} · ${cond} 가 없습니다: ${first}`);
    await shot(c, `PHYS-1${"bc"[n - 1]}-search`);
    await c.locator(".search-input").fill("");
  }
  // 물리엔진 서랍은 허브 위에서 멈추므로 검색 결과판이 서랍 뒤에 남지 않게 닫습니다
  if (await c.locator(".search-results").count()) await c.locator(".search-results .ov-close").click();
  await waitText(c, L("Tier 1 자율 대응", "Tier 1 Autonomous Response"), 240000);
  await shot(c, "PHYS-2-mitigation");
  const run = c.locator(".drawer button", { hasText: L("완화 조치 실행", "Run mitigation") });
  if (await run.count()) await run.click();
  // 유량은 곧 돌아오지만 과열기는 이류 지연(4.2초) 뒤에 꺾입니다. 과열기 자신이 회복으로 넘어간 뒤 고점을 읽습니다.
  await waitText(c, L("10HAH10CT001 완화 조치 → 회복 중", "10HAH10CT001 Mitigating → Recovering"), 180000);
  await sleep(2000);
  await shot(c, "PHYS-3-recovering");
  const text = await c.locator(".physics").innerText();
  const peak = Number((text.match(L(/과열기 고점 ([0-9.]+)/, /Superheater peak ([0-9.]+)/)) || [])[1]);
  const proposed = Number((text.match(L(/제안 t = ([0-9.]+)s/, /proposed t = ([0-9.]+)s/)) || [])[1]);
  const locked = Number((text.match(L(/t = ([0-9.]+)s · 10HAH10CT001 SLEW \(변곡\) → 인과 사슬 잠금/, /t = ([0-9.]+)s · 10HAH10CT001 SLEW \(inflection\) → Causal chain locked/)) || [])[1]);
  const minimum = (text.match(L(/최솟값\(확신도\) ([0-9.]+|없음)/, /min \(confidence\) ([0-9.]+|none)/)) || [])[1];
  const record = { id: "PHYS", peak, locked, proposed, minimum, status_ms: status };
  assert(peak > 0 && peak < 645, `과열기 고점 ${peak} ℃ 가 트립(645) 아래여야 합니다 (${JSON.stringify(record)})`);
  assert(status.max < 1000, `분석 중 상태 줄 조회가 ${status.max.toFixed(0)} ms 기다렸습니다 (${JSON.stringify(status)})`);
  await c.locator(".drawer header button", { hasText: L("닫기", "Close") }).click();
  results.push({ ...record, ok: true });
}

// ───────── 개선안: CCTV형 통합 관제판 ─────────

// 2차 개선안: 도면이 무대 전체에 깔리고(빈 자리 높이를 거의 다 씀), 근본 원인 · 경보 설비에 CCTV 말풍선이 지시선으로 붙고,
// 결재 HUD · 하단 허브(파이프라인 · 3줄 미니 파형 · 질의)가 떠 있는지 봅니다. CAM-02 · 03 은 허브의 [📈] Drop-up 안에 있습니다.
async function visibleCards(c) {
  return c.locator(".cctv").evaluateAll((els) => els.filter((e) => e.style.visibility !== "hidden" && e.offsetWidth > 0).map((e) => e.dataset.card));
}

async function cctv(c) {
  await tidy(c);
  await inject(c, "P2");
  await openIncident(c, "P2");
  await c.locator(".tab", { hasText: "P&ID" }).click();
  await c.waitForSelector(".sheet img", { timeout: 30000 });
  await sleep(1500);
  const text = await c.evaluate(() => document.body.innerText);
  for (const want of L(["감지된 핵심 병목", "인터록 게이트", "표준 조치 요약", "STUCK_LOW", "CAM-EQ"], ["Detected Key Bottlenecks", "Interlock Gate", "Standard Action Summary", "STUCK_LOW", "CAM-EQ"])) {
    assert(text.includes(want), `CCTV 관제판에 '${want}' 가 없습니다`);
  }
  // 도면 크기: 결재 HUD 를 뺀 빈 자리에 맞춰 그립니다 (예전 6 : 4 분할에서는 410 × 290 px 였습니다)
  const geo = await c.evaluate(() => {
    const r = (s) => document.querySelector(s).getBoundingClientRect();
    const st = r(".stage");
    const sh = r(".sheet");
    return { stage: [Math.round(st.width), Math.round(st.height)], sheet: [Math.round(sh.width), Math.round(sh.height)] };
  });
  assert(geo.sheet[1] >= (geo.stage[1] - 60) * 0.85, `도면이 무대 높이를 다 쓰지 않습니다 ${JSON.stringify(geo)}`);
  const cards = await visibleCards(c);
  assert(cards.includes("10LAB20AA102"), `근본 원인(주급수 유량조절밸브)에 CCTV 말풍선이 없습니다: ${cards}`);
  assert(cards.includes("10HAH10BB001"), `다른 도면의 경보 설비(과열기) 말풍선이 점프 커넥터에 붙지 않았습니다: ${cards}`);
  assert((await c.locator(".cctv-remote").count()) > 0, "다른 도면 설비 말풍선에 점프 커넥터 안내가 없습니다");
  const lines = await c.locator(".cctv-lines line").evaluateAll((els) => els.filter((e) => e.style.display !== "none").length);
  assert(lines >= 2, `CCTV 지시선이 ${lines} 개뿐입니다`);
  assert((await c.locator(".lag-label").count()) > 0, "원인 경로 배관에 Δt 라벨이 없습니다");
  assert((await c.locator(".scrub-waves polyline").count()) >= 2, "허브 타임머신에 미니 파형 줄이 없습니다");
  // 말풍선이 결재 HUD · 허브 · 코너 알약과 겹치지 않습니다
  const overlap = await c.evaluate(() => {
    const boxes = [...document.querySelectorAll(".ov-avoid")].map((e) => e.getBoundingClientRect()).filter((r) => r.width > 0);
    return [...document.querySelectorAll(".cctv")]
      .filter((e) => e.style.visibility !== "hidden")
      .map((e) => e.getBoundingClientRect())
      .some((a) => boxes.some((b) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom));
  });
  assert(!overlap, "CCTV 말풍선이 떠 있는 판과 겹칩니다");
  await shot(c, "CCTV-1-p2");
  // 허브의 [📈 CAM-02·03] → 시계열 · SDS 로직 차트 Drop-up
  await c.locator(".charts-toggle").click();
  await c.locator(".telemetry-dropup").waitFor({ timeout: 10000 });
  const charts = await c.locator(".telemetry-dropup").innerText();
  assert(charts.includes("CAM-02") && charts.includes("CAM-03"), "차트 Drop-up 에 CAM-02 · CAM-03 이 없습니다");
  await shot(c, "CCTV-2-charts");
  await c.locator(".telemetry-dropup .ov-close").click();
  // 재생 슬라이더를 발원 직후로 옮기면 SDS 전이와 말풍선 파형이 그 시각으로 바뀝니다.
  await c.locator(".scrubber button", { hasText: L("발원점", "Origin") }).click();
  await sleep(800);
  await shot(c, "CCTV-3-origin");
  // 말풍선 [🔍 파형 확대] → 확대 창, ESC 로 닫힘
  await c.locator('.cctv[data-card="10LAB20AA102"] button', { hasText: L("파형 확대", "Zoom waveform") }).click();
  await c.locator(".cz-card").waitFor({ timeout: 10000 });
  await shot(c, "CCTV-4-zoom");
  await c.keyboard.press("Escape");
  await c.locator(".cz-card").waitFor({ state: "detached", timeout: 5000 });
  // 결재 HUD 를 접으면 도면이 오른쪽 빈 자리까지 넓어집니다
  await c.locator(".approval-hud .bh-toggle").click();
  await sleep(600);
  const wide = await c.evaluate(() => Math.round(document.querySelector(".sheet").getBoundingClientRect().width));
  await shot(c, "CCTV-5-hud-folded");
  await c.locator(".approval-hud .bh-toggle").click();
  await sleep(400);
  // [📋 인터록 이동] → HUD 의 인터록 칸
  await c.locator('.cctv[data-card="10LAB20AA102"] button', { hasText: L("인터록 이동", "Go to interlock") }).click();
  await c.locator(".approval-hud .il-step").first().waitFor({ timeout: 5000 });
  // 발원점(t = 0)에서는 아직 변곡을 지난 노드가 없습니다 (탐지된 첫 변곡은 추정 발원 시각보다 조금 뒤). 분석 시각으로 돌려 핀을 봅니다.
  await c.locator(".scrubber button", { hasText: L("분석 시각", "Analysis time") }).click();
  await c.locator(".tab", { hasText: L("마인드맵", "Mind Map") }).click();
  await sleep(2500);
  assert((await c.locator(".mm-pin").count()) > 0, "마인드맵 변곡 노드에 유리 핀이 없습니다");
  assert((await visibleCards(c)).length > 0, "마인드맵에 CCTV 말풍선이 없습니다");
  await shot(c, "CCTV-6-mindmap");
  await c.locator(".tab", { hasText: "Mermaid" }).click();
  await c.waitForSelector(".mermaid-host svg", { timeout: 60000 });
  await waitText(c, "SDS:", 30000);
  await sleep(800);
  assert((await visibleCards(c)).length > 0, "Mermaid 노드에 CCTV 말풍선이 없습니다");
  await shot(c, "CCTV-7-mermaid");
  await c.locator(".tab", { hasText: "P&ID" }).click();
  results.push({ id: "CCTV", ok: true, ...geo, folded_width: wide, cards });
}

// ───────── 추가 개선안: AI 데이터센터 모드 ─────────

async function switchDomain(c, label, brand) {
  await tidy(c);
  await c.locator(".domain-switch .seg", { hasText: label }).click();
  await waitText(c, brand, 60000);
  await sleep(1500);
}

async function aidc(c, f) {
  await switchDomain(c, L("AI 데이터센터", "AI Data Center"), L("AI 데이터센터 자율 관제 콘솔", "AI Data Center Autonomous Control Console"));
  await waitText(f, L("AI 데이터센터 현장 엔지니어 단말", "AI Data Center Field Engineer Terminal"), 30000);
  // 도면 칩은 도면 뷰의 도구줄에 있습니다 (앞 흐름이 다른 뷰에서 멈췄을 수 있음)
  await c.locator(".tab", { hasText: L("도면", "Drawing") }).click();
  await sleep(800);
  const text = await c.evaluate(() => document.body.innerText);
  assert(text.includes("MEP-103-CDU"), "데이터센터 도면 칩이 없습니다");
  assert(!text.includes("INC-"), "도메인을 바꾸면 사건 목록이 비어야 합니다");
  await shot(c, "AIDC-1-console");
  await shot(f, "AIDC-2-kiosk");
  results.push({ id: "AIDC", ok: true });
}

async function d2(c) {
  await inject(c, "D2");
  await openIncident(c, "D2");
  await c.locator(".tab", { hasText: "MEP" }).click();
  await sleep(1500);
  const text = await c.evaluate(() => document.body.innerText);
  for (const want of ["DC1CDU20AA02", "EOP-103-CDU02", L("셧다운", "hutdown")]) assert(text.includes(want), `D2 화면에 '${want}' 가 없습니다`);
  await shot(c, "D2-1-cctv");
  await c.locator(".tab", { hasText: "Mermaid" }).click();
  await c.waitForSelector(".mermaid-host svg", { timeout: 60000 });
  await shot(c, "D2-2-mermaid");
  await c.locator(".tab", { hasText: L("마인드맵", "Mind Map") }).click();
  await sleep(2500);
  await shot(c, "D2-3-mindmap");
  await c.locator(".tab", { hasText: "MEP" }).click();
  await stage(c, 4);
  await shot(c, "D2-4-stage4");
  await closeDrawer(c);
  await stage(c, 5);
  await c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input").check();
  await c.locator(".drawer button", { hasText: L("선행 절차를 건너뛰고 확정 시도", "without prerequisite steps") }).click();
  await waitText(c, "Step 2");
  await c.locator(".drawer .il-step", { hasText: "Step 2" }).locator("input").fill("65");
  await c.locator(".drawer .il-step", { hasText: "Step 2" }).locator("button", { hasText: L("현장 값 확인", "Check field value") }).click();
  await waitText(c, L("남은 선행 절차: Step 3", "Remaining prerequisite steps: Step 3"));
  await c.locator(".drawer .il-step", { hasText: "Step 3" }).locator("button", { hasText: L("무전 확인", "radio confirmation") }).click();
  await waitText(c, L("모두 확인", "prerequisite steps confirmed"));
  const cosign = c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input");
  if (!(await cosign.isChecked())) await cosign.check();
  await c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }).click();
  await waitText(c, L("확정했습니다", "Confirmed ("));
  await waitText(c, L("보관 문서", "Archived Record"), 30000);
  const archive = await c.locator(".drawer").innerText();
  assert(archive.includes("NOC-2026-") && archive.includes(L("NOC 관제실 결재", "NOC approval")), "데이터센터 보관 문서는 NOC 번호 · 관제실 결재여야 합니다");
  await shot(c, "D2-5-approved");
  await closeDrawer(c);
  results.push({ id: "D2", ok: true });
}

async function d1(c, f) {
  await inject(c, "D1");
  await openIncident(c, "D1");
  await waitText(c, L("현장 대기", "Awaiting Field Check"));
  placeScan("D1_gauge.png");
  await fieldReset(f);
  await f.locator("button", { hasText: L("도면/계기 스캔", "Scan Drawing/Gauge") }).click();
  await f.waitForSelector(".form-grid", { timeout: 300000 });
  await noLeak(f, "D1 점검표 확인");
  const crops = await f.locator(".crop").count();
  const kks = await f.locator(".field-card", { hasText: L("자산코드", "Asset code") }).locator("input").inputValue();
  await shot(f, "D1-1-field-cards");
  assert(kks === "DC1MNF10MV01", `점검표 자산 코드를 ${kks} 로 읽었습니다`);
  assert(crops >= 2, `원터치 대조 카드의 스캔 조각이 ${crops} 개뿐입니다`);
  // 스테퍼: +0.1 → -0.1 로 값이 돌아옵니다 (장갑 입력)
  const reading = f.locator(".field-card", { hasText: L("현장 실측치", "Field Reading") });
  const before = await reading.locator("input").inputValue();
  await reading.locator("button.step", { hasText: "+0.1" }).click();
  await reading.locator("button.step", { hasText: "-0.1" }).click();
  assert(Number(await reading.locator("input").inputValue()) === Number(before), "스테퍼가 값을 되돌리지 못했습니다");
  await f.locator("button", { hasText: L("NOC 관제실로 전송하기", "Send to NOC") }).click();
  await waitText(f, L("편차가 있어", "deviation between"));
  await noLeak(f, "D1 소명");
  await f.locator("input[value=transmitter_drift]").check();
  await f.locator("button", { hasText: L("소명 포함 전송하기", "Send with Justification") }).click();
  await waitText(f, L("접수 번호", "Receipt No."));
  await sleep(1500);
  await openIncident(c, "D1");
  await stage(c, 3);
  await shot(c, "D1-2-stage3-truth");
  await closeDrawer(c);
  await stage(c, 5);
  await c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }).click();
  await waitText(c, L("확정했습니다", "Confirmed ("));
  await closeArchive(c);
  await waitText(f, L("처리가 끝났습니다", "Processing complete"), 60000);
  await noLeak(f, "D1 확정 안내");
  await shot(f, "D1-3-field-finished");
  results.push({ id: "D1", ok: true, crops });
}

async function d3(c, f, forged) {
  const id = forged ? "D3F" : "D3";
  await inject(c, id);
  placeScan(forged ? "D3F_redline.png" : "D3_redline.png");
  await fieldReset(f);
  await f.locator("button", { hasText: L("도면/계기 스캔", "Scan Drawing/Gauge") }).click();
  await f.waitForSelector(".form-grid", { timeout: 300000 });
  const moc = await f.locator(".field-card", { hasText: L("MOP 변경번호", "MOP No.") }).locator("input").inputValue();
  await shot(f, `${id}-1-field-redline`);
  assert(moc === "MOP-2026-1104", `Red-line 변경번호를 ${moc} 로 읽었습니다`);
  await f.locator("label.check", { hasText: "MOP/Hot-Work" }).locator("input").check();
  await f.locator("button", { hasText: L("NOC 관제실로 전송하기", "Send to NOC") }).click();
  await waitText(f, L("접수 번호", "Receipt No."));
  await noLeak(f, `${id} 전송`);
  await sleep(1500);
  await clickQueueItem(c, L("현장 보고", "Field report"));
  await stage(c, 3);
  await shot(c, `${id}-2-stage3`);
  await closeDrawer(c);
  if (!forged) {
    await stage(c, 5);
    await c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }).click();
    await waitText(c, L("확정했습니다", "Confirmed ("));
    await closeArchive(c);
    await waitText(f, L("반영되었습니다", "was applied"), 60000);
    await shot(f, `${id}-3-field-finished`);
  }
  results.push({ id, ok: true });
}

async function d4(c) {
  await inject(c, "D4");
  await openIncident(c, "D4");
  await stage(c, 5);
  const text = await c.locator(".drawer").innerText();
  assert(text.includes("EPO"), "배터리 절차서의 고위험 조치(EPO)가 보이지 않습니다");
  await c.locator(".drawer button", { hasText: L("공동 결재 없이 확정 시도", "without co-sign") }).click();
  await waitText(c, L("차단", "blocked"));
  await c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input").check();
  await c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }).click();
  await waitText(c, L("확정했습니다", "Confirmed ("));
  await waitText(c, L("보관 문서", "Archived Record"), 30000);
  await shot(c, "D4-1-approved");
  await closeDrawer(c);
  results.push({ id: "D4", ok: true });
}

// 데이터센터 물리엔진: CDU 밸브 고착 → GPU 정션 온도 상승 → 인과 사슬 → CDU 우회밸브 완화 → 셧다운(95 ℃) 전 회복
async function dphys(c) {
  await tidy(c);
  await c.locator("button", { hasText: L("물리엔진 · 자율 대응", "Physics Engine") }).click();
  await c.locator(".drawer button", { hasText: L("시뮬레이터 시작", "Start simulator") }).click();
  await sleep(4000);
  await c.locator(".drawer button", { hasText: L("18% 고착 주입", "stuck at 18%") }).click();
  await waitText(c, L("인과 사슬 잠금", "Causal chain locked"), 90000);
  await shot(c, "DPHYS-1-chain");
  await waitText(c, L("고장 코드 VALVE_STUCK_LOW", "fault code VALVE_STUCK_LOW"), 30000);
  await waitText(c, L("Tier 1 자율 대응", "Tier 1 Autonomous Response"), 240000);
  await shot(c, "DPHYS-2-mitigation");
  const run = c.locator(".drawer button", { hasText: L("완화 조치 실행", "Run mitigation") });
  if (await run.count()) await run.click();
  await waitText(c, L("DC1GPU10CT01 완화 조치 → 회복 중", "DC1GPU10CT01 Mitigating → Recovering"), 180000);
  await sleep(2000);
  await shot(c, "DPHYS-3-recovering");
  const text = await c.locator(".physics").innerText();
  const peak = Number((text.match(L(/고점 ([0-9.]+)/, /peak ([0-9.]+)/)) || [])[1]);
  assert(peak > 0 && peak < 95, `GPU 정션 고점 ${peak} ℃ 가 셧다운(95) 아래여야 합니다`);
  await c.locator(".drawer header button", { hasText: L("닫기", "Close") }).click();
  results.push({ id: "DPHYS", ok: true, peak });
}

async function plant(c, f) {
  await switchDomain(c, L("화력발전소", "Thermal Power Plant"), L("1.2GW 발전소 관제 콘솔", "1.2GW Power Plant Control Console"));
  await waitText(f, L("1.2GW 발전소 현장 순시", "1.2GW Power Plant Field Patrol"), 30000);
  results.push({ id: "PLANT", ok: true });
}

(async () => {
  const browser = await connect();
  const { consolePage: c, fieldPage: f } = await pages(browser);
  // 화면의 단계 이름에도 "Zoro-1" 이 있으니, 아래 상태 줄이 모델을 다 연 뒤의 문구로 바뀔 때까지 기다립니다.
  await c.waitForFunction(() => /(엔진|Engine): Zoro-1/.test(document.body.innerText), null, { timeout: 900000 });
  if (AUDIT) for (const p of [c, f]) await p.evaluate(() => (window.__i18nAudit = true));
  await setLanguage(c, f, EN ? "en" : "ko");
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
      if (id === "CCTV") await cctv(c);
      if (id === "AIDC") await aidc(c, f);
      if (id === "D2") await d2(c);
      if (id === "D1") await d1(c, f);
      if (id === "D3") await d3(c, f, false);
      if (id === "D3F") await d3(c, f, true);
      if (id === "D4") await d4(c);
      if (id === "DPHYS") await dphys(c);
      if (id === "PLANT") await plant(c, f);
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
  if (AUDIT) {
    const seen = new Set();
    for (const p of [c, f]) for (const l of await p.evaluate(() => (window.__i18nSeen ? window.__i18nSeen() : [])).catch(() => [])) seen.add(l);
    fs.writeFileSync(path.join(SHOTS, "i18n_seen.json"), JSON.stringify([...seen].sort(), null, 1));
  }
  if (EN) {
    const misses = new Set();
    for (const p of [c, f]) for (const l of await p.evaluate(() => (window.__i18nMisses ? window.__i18nMisses() : [])).catch(() => [])) misses.add(l);
    fs.writeFileSync(path.join(SHOTS, "i18n_visible.json"), JSON.stringify({ misses: [...misses].sort(), screens: visible }, null, 1));
  }
  console.log(JSON.stringify(results, null, 2));
  await browser.close().catch(() => {});
  process.exit(results.every((r) => r.ok) ? 0 : 1);
})();
