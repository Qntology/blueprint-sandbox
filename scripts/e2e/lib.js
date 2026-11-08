// console.js · record.js 가 함께 쓰는 앱 다루기: 창 찾기 · 판 여닫기 · 시나리오 주입 · 결재 · 현장 단말.
// 화면 구조가 바뀌면 여기 한 곳만 고칩니다. record.js 는 마우스 자리를 남기는 click 을 넘겨 씁니다.
const { chromium } = require("playwright-core");
const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "../..");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * @param {{ en?: boolean, click?: (page, locator, opts?) => Promise<void>, scanDir?: string }} opts
 */
function make(opts = {}) {
  const en = !!opts.en;
  const click = opts.click || ((page, locator, o) => locator.click(o));
  const scanDir = opts.scanDir ?? process.env.BLUEPRINT_SCANNER_DIR;
  /** 화면 글 (한국어 화면, 영어 화면) */
  const L = (ko, e) => (en ? e : ko);

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

  /** 두 창을 화면 글이 아니라 뿌리 요소로 찾습니다 (main.console · main.kiosk). 글은 도메인 · 언어 · 선택 상태마다 바뀝니다. */
  async function findPages(browser) {
    for (let i = 0; i < 150; i++) {
      let c = null;
      let f = null;
      for (const p of browser.contexts().flatMap((x) => x.pages())) {
        const root = await p.evaluate(() => (document.querySelector("main.console") ? "c" : document.querySelector("main.kiosk") ? "f" : "")).catch(() => "");
        if (root === "c") c = p;
        if (root === "f") f = p;
      }
      if (c && f) return { c, f };
      await sleep(2000);
    }
    throw new Error("두 창을 찾지 못했습니다");
  }

  async function waitText(page, text, timeout = 240000) {
    await page.waitForFunction((t) => document.body.innerText.includes(t), text, { timeout });
  }

  /** 상태 줄이 모델을 다 연 뒤의 문구로 바뀔 때까지 (화면의 단계 이름에도 "Zoro-1" 이 있어 상태 줄만 봅니다) */
  async function waitEngine(c, timeout = 900000) {
    await c.waitForFunction(() => /(엔진|Engine): Zoro-1/.test(document.querySelector(".status-bar")?.innerText ?? ""), null, { timeout });
  }

  // ── 떠 있는 판: 좌상단 사건 대기열 · 우상단 시연 · 설정 팝오버 (한 번에 하나만 열림) ──
  const POPOVER = { queue: ["#incident-popover", ".queue-pill"], demo: ["#demo-popover", ".demo-pill"], settings: [".settings-popover", ".settings-pill"] };

  async function openPopover(page, which) {
    const [panel, pill] = POPOVER[which];
    if (await page.locator(panel).isHidden()) await click(page, page.locator(pill));
    await page.locator(panel).waitFor({ state: "visible", timeout: 10000 });
  }

  async function closePopover(page, which) {
    const [panel] = POPOVER[which];
    if (await page.locator(panel).isVisible()) await page.locator(`${panel} .ov-close`).first().click();
  }

  const openQueue = (page) => openPopover(page, "queue");
  const closeQueue = (page) => closePopover(page, "queue");

  /** 시나리오 주입은 시연 판(🧪 시연) 안에 있습니다 (설정의 "시연 · 시험 도구" 가 켜져 있어야 보임) */
  async function inject(page, id) {
    await openPopover(page, "demo");
    await sleep(300);
    await page.selectOption("#demo-popover select.sim", id);
  }

  /** 주입이 끝나면 콘솔이 새 사건을 스스로 고릅니다. 그 전에 목록에 뜨면 대기열에서 누릅니다 (어느 쪽이 먼저든 같은 사건). */
  async function openIncident(page, scenario, timeout = 300000) {
    const end = Date.now() + timeout;
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

  /** 단계 카드 (Stage 1 · 2 는 한 칸) */
  async function stage(page, n) {
    await click(page, page.locator(`.stage-card[data-stage="${n === 2 ? 1 : n}"]`));
    await sleep(800);
  }

  async function closeDrawer(page) {
    const b = page.locator(".drawer header button", { hasText: L("닫기", "Close") });
    if (await b.count()) await b.first().click();
    await sleep(300);
  }

  /** 확정 뒤 허브 알림의 [보관 문서 열기] (서랍이 저절로 열리지 않습니다) */
  async function openArchive(page) {
    const b = page.locator(".cmd-toast button.toast-action", { hasText: L("보관 문서", "Archived Record") });
    await b.waitFor({ timeout: 30000 });
    await click(page, b);
    await page.locator(".drawer").waitFor({ timeout: 30000 });
    await waitText(page, L("보관 문서", "Archived Record"), 30000);
  }

  // ── 결재 HUD (오른쪽): 인터록 게이트 · 공동 결재 · 조치확정 · 더 보기 · 시험 단추 ──
  const qa = (page) => page.locator(".approval-hud .quick-approval");

  async function cosign(page) {
    const box = qa(page).locator("label.cosign input");
    await box.waitFor({ timeout: 30000 });
    if (!(await box.isChecked())) await box.check();
  }

  async function confirm(page) {
    await click(page, qa(page).locator("button.confirm"));
  }

  async function more(page) {
    if ((await qa(page).locator(".qa-more").count()) === 0) await click(page, qa(page).locator("button.qa-more-toggle"));
    await qa(page).locator(".qa-more").waitFor({ timeout: 5000 });
  }

  /** 시연 모드의 가드레일 시험 단추: cosign(공동 결재 없이) · order(선행 절차 건너뛰기) · field(현장 확인 전) */
  async function tryGuard(page, which) {
    await click(page, qa(page).locator(`button.qa-test-${which}`));
  }

  async function interlockValue(page, step, value) {
    const row = page.locator(".approval-hud .il-step", { hasText: `Step ${step}` });
    await row.locator("input").fill(String(value));
    await click(page, row.locator("button", { hasText: L("현장 값 확인", "Check field value") }));
  }

  async function interlockRadio(page, step) {
    await click(page, page.locator(".approval-hud .il-step", { hasText: `Step ${step}` }).locator("button", { hasText: L("무전 확인", "radio confirmation") }));
  }

  /** 결재 HUD 응답 줄 (차단 · 안내) */
  async function reply(page) {
    return (await qa(page).locator("p.qa-reply").allInnerTexts().catch(() => [])).join(" | ");
  }

  // ── 현장 단말 ──
  /** 스캐너 폴더를 비우고 서류 한 장만 넣습니다 (앞 흐름이 읽지 못한 서류가 섞이지 않게) */
  function placeScan(file) {
    if (!scanDir) throw new Error("BLUEPRINT_SCANNER_DIR 를 주세요");
    fs.mkdirSync(scanDir, { recursive: true });
    for (const old of fs.readdirSync(scanDir)) fs.rmSync(path.join(scanDir, old), { force: true });
    fs.copyFileSync(path.join(ROOT, "out", "field", file), path.join(scanDir, file));
  }

  /** 앞 흐름이 어느 화면에서 멈췄든 첫 화면으로 되돌립니다 */
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

  // ── 정리 ──
  /** 앞 흐름이 남긴 확대 창 · 서랍 · 팝오버 · Drop-up 을 닫습니다 */
  async function tidy(c) {
    if (await c.locator(".cz-card").count()) await c.locator(".cz-card .ov-close").click();
    await closeDrawer(c);
    for (const which of ["queue", "demo", "settings"]) await closePopover(c, which);
    if (await c.locator(".search-results").count()) await c.locator(".search-results .ov-close").click();
    if (await c.locator(".telemetry-dropup").count()) await c.locator(".telemetry-dropup .ov-close").click();
    await sleep(300);
  }

  /** 판이 모두 닫힌 뒤 ESC 를 누르면 검색 표시 → 설비 선택이 풀립니다 (말풍선이 타일 · 상자를 가리지 않게) */
  async function deselect(c) {
    await tidy(c);
    await c.evaluate(() => document.activeElement && document.activeElement.blur && document.activeElement.blur());
    for (let i = 0; i < 4 && (await c.locator(".cctv").count()) > 0; i++) {
      await c.keyboard.press("Escape");
      await sleep(300);
    }
  }

  /** 화면 언어는 설정 팝오버에서 고릅니다. 두 창이 그 언어로 바뀔 때까지 기다립니다. */
  async function setLanguage(c, f, lang) {
    const now = await c.evaluate(() => document.documentElement.lang);
    if (now !== lang) {
      await openPopover(c, "settings");
      await c.locator(`.settings-popover [data-lang="${lang}"]`).click();
      await closePopover(c, "settings");
    }
    for (const p of [c, f]) await p.waitForFunction((l) => document.documentElement.lang === l, lang, { timeout: 30000 });
    await sleep(1500);
  }

  /** 도메인은 설정 팝오버에서 바꿉니다 (사건 · 감사를 비우는 드문 조작이라 머리에서 내렸습니다) */
  async function switchDomain(c, f, to, timeout = 120000) {
    await tidy(c);
    const label = to === "aidc" ? L("AI 데이터센터", "AI Data Center") : L("화력발전소", "Thermal Power Plant");
    const brand = to === "aidc" ? L("AI 데이터센터 자율 관제 콘솔", "AI Data Center Autonomous Control Console") : L("1.2GW 발전소 관제 콘솔", "1.2GW Power Plant Control Console");
    const kiosk = to === "aidc" ? L("AI 데이터센터 현장 엔지니어 단말", "AI Data Center Field Engineer Terminal") : L("1.2GW 발전소 현장 순시", "1.2GW Power Plant Field Patrol");
    await openPopover(c, "settings");
    await click(c, c.locator(".settings-popover .domain-row .seg", { hasText: label }));
    await waitText(c, brand, timeout);
    if (f) await waitText(f, kiosk, timeout);
    await closePopover(c, "settings");
    await sleep(1500);
  }

  return {
    L,
    sleep,
    connect,
    findPages,
    waitText,
    waitEngine,
    openPopover,
    closePopover,
    openQueue,
    closeQueue,
    inject,
    openIncident,
    clickQueueItem,
    stage,
    closeDrawer,
    openArchive,
    qa,
    cosign,
    confirm,
    more,
    tryGuard,
    interlockValue,
    interlockRadio,
    reply,
    placeScan,
    fieldReset,
    tidy,
    deselect,
    setLanguage,
    switchDomain,
  };
}

module.exports = { make, ROOT, sleep };
