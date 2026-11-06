// 시연 영상 촬영 (docs/video.md 의 마스터 기획 6파트). 실제 앱의 콘솔 · 현장 단말 두 창을 CDP 화면 캐스트로 녹화하고,
// 장면 표시(marks) · 백엔드 스냅숏(snaps)을 남깁니다. 편집은 scripts/e2e/edit_video.py 가 합니다.
//
//   node scripts/e2e/record.js [--out out/rec/master_ko] [--lang en] [--audit] [--only INTRO,SEARCH,P2,...]
//
// 앱은 먼저 띄워 둡니다 (console.js 와 같음). 모델 장면을 그리려면 모델 사용 기록을 남기게 띄웁니다.
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
//   BLUEPRINT_SCANNER_DIR=<폴더>   BLUEPRINT_TRACE=<out>/trace.jsonl   BLUEPRINT_TRACE_TEXT=1 (합성 서류 시연 전용)
//
// 장면 순서: INTRO(도메인 · 3-Way · A0) → SEARCH(자연어 질의) → P2(인과 역추적 · 판단 · 인터록 · 결재) → P1(현장 대기 · 단말 스캔 · 서명 재요청 ·
// 소명 · 3중 진실) → P3 · P3F(Red-line 델타 · 위조 잠금) → P4(모델 이견 · 공동 결재) → P567(잡음 · 동결 · 선행 경보) → PHYS(물리엔진 폐루프) →
// AIDC(도메인 전환 · D2) → D1 → D3 · D3F → D4 → D567 → DPHYS → END(발전소로 되돌림).
// 장면 하나가 실패해도 기록을 남기고 다음 장면으로 갑니다 (편집기는 실패한 장면을 빼고 잇습니다).
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
const EN = flag("--lang") === "en";
const AUDIT = args.includes("--audit");
const OUT = path.resolve(ROOT, flag("--out") || path.join("out", "rec", EN ? "master_en" : "master_ko"));
const ONLY = (flag("--only") || "INTRO,SEARCH,P2,P1,P3,P4,P567,PHYS,AIDC,D1,D3,D4,D567,DPHYS,END").split(",");
const FRAMES = path.join(OUT, "frames");
const SNAPS = path.join(OUT, "snaps");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
/** 화면 글 (한국어 화면, 영어 화면) */
const L = (ko, en) => (EN ? en : ko);

// ───────── 녹화 ─────────

const events = [];
const frames = { c: [], f: [] };
let current = null;

function mark(id, data = {}) {
  events.push({ ...data, t: Date.now(), type: "mark", id, scene: current });
  console.log(`[mark] ${id} ${JSON.stringify(data).slice(0, 160)}`);
}

/** 화면 주인 바꾸기 (편집기가 이 창의 프레임을 크게 씁니다) */
function src(name) {
  events.push({ t: Date.now(), type: "src", src: name, scene: current });
}

function scene(id, source = "c") {
  current = id;
  events.push({ t: Date.now(), type: "scene", id });
  src(source);
}

function save() {
  fs.writeFileSync(path.join(OUT, "timeline.json"), JSON.stringify({ lang: EN ? "en" : "ko", events, frames }, null, 1));
}

/** CDP 화면 캐스트: 그림이 바뀔 때마다 프레임이 옵니다. 멈춘 창(가려진 단말)은 0.8 초마다 직접 찍어 메웁니다. */
class Cast {
  constructor(page, name) {
    this.page = page;
    this.name = name;
    this.dir = path.join(FRAMES, name);
    this.last = 0;
    fs.mkdirSync(this.dir, { recursive: true });
  }

  keep(buf) {
    const t = Date.now();
    const file = `${t}.jpg`;
    fs.writeFileSync(path.join(this.dir, file), buf);
    frames[this.name].push({ t, file });
    this.last = t;
  }

  async start() {
    this.client = await this.page.context().newCDPSession(this.page);
    this.client.on("Page.screencastFrame", (f) => {
      this.keep(Buffer.from(f.data, "base64"));
      this.client.send("Page.screencastFrameAck", { sessionId: f.sessionId }).catch(() => {});
    });
    await this.client.send("Page.startScreencast", { format: "jpeg", quality: 84, maxWidth: 1920, maxHeight: 1080, everyNthFrame: 2 });
    this.timer = setInterval(async () => {
      if (Date.now() - this.last < 800 || this.busy) return;
      this.busy = true;
      try {
        const r = await this.client.send("Page.captureScreenshot", { format: "jpeg", quality: 84 });
        this.keep(Buffer.from(r.data, "base64"));
      } catch {}
      this.busy = false;
    }, 400);
  }

  async stop() {
    clearInterval(this.timer);
    await this.client.send("Page.stopScreencast").catch(() => {});
  }
}

/** 화면에 남길 클릭 자리 (캐스트에는 마우스가 안 보여서 편집기가 물결을 그립니다) */
async function click(page, locator, opts = {}) {
  const box = await locator.boundingBox().catch(() => null);
  if (box) events.push({ t: Date.now(), type: "click", page: page === pages.c ? "c" : "f", x: box.x + box.width / 2, y: box.y + box.height / 2 });
  await locator.click(opts);
}

async function hold(ms) {
  await sleep(ms);
}

// ───────── 앱 다루기 (console.js 와 같은 선택자) ─────────

const pages = { c: null, f: null };

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

async function findPages(browser) {
  for (let i = 0; i < 150; i++) {
    let c = null;
    let f = null;
    for (const p of browser.contexts().flatMap((x) => x.pages())) {
      const text = await p.evaluate(() => document.body.innerText).catch(() => "");
      if ((text.includes("관제 콘솔") && text.includes("공간 Depth")) || (text.includes("Control Console") && text.includes("Spatial Depth"))) c = p;
      if (text.includes("Field Patrol Kiosk")) f = p;
    }
    if (c && f) return { c, f };
    await sleep(2000);
  }
  throw new Error("두 창을 찾지 못했습니다");
}

async function invoke(cmd, payload = {}) {
  return await pages.c.evaluate(([cmd, payload]) => window.__TAURI_INTERNALS__.invoke(cmd, payload), [cmd, payload]);
}

/** 백엔드 값을 그대로 남깁니다 (영상의 숫자 · 글은 여기와 trace 에서 옵니다) */
async function snap(name, cmd, payload = {}) {
  try {
    const v = await invoke(cmd, payload);
    fs.writeFileSync(path.join(SNAPS, `${name}.json`), JSON.stringify(v, null, 1));
    events.push({ t: Date.now(), type: "snap", name, scene: current });
    return v;
  } catch (e) {
    console.log(`[snap] ${name} 실패: ${e}`);
    return null;
  }
}

async function incidentId() {
  return await pages.c.evaluate(() => document.querySelector("main.console")?.dataset.incident || "");
}

async function waitText(page, text, timeout = 240000) {
  await page.waitForFunction((t) => document.body.innerText.includes(t), text, { timeout });
}

async function setLanguage(lang) {
  const c = pages.c;
  const now = await c.evaluate(() => document.documentElement.lang);
  if (now !== lang) {
    await c.locator(".settings-pill").click();
    await c.locator(`.settings-popover [data-lang="${lang}"]`).click();
    await c.locator(".settings-popover .ov-close").click();
  }
  for (const p of [pages.c, pages.f]) await p.waitForFunction((l) => document.documentElement.lang === l, lang, { timeout: 30000 });
  await sleep(1500);
}

async function openQueue(page) {
  if (await page.locator("#incident-popover").isHidden()) await click(page, page.locator(".queue-pill"));
  await page.locator("#incident-popover").waitFor({ state: "visible", timeout: 10000 });
}

async function closeQueue(page) {
  if (await page.locator("#incident-popover").isVisible()) await page.locator("#incident-popover .ov-close").click();
}

async function inject(page, id) {
  await openQueue(page);
  await hold(600);
  await page.selectOption("select.sim", id);
}

async function openIncident(page, scenario) {
  const end = Date.now() + 300000;
  while (Date.now() < end) {
    if ((await page.locator(`main.console[data-scenario="${scenario}"]`).count()) > 0) break;
    await openQueue(page).catch(() => {});
    const item = page.locator(".queue-item", { hasText: `${scenario} · ` }).first();
    if (await item.isVisible().catch(() => false)) await item.click({ timeout: 3000 }).catch(() => {});
    await sleep(1500);
  }
  if ((await page.locator(`main.console[data-scenario="${scenario}"]`).count()) === 0) throw new Error(`${scenario} 사건을 열지 못했습니다`);
  await closeQueue(page);
  await sleep(1200);
}

async function clickQueueItem(page, text) {
  await openQueue(page);
  const item = page.locator(".queue-item", { hasText: text }).first();
  await item.waitFor({ timeout: 300000 });
  await click(page, item);
  await closeQueue(page);
  await sleep(1500);
}

async function stage(page, n) {
  await click(page, page.locator(".stage-card").nth(n - 1));
  await sleep(900);
}

async function closeDrawer(page) {
  const b = page.locator(".drawer header button", { hasText: L("닫기", "Close") });
  if (await b.count()) await b.first().click();
  await sleep(300);
}

async function tab(page, n) {
  await click(page, page.locator(".tab").nth(n));
  await sleep(600);
}

function placeScan(file) {
  if (!SCAN_DIR) throw new Error("BLUEPRINT_SCANNER_DIR 를 주세요");
  fs.mkdirSync(SCAN_DIR, { recursive: true });
  for (const old of fs.readdirSync(SCAN_DIR)) fs.rmSync(path.join(SCAN_DIR, old), { force: true });
  fs.copyFileSync(path.join(ROOT, "out", "field", file), path.join(SCAN_DIR, file));
}

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

async function tidy(c) {
  if (await c.locator(".cz-card").count()) await c.locator(".cz-card .ov-close").click();
  await closeDrawer(c);
  await closeQueue(c);
  if (await c.locator(".settings-popover").isVisible()) await c.locator(".settings-popover .ov-close").click();
  if (await c.locator(".search-results").count()) await c.locator(".search-results .ov-close").click();
  if (await c.locator(".telemetry-dropup").count()) await c.locator(".telemetry-dropup .ov-close").click();
  await sleep(300);
}

async function deselect(c) {
  await tidy(c);
  await c.evaluate(() => document.activeElement && document.activeElement.blur && document.activeElement.blur());
  for (let i = 0; i < 4 && (await c.locator(".cctv").count()) > 0; i++) {
    await c.keyboard.press("Escape");
    await sleep(300);
  }
}

/** 한 줄 요약 · 첫 결과 · "왜" 를 읽어 둡니다 (자막은 이 값에서 만듭니다) */
async function query(c, q, id, opts = {}) {
  const t0 = Date.now();
  const box = c.locator(".search-input");
  await click(c, box);
  await box.fill("");
  await c.keyboard.type(q, { delay: 55 });
  await hold(400);
  await box.press("Enter");
  await c.waitForSelector(".search-results", { timeout: 20000 }).catch(() => {});
  await sleep(1800);
  const top = await c.locator(".search-hit").first().innerText().catch(() => "");
  const toast = await c.locator(".cmd-toast").last().innerText().catch(() => "");
  const conds = await c.locator(".search-results .cond").allInnerTexts().catch(() => []);
  mark(id, { q, t0, top: top.slice(0, 400), toast: toast.slice(0, 300), conds: conds.slice(0, 8) });
  await hold(opts.hold ?? 3200);
  return top;
}

// ───────── 장면 ─────────

async function intro(c, f) {
  scene("INTRO");
  await deselect(c);
  await tab(c, 0);
  mark("intro.console");
  await hold(3500);
  await tab(c, 1);
  await c.waitForSelector(".mermaid-host svg", { timeout: 60000 });
  mark("intro.mermaid");
  await hold(3000);
  await tab(c, 2);
  mark("intro.mindmap");
  await hold(3500);
  await tab(c, 0);
  await click(c, c.locator(".chip", { hasText: L("A0 종합", "A0 composite") }));
  await c.waitForSelector(".a0-sheet img", { timeout: 30000 });
  await sleep(1200);
  mark("intro.a0");
  await hold(2500);
  await click(c, c.locator(".a0-tile", { hasText: "B4" }).first(), { position: { x: 30, y: 30 } });
  mark("intro.a0tile");
  await hold(3000);
  await click(c, c.locator("button", { hasText: L("전체 보기", "Show all") }));
  await click(c, c.locator(".chip", { hasText: "PID-103-LAB" }));
  await hold(800);
  src("f");
  mark("intro.kiosk");
  await hold(3000);
  src("c");
  // 도메인 전환 맛보기: 같은 엔진이 데이터센터 원장 · 도면으로 바뀝니다
  const t0 = Date.now();
  await switchDomain(c, f, "aidc");
  mark("intro.aidc", { ms: Date.now() - t0 });
  await tab(c, 0);
  await hold(3000);
  await tab(c, 1);
  await c.waitForSelector(".mermaid-host svg", { timeout: 60000 }).catch(() => {});
  mark("intro.aidc_mermaid");
  await hold(2500);
  await tab(c, 2);
  mark("intro.aidc_mindmap");
  await hold(2500);
  await tab(c, 0);
  const t1 = Date.now();
  await switchDomain(c, f, "plant");
  mark("intro.plant", { ms: Date.now() - t1 });
  await hold(1500);
}

async function switchDomain(c, f, to) {
  await tidy(c);
  const label = to === "aidc" ? L("AI 데이터센터", "AI Data Center") : L("화력발전소", "Thermal Power Plant");
  const brand = to === "aidc" ? L("AI 데이터센터 자율 관제 콘솔", "AI Data Center Autonomous Control Console") : L("1.2GW 발전소 관제 콘솔", "1.2GW Power Plant Control Console");
  await click(c, c.locator(".domain-switch .seg", { hasText: label }));
  await waitText(c, brand, 120000);
  await waitText(f, to === "aidc" ? L("AI 데이터센터 현장 엔지니어 단말", "AI Data Center Field Engineer Terminal") : L("1.2GW 발전소 현장 순시", "1.2GW Power Plant Field Patrol"), 60000);
  await sleep(1500);
}

async function searchStatic(c) {
  scene("SEARCH");
  await deselect(c);
  await tab(c, 0);
  const qs = L(
    [
      ["103동 2층 밸브", "search.floor"],
      ["105동 밸브 유인통풍기", "search.demote"],
      ["밸브 아닌 103동 설비", "search.negate"],
      ["10LABI0AA102", "search.typo"],
      ["승인 대기 MOC", "search.moc"],
    ],
    [
      ["valves on floor 2 of building 103", "search.floor"],
      ["building 105 valve ID fan", "search.demote"],
      ["building 103 equipment not valves", "search.negate"],
      ["10LABI0AA102", "search.typo"],
      ["pending MOC", "search.moc"],
    ],
  );
  for (const [q, id] of qs) {
    await query(c, q, id);
    if (id === "search.floor") {
      // 1위를 누르면 도면 · Mermaid · 마인드맵이 함께 그 설비로 갑니다
      await click(c, c.locator(".search-hit").first());
      await hold(2200);
      mark("search.sync_drawing");
      await tab(c, 1);
      await c.waitForSelector(".mermaid-host svg", { timeout: 60000 }).catch(() => {});
      await hold(2200);
      mark("search.sync_mermaid");
      await tab(c, 2);
      await hold(2500);
      mark("search.sync_mindmap");
      await tab(c, 0);
    }
  }
  await deselect(c);
}

async function p2(c) {
  scene("P2");
  await deselect(c);
  await inject(c, "P2");
  mark("p2.inject");
  await openIncident(c, "P2");
  const id = await incidentId();
  await tab(c, 0);
  await c.waitForSelector(".sheet img", { timeout: 30000 });
  await sleep(1500);
  const cards = await c.locator(".cctv").evaluateAll((els) => els.filter((e) => e.style.visibility !== "hidden" && e.offsetWidth > 0).map((e) => e.dataset.card));
  const lags = await c.locator(".lag-label").allInnerTexts().catch(() => []);
  mark("p2.open", { incident: id, cards, lags });
  await snap("P2-detail", "incident_detail", { id });
  await hold(3500);
  // 타임머신: 발원점에서 재생 (상류 밸브 → 펌프 → 과열기 순서로 켜짐)
  await click(c, c.locator(".scrubber button", { hasText: L("발원점", "Origin") }));
  await hold(1200);
  const play = c.locator(".scrubber button", { hasText: L("재생", "Play") }).first();
  if (await play.count()) await click(c, play);
  mark("p2.replay");
  await hold(9000);
  await click(c, c.locator(".scrubber button", { hasText: L("분석 시각", "Analysis time") }));
  await hold(1200);
  // 허브 Drop-up: 시계열 · SDS 로직 차트
  await click(c, c.locator(".charts-toggle"));
  await c.locator(".telemetry-dropup").waitFor({ timeout: 10000 }).catch(() => {});
  mark("p2.charts");
  await hold(4000);
  await c.locator(".telemetry-dropup .ov-close").click().catch(() => {});
  // 프로세스 마이닝: 흐름 · 병목 겹치기 + 대기열 레인
  await c.locator(".mining-toggle input").check();
  mark("p2.mining");
  await hold(3500);
  await openQueue(c);
  mark("p2.lanes");
  await hold(3000);
  await closeQueue(c);
  await c.locator(".mining-toggle input").uncheck();
  await tab(c, 1);
  await c.waitForSelector(".mermaid-host svg", { timeout: 60000 }).catch(() => {});
  await hold(1200);
  mark("p2.mermaid");
  await hold(3000);
  await tab(c, 2);
  mark("p2.mindmap");
  await hold(3500);
  await tab(c, 0);
  // Stage 3 (원장 대조) · Stage 4 (Zoro-1 판단 · SDS 축)
  await stage(c, 3);
  mark("p2.stage3");
  await hold(3500);
  await closeDrawer(c);
  await stage(c, 4);
  mark("p2.stage4");
  await hold(7000);
  await c.locator(".drawer").evaluate((e) => e.querySelector(".drawer-body, section, div")?.scrollBy?.(0, 400)).catch(() => {});
  await hold(2500);
  await closeDrawer(c);
  // 고장 중 실시간 질의 (시나리오 계측값으로 상태 조건)
  const live = L(
    [
      ["103동 압력 높은 급수펌프", "p2.q_pressure"],
      ["고착된 밸브", "p2.q_stuck"],
      ["10LABI0AA102 고착", "p2.q_typo"],
      ["유량 1,400 t/h 미만", "p2.q_flow"],
    ],
    [
      ["feedwater pump in building 103 with high pressure", "p2.q_pressure"],
      ["stuck valve", "p2.q_stuck"],
      ["10LABI0AA102 stuck", "p2.q_typo"],
      ["flow below 1,400 t/h", "p2.q_flow"],
    ],
  );
  for (const [q, mid] of live) await query(c, q, mid, { hold: 2800 });
  await tidy(c);
  // Stage 5: 가드레일 → 인터록 → 결재 → 보관 · 봉인
  await stage(c, 5);
  mark("p2.stage5");
  await hold(3000);
  await click(c, c.locator(".drawer button", { hasText: L("공동 결재 없이 확정 시도", "without co-sign") }));
  await waitText(c, L("차단", "blocked"), 60000).catch(() => {});
  mark("p2.g3_rubber", { text: await replyText(c) });
  await hold(3500);
  await click(c, c.locator(".drawer button", { hasText: L("반려", "Reject") }).first());
  await sleep(2500);
  mark("p2.reject", { text: await replyText(c) });
  await hold(3000);
  await c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input").check();
  await click(c, c.locator(".drawer button", { hasText: L("선행 절차를 건너뛰고 확정 시도", "without prerequisite steps") }));
  await waitText(c, "Step 2", 60000);
  mark("p2.g3_order", { text: await replyText(c) });
  await hold(3500);
  const step2 = c.locator(".drawer .il-step", { hasText: "Step 2" });
  await click(c, step2.locator("input"));
  await c.keyboard.type("65", { delay: 120 });
  await click(c, step2.locator("button", { hasText: L("현장 값 확인", "Check field value") }));
  await waitText(c, L("남은 선행 절차: Step 3", "Remaining prerequisite steps: Step 3"), 60000).catch(() => {});
  mark("p2.step2");
  await hold(2500);
  await click(c, c.locator(".drawer .il-step", { hasText: "Step 3" }).locator("button", { hasText: L("무전 확인", "radio confirmation") }));
  await waitText(c, L("모두 확인", "prerequisite steps confirmed"), 60000).catch(() => {});
  mark("p2.step3");
  await hold(2500);
  const cosign = c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input");
  if (!(await cosign.isChecked())) await cosign.check();
  await snap("P2-detail-final", "incident_detail", { id });
  await click(c, c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }));
  await waitText(c, L("확정했습니다", "Confirmed ("), 120000);
  await waitText(c, L("보관 문서", "Archived Record"), 30000);
  mark("p2.archive", { text: (await c.locator(".drawer").innerText().catch(() => "")).slice(0, 1200) });
  await hold(5000);
  await closeDrawer(c);
}

/** 서랍의 결재 응답 줄 (차단 · 안내). 앞에서 읽은 줄과 달라질 때까지 잠깐 기다립니다. */
let lastNote = null;
async function replyText(c) {
  let t = "";
  for (let i = 0; i < 20; i++) {
    t = (await c.locator(".drawer p.note").allInnerTexts().catch(() => [])).join(" | ");
    if (t && t !== lastNote) break;
    await sleep(250);
  }
  lastNote = t;
  return t.slice(0, 500);
}

async function kioskScan(f, file, id) {
  placeScan(file);
  await fieldReset(f);
  await click(f, f.locator("button", { hasText: L("도면/계기 스캔", "Scan Drawing/Gauge") }));
  mark(`${id}.scan_start`);
  await f.waitForSelector(".form-grid", { timeout: 300000 });
  await sleep(800);
  const crops = await f.locator(".crop").count();
  const badges = await f.locator(".field-card .badge, .field-card .tag").allInnerTexts().catch(() => []);
  mark(`${id}.scan_done`, { crops, badges: badges.slice(0, 10) });
  await hold(3500);
}

async function p1(c, f) {
  scene("P1");
  await deselect(c);
  await inject(c, "P1");
  await openIncident(c, "P1");
  const id = await incidentId();
  await waitText(c, L("현장 대기", "Awaiting Field Check"), 120000);
  mark("p1.awaiting", { incident: id });
  await hold(2500);
  // 현장 확인 전 확정 → 규칙이 막고 재점검을 요구합니다 (G2_FIELD_RECHECK_REQUIRED)
  await stage(c, 5);
  await click(c, c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }));
  await sleep(3000);
  mark("p1.g2_block", { text: await replyText(c) });
  await hold(3500);
  const note = c.locator(".drawer label.field", { hasText: L("현장 재점검 요청 메모", "Field recheck request note") }).locator("input");
  if (await note.count()) {
    await click(c, note);
    await c.keyboard.type(L("102동 주증기 압력 게이지 실측 바랍니다", "Please read the 102 main steam pressure gauge"), { delay: 30 });
  }
  await click(c, c.locator(".drawer button", { hasText: L("현장 재점검요구", "Request field recheck") }));
  await sleep(2500);
  mark("p1.recheck", { text: await replyText(c) });
  await hold(2500);
  await closeDrawer(c);
  // 현장 단말: 계기 점검표 스캔 → 원터치 대조 카드 → 장갑 스테퍼 → 서명 재요청 → 소명(자유 서술) → 전송
  src("f");
  mark("p1.kiosk");
  await hold(2000);
  await kioskScan(f, "P1_gauge.png", "p1");
  const reading = f.locator(".field-card", { hasText: L("현장 실측치", "Field Reading") });
  if (await reading.count()) {
    await click(f, reading.locator("button.step", { hasText: "+0.1" }));
    await hold(700);
    await click(f, reading.locator("button.step", { hasText: "-0.1" }));
    mark("p1.stepper");
    await hold(1500);
  }
  // 서명 패드에 한 획 직선을 긋습니다 (서명 규칙: 획 2회 이상 · 면적 15% 이상 · 직선 아님). 서명은 소명까지 마친 마지막 전송에서 심사합니다.
  const paperBox = () => f.locator("label.check", { hasText: L("스캔한 문서의 서명", "signature/seal on the scanned document") }).locator("input");
  let badSign = false;
  if ((await paperBox().count()) && (await paperBox().isChecked())) {
    await paperBox().uncheck();
    await sleep(600);
    const pad = f.locator(".signature-field canvas");
    // 패드가 단말 화면 아래쪽에 있으면 마우스가 닿지 않습니다
    await pad.scrollIntoViewIfNeeded();
    await sleep(400);
    const b = await pad.boundingBox();
    if (b) {
      await f.mouse.move(b.x + 30, b.y + b.height / 2);
      await f.mouse.down();
      await f.mouse.move(b.x + b.width * 0.6, b.y + b.height / 2 + 2, { steps: 12 });
      await f.mouse.up();
      badSign = true;
    }
    mark("p1.sign_line");
    await hold(1500);
  }
  const justify = async (first) => {
    await click(f, f.locator("button", { hasText: L("주제어실로 전송하기", "Send to Main Control Room") }));
    await waitText(f, L("편차가 있어", "deviation between"), 120000);
    if (first) {
      mark("p1.excuse");
      await hold(3000);
    }
    await click(f, f.locator("input[value=free_text]"));
    const box = f.locator("textarea");
    if (!(await box.inputValue().catch(() => ""))) {
      await click(f, box);
      await f.keyboard.type(L("현장 아날로그 게이지는 26.0 MPa 로 정상인데 전송기 A 채널만 낮게 나옵니다. 영점이 흔들리는 것 같습니다", "The local analog gauge reads a normal 26.0 MPa, but only transmitter channel A reads low. Its zero seems to be drifting"), { delay: 18 });
    }
    if (first) {
      mark("p1.free_text");
      await hold(1500);
    }
    await click(f, f.locator("button", { hasText: L("소명 포함 전송하기", "Send with Justification") }));
    // 접수되거나(접수 번호) 서명을 다시 받으라는 안내와 함께 점검표로 돌아옵니다 (G1_SIGN_INVALID_RETRY)
    await f.waitForFunction((r) => document.body.innerText.includes(r) || !!document.querySelector(".kiosk-card .error"), L("접수 번호", "Receipt No."), { timeout: 180000 });
  };
  await justify(true);
  if (badSign && (await f.locator(".kiosk-card .error").count())) {
    mark("p1.g1_retry", { text: (await f.locator(".kiosk-card .error").first().innerText().catch(() => "")).slice(0, 300) });
    await hold(4000);
    // 스캔한 점검표의 서명을 씁니다
    if (await paperBox().count()) await paperBox().check();
    await hold(1000);
    await justify(false);
  }
  await waitText(f, L("접수 번호", "Receipt No."), 180000);
  mark("p1.sent");
  await hold(3000);
  src("c");
  await openIncident(c, "P1");
  await stage(c, 3);
  mark("p1.truth");
  await hold(4500);
  const unmask = c.locator(".drawer button", { hasText: L("작업자 실명 열람 요청", "Request worker names") });
  if (await unmask.count()) {
    await click(c, unmask);
    await sleep(2000);
    mark("p1.unmask", { text: (await c.locator(".drawer").innerText().catch(() => "")).match(/[^\n]*(열람|unmask|Unmask|denied|거부|names)[^\n]*/g)?.slice(0, 4) ?? [] });
    await hold(3500);
  }
  await snap("P1-detail", "incident_detail", { id: await incidentId() });
  await closeDrawer(c);
  await stage(c, 5);
  mark("p1.stage5");
  await hold(2500);
  await click(c, c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }));
  await waitText(c, L("확정했습니다", "Confirmed ("), 120000);
  await waitText(c, L("보관 문서", "Archived Record"), 30000).catch(() => {});
  mark("p1.archive");
  await hold(3000);
  await closeDrawer(c);
  src("f");
  await waitText(f, L("처리가 끝났습니다", "Processing complete"), 60000).catch(() => {});
  mark("p1.kiosk_done");
  await hold(3000);
  src("c");
}

async function p3(c, f, forged) {
  const sid = forged ? "P3F" : "P3";
  const key = forged ? "p3f" : "p3";
  scene(sid);
  await deselect(c);
  await inject(c, sid);
  src("f");
  mark(`${key}.kiosk`);
  await kioskScan(f, forged ? "P3F_redline.png" : "P3_redline.png", key);
  const moc = await f.locator(".field-card", { hasText: L("MOC 변경번호", "MOC No.") }).locator("input").inputValue().catch(() => "");
  mark(`${key}.form`, { moc });
  await hold(2500);
  await f.locator("label.check", { hasText: "PTW" }).locator("input").check();
  await click(f, f.locator("button", { hasText: L("주제어실로 전송하기", "Send to Main Control Room") }));
  await waitText(f, L("접수 번호", "Receipt No."), 180000);
  mark(`${key}.sent`);
  await hold(2500);
  src("c");
  await clickQueueItem(c, L("현장 보고", "Field report"));
  const id = await incidentId();
  await snap(`${sid}-detail`, "incident_detail", { id });
  await stage(c, 1);
  mark(`${key}.stage12`, { incident: id });
  await hold(6000);
  await closeDrawer(c);
  await stage(c, 3);
  mark(`${key}.stage3`);
  await hold(4500);
  await closeDrawer(c);
  if (!forged) {
    await tab(c, 0);
    await hold(1500);
    mark("p3.overlay");
    await hold(3000);
    await stage(c, 5);
    await click(c, c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }));
    await waitText(c, L("확정했습니다", "Confirmed ("), 120000);
    mark("p3.archive");
    await hold(2500);
    await closeDrawer(c);
    src("f");
    await waitText(f, L("반영되었습니다", "was applied"), 60000).catch(() => {});
    mark("p3.kiosk_done");
    await hold(3000);
    src("c");
  } else {
    mark("p3f.locked", { text: (await c.locator(".approval-hud").innerText().catch(() => "")).slice(0, 600) });
    await hold(3500);
  }
}

async function p4(c) {
  scene("P4");
  await deselect(c);
  await inject(c, "P4");
  await openIncident(c, "P4");
  const id = await incidentId();
  await snap("P4-detail", "incident_detail", { id });
  await stage(c, 4);
  mark("p4.stage4", { incident: id });
  await hold(6500);
  await closeDrawer(c);
  await stage(c, 5);
  mark("p4.stage5");
  await hold(2500);
  await click(c, c.locator(".drawer button", { hasText: L("공동 결재 없이 확정 시도", "without co-sign") }));
  await waitText(c, L("차단", "blocked"), 60000).catch(() => {});
  mark("p4.g3_rubber", { text: await replyText(c) });
  await hold(3500);
  // 오경보 예외 승인은 현장 증빙 스캔이 1장 이상 있어야 합니다
  const fp = c.locator(".drawer button", { hasText: L("고위험 예외승인 (오경보)", "High-risk exception approval (false alarm)") });
  if (await fp.count()) {
    await click(c, fp);
    await sleep(2500);
    mark("p4.false_positive", { text: await replyText(c) });
    await hold(3000);
  }
  await c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input").check();
  mark("p4.cosign");
  await hold(1500);
  await click(c, c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }));
  await waitText(c, L("확정했습니다", "Confirmed ("), 120000);
  await waitText(c, L("보관 문서", "Archived Record"), 30000).catch(() => {});
  mark("p4.archive", { text: (await c.locator(".drawer").innerText().catch(() => "")).slice(0, 1200) });
  await hold(4500);
  await closeDrawer(c);
}

/** 잡음 · 동결 · 선행 경보 요약 컷: 사건이 어떻게 처리됐는지(자동 종료 · 판정)와 Stage 4 를 짧게 */
async function summaryCuts(c, ids, prefix) {
  scene(prefix === "p" ? "P567" : "D567");
  for (const sid of ids) {
    await deselect(c);
    const before = (await invoke("incident_list").catch(() => null)) ?? null;
    await inject(c, sid);
    const t0 = Date.now();
    let opened = false;
    while (Date.now() - t0 < 150000) {
      if ((await c.locator(`main.console[data-scenario="${sid}"]`).count()) > 0) {
        opened = true;
        break;
      }
      const busy = await c.evaluate(() => document.body.innerText.includes("주입 중") || document.body.innerText.includes("Injecting"));
      if (!busy && Date.now() - t0 > 20000) break;
      await sleep(1500);
    }
    await closeQueue(c);
    const list = await invoke("incident_list").catch(() => null);
    fs.writeFileSync(path.join(SNAPS, `${sid}-list.json`), JSON.stringify({ before, after: list }, null, 1));
    if (opened) {
      const id = await incidentId();
      await snap(`${sid}-detail`, "incident_detail", { id });
      await tab(c, 0);
      mark(`${sid.toLowerCase()}.open`, { incident: id });
      await hold(3500);
      await stage(c, 4);
      mark(`${sid.toLowerCase()}.stage4`);
      await hold(4500);
      await closeDrawer(c);
    } else {
      // 사건을 열지 않고 끝난 경우 (잡음으로 걸러 자동 종료 등): 대기열의 처리 완료 줄을 보입니다
      await openQueue(c);
      mark(`${sid.toLowerCase()}.closed`);
      await hold(4000);
      await closeQueue(c);
    }
  }
}

async function phys(c, aidc) {
  scene(aidc ? "DPHYS" : "PHYS");
  const key = aidc ? "dphys" : "phys";
  await deselect(c);
  await click(c, c.locator("button", { hasText: L("물리엔진 · 자율 대응", "Physics Engine") }));
  await click(c, c.locator(".drawer button", { hasText: L("시뮬레이터 시작", "Start simulator") }));
  mark(`${key}.start`);
  await hold(4500);
  await click(c, c.locator(".drawer button", { hasText: L("18% 고착 주입", "stuck at 18%") }));
  mark(`${key}.inject`);
  await waitText(c, L("인과 사슬 잠금", "Causal chain locked"), 90000);
  mark(`${key}.chain`);
  await waitText(c, L("고장 코드 VALVE_STUCK_LOW", "fault code VALVE_STUCK_LOW"), 30000).catch(() => {});
  await waitText(c, L("Tier 1 자율 대응", "Tier 1 Autonomous Response"), 240000);
  mark(`${key}.tier1`, { text: (await c.locator(".physics").innerText().catch(() => "")).slice(0, 1500) });
  await hold(2500);
  const run = c.locator(".drawer button", { hasText: L("완화 조치 실행", "Run mitigation") });
  if (await run.count()) await click(c, run);
  mark(`${key}.run`);
  const hot = aidc ? "DC1GPU10CT01" : "10HAH10CT001";
  await waitText(c, L(`${hot} 완화 조치 → 회복 중`, `${hot} Mitigating → Recovering`), 180000).catch(() => {});
  await hold(6000);
  mark(`${key}.recovering`, { text: (await c.locator(".physics").innerText().catch(() => "")).slice(0, 2500) });
  await hold(3000);
  await closeDrawer(c);
  // 완화 조치가 실제 계측으로 Step 2 를 자동 통과시켰는지 (HUD 인터록)
  mark(`${key}.hud`, { text: (await c.locator(".approval-hud").innerText().catch(() => "")).slice(0, 800) });
  await hold(3500);
}

async function d2(c, f) {
  scene("AIDC");
  await deselect(c);
  const t0 = Date.now();
  await switchDomain(c, f, "aidc");
  mark("aidc.switch", { ms: Date.now() - t0 });
  await hold(2500);
  await inject(c, "D2");
  await openIncident(c, "D2");
  const id = await incidentId();
  await tab(c, 0);
  await sleep(1500);
  const lags = await c.locator(".lag-label").allInnerTexts().catch(() => []);
  mark("d2.open", { incident: id, lags });
  await snap("D2-detail", "incident_detail", { id });
  await hold(3500);
  await click(c, c.locator(".scrubber button", { hasText: L("발원점", "Origin") }));
  await hold(800);
  const play = c.locator(".scrubber button", { hasText: L("재생", "Play") }).first();
  if (await play.count()) await click(c, play);
  mark("d2.replay");
  await hold(8000);
  await click(c, c.locator(".scrubber button", { hasText: L("분석 시각", "Analysis time") }));
  await stage(c, 4);
  mark("d2.stage4");
  await hold(6500);
  await closeDrawer(c);
  for (const [q, mid] of L(
    [
      ["고착된 밸브", "d2.q_stuck"],
      ["온도 높은 GPU 랙", "d2.q_gpu"],
      ["DC1CDUI0AP01", "d2.q_typo"],
      ["승인 대기 MOP", "d2.q_mop"],
    ],
    [
      ["stuck valve", "d2.q_stuck"],
      ["GPU rack with high temperature", "d2.q_gpu"],
      ["DC1CDUI0AP01", "d2.q_typo"],
      ["pending MOP", "d2.q_mop"],
    ],
  ))
    await query(c, q, mid, { hold: 2600 });
  await tidy(c);
  await stage(c, 5);
  await c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input").check();
  await click(c, c.locator(".drawer button", { hasText: L("선행 절차를 건너뛰고 확정 시도", "without prerequisite steps") }));
  await waitText(c, "Step 2", 60000);
  mark("d2.g3_order", { text: await replyText(c) });
  await hold(3000);
  const step2 = c.locator(".drawer .il-step", { hasText: "Step 2" });
  await click(c, step2.locator("input"));
  await c.keyboard.type("65", { delay: 120 });
  await click(c, step2.locator("button", { hasText: L("현장 값 확인", "Check field value") }));
  await waitText(c, L("남은 선행 절차: Step 3", "Remaining prerequisite steps: Step 3"), 60000).catch(() => {});
  await click(c, c.locator(".drawer .il-step", { hasText: "Step 3" }).locator("button", { hasText: L("무전 확인", "radio confirmation") }));
  await waitText(c, L("모두 확인", "prerequisite steps confirmed"), 60000).catch(() => {});
  mark("d2.interlock");
  await hold(2500);
  const cosign = c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input");
  if (!(await cosign.isChecked())) await cosign.check();
  await click(c, c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }));
  await waitText(c, L("확정했습니다", "Confirmed ("), 120000);
  await waitText(c, L("보관 문서", "Archived Record"), 30000).catch(() => {});
  mark("d2.archive", { text: (await c.locator(".drawer").innerText().catch(() => "")).slice(0, 1200) });
  await hold(4500);
  await closeDrawer(c);
}

async function d1(c, f) {
  scene("D1");
  await deselect(c);
  await inject(c, "D1");
  await openIncident(c, "D1");
  await waitText(c, L("현장 대기", "Awaiting Field Check"), 120000);
  mark("d1.awaiting", { incident: await incidentId() });
  await hold(2000);
  src("f");
  await kioskScan(f, "D1_gauge.png", "d1");
  const kks = await f.locator(".field-card", { hasText: L("자산코드", "Asset code") }).locator("input").inputValue().catch(() => "");
  mark("d1.form", { kks });
  await hold(2000);
  await click(f, f.locator("button", { hasText: L("NOC 관제실로 전송하기", "Send to NOC") }));
  await waitText(f, L("편차가 있어", "deviation between"), 120000);
  await click(f, f.locator("input[value=transmitter_drift]"));
  mark("d1.excuse");
  await hold(1500);
  await click(f, f.locator("button", { hasText: L("소명 포함 전송하기", "Send with Justification") }));
  await waitText(f, L("접수 번호", "Receipt No."), 180000);
  mark("d1.sent");
  await hold(2000);
  src("c");
  await openIncident(c, "D1");
  await stage(c, 3);
  mark("d1.truth");
  await hold(4500);
  await snap("D1-detail", "incident_detail", { id: await incidentId() });
  await closeDrawer(c);
  await stage(c, 5);
  await click(c, c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }));
  await waitText(c, L("확정했습니다", "Confirmed ("), 120000);
  mark("d1.archive");
  await hold(2500);
  await closeDrawer(c);
  src("f");
  await waitText(f, L("처리가 끝났습니다", "Processing complete"), 60000).catch(() => {});
  mark("d1.kiosk_done");
  await hold(2500);
  src("c");
}

async function d3(c, f, forged) {
  const sid = forged ? "D3F" : "D3";
  const key = sid.toLowerCase();
  scene(sid);
  await deselect(c);
  await inject(c, sid);
  src("f");
  await kioskScan(f, forged ? "D3F_redline.png" : "D3_redline.png", key);
  const moc = await f.locator(".field-card", { hasText: L("MOP 변경번호", "MOP No.") }).locator("input").inputValue().catch(() => "");
  mark(`${key}.form`, { moc });
  await f.locator("label.check", { hasText: "MOP/Hot-Work" }).locator("input").check();
  await click(f, f.locator("button", { hasText: L("NOC 관제실로 전송하기", "Send to NOC") }));
  await waitText(f, L("접수 번호", "Receipt No."), 180000);
  mark(`${key}.sent`);
  await hold(2000);
  src("c");
  await clickQueueItem(c, L("현장 보고", "Field report"));
  const id = await incidentId();
  await snap(`${sid}-detail`, "incident_detail", { id });
  await stage(c, 1);
  mark(`${key}.stage12`, { incident: id });
  await hold(5500);
  await closeDrawer(c);
  await stage(c, 3);
  mark(`${key}.stage3`);
  await hold(4000);
  await closeDrawer(c);
  if (!forged) {
    await stage(c, 5);
    await click(c, c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }));
    await waitText(c, L("확정했습니다", "Confirmed ("), 120000);
    mark("d3.archive");
    await hold(2000);
    await closeDrawer(c);
    src("f");
    await waitText(f, L("반영되었습니다", "was applied"), 60000).catch(() => {});
    mark("d3.kiosk_done");
    await hold(2500);
    src("c");
  } else {
    mark("d3f.locked", { text: (await c.locator(".approval-hud").innerText().catch(() => "")).slice(0, 600) });
    await hold(3500);
  }
}

async function d4(c) {
  scene("D4");
  await deselect(c);
  await inject(c, "D4");
  await openIncident(c, "D4");
  const id = await incidentId();
  await snap("D4-detail", "incident_detail", { id });
  await stage(c, 4);
  mark("d4.stage4", { incident: id });
  await hold(6000);
  await closeDrawer(c);
  await stage(c, 5);
  await click(c, c.locator(".drawer button", { hasText: L("공동 결재 없이 확정 시도", "without co-sign") }));
  await waitText(c, L("차단", "blocked"), 60000).catch(() => {});
  mark("d4.g3_rubber", { text: await replyText(c) });
  await hold(3000);
  await c.locator(".drawer label.check", { hasText: L("공동 결재", "co-sign") }).locator("input").check();
  await click(c, c.locator(".drawer button", { hasText: L("조치확정", "Confirm Action") }));
  await waitText(c, L("확정했습니다", "Confirmed ("), 120000);
  await waitText(c, L("보관 문서", "Archived Record"), 30000).catch(() => {});
  mark("d4.archive", { text: (await c.locator(".drawer").innerText().catch(() => "")).slice(0, 1200) });
  await hold(4000);
  await closeDrawer(c);
}

async function end(c, f) {
  scene("END");
  await deselect(c);
  const list = await invoke("incident_list").catch(() => null);
  fs.writeFileSync(path.join(SNAPS, "end-aidc-list.json"), JSON.stringify(list, null, 1));
  await openQueue(c);
  mark("end.queue");
  await hold(3500);
  await closeQueue(c);
  await switchDomain(c, f, "plant");
  mark("end.plant");
  await hold(2500);
}

(async () => {
  fs.mkdirSync(SNAPS, { recursive: true });
  const browser = await connect();
  const found = await findPages(browser);
  pages.c = found.c;
  pages.f = found.f;
  const { c, f } = pages;
  await c.waitForFunction(() => /(엔진|Engine): Zoro-1/.test(document.body.innerText), null, { timeout: 900000 });
  if (AUDIT) for (const p of [c, f]) await p.evaluate(() => (window.__i18nAudit = true));
  await setLanguage(EN ? "en" : "ko");
  await snap("verdict_catalog", "verdict_catalog");
  const casts = [new Cast(c, "c"), new Cast(f, "f")];
  for (const k of casts) await k.start();
  events.push({ t: Date.now(), type: "start", lang: EN ? "en" : "ko" });
  const results = [];
  const flows = {
    INTRO: () => intro(c, f),
    SEARCH: () => searchStatic(c),
    P2: () => p2(c),
    P1: () => p1(c, f),
    P3: async () => {
      await p3(c, f, false);
      await p3(c, f, true);
    },
    P4: () => p4(c),
    P567: () => summaryCuts(c, ["P5", "P6", "P7"], "p"),
    PHYS: () => phys(c, false),
    AIDC: () => d2(c, f),
    D1: () => d1(c, f),
    D3: async () => {
      await d3(c, f, false);
      await d3(c, f, true);
    },
    D4: () => d4(c),
    D567: () => summaryCuts(c, ["D5", "D6", "D7"], "d"),
    DPHYS: () => phys(c, true),
    END: () => end(c, f),
  };
  for (const id of ONLY) {
    const t0 = Date.now();
    try {
      await flows[id]();
      results.push({ id, ok: true, s: (Date.now() - t0) / 1000 });
    } catch (e) {
      console.log(`[fail] ${id}: ${e}`);
      events.push({ t: Date.now(), type: "fail", id, error: String(e).slice(0, 400) });
      results.push({ id, ok: false, error: String(e).slice(0, 400) });
      await c.screenshot({ path: path.join(OUT, `${id}-error-console.png`) }).catch(() => {});
      await f.screenshot({ path: path.join(OUT, `${id}-error-field.png`) }).catch(() => {});
      await tidy(c).catch(() => {});
      src("c");
    }
    save();
  }
  events.push({ t: Date.now(), type: "end" });
  for (const k of casts) await k.stop();
  save();
  fs.writeFileSync(path.join(OUT, "results.json"), JSON.stringify(results, null, 1));
  if (AUDIT) {
    const seen = new Set();
    for (const p of [c, f]) for (const l of await p.evaluate(() => (window.__i18nSeen ? window.__i18nSeen() : [])).catch(() => [])) seen.add(l);
    fs.writeFileSync(path.join(OUT, "i18n_seen.json"), JSON.stringify([...seen].sort(), null, 1));
  }
  if (EN) {
    const misses = new Set();
    for (const p of [c, f]) for (const l of await p.evaluate(() => (window.__i18nMisses ? window.__i18nMisses() : [])).catch(() => [])) misses.add(l);
    fs.writeFileSync(path.join(OUT, "i18n_misses.json"), JSON.stringify([...misses].sort(), null, 1));
  }
  console.log(JSON.stringify(results, null, 1));
  await browser.close().catch(() => {});
  process.exit(results.every((r) => r.ok) ? 0 : 1);
})();
