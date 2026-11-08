// 화면이 백엔드 명령을 얼마나 자주 부르는지 잽니다 (window.__ipcCalls, app/src/ipc.ts).
// 떠 있는 앱에 붙어서 ① 가만히 둘 때 ② 사건을 고른 채 둘 때 ③ 타임머신 재생 중, 명령별 호출 수를 셉니다.
//
//   node scripts/e2e/run.js --app-only
//   node scripts/e2e/ipc_probe.js [--label before] [--secs 60]     → out/e2e/ipc_<label>.json
//   node scripts/e2e/run.js --stop
const { chromium } = require("playwright-core");
const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "../..");
const args = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : null;
};
const LABEL = flag("--label") || "now";
const SECS = Number(flag("--secs") || 60);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function consolePage(browser) {
  for (let i = 0; i < 150; i++) {
    for (const p of browser.contexts().flatMap((c) => c.pages())) {
      if (await p.evaluate(() => !!document.querySelector("main.console")).catch(() => false)) return p;
    }
    await sleep(2000);
  }
  throw new Error("콘솔 창을 찾지 못했습니다");
}

const snapshot = (c) => c.evaluate(() => ({ ...(window.__ipcCalls || {}) }));
function diff(a, b) {
  const out = {};
  for (const k of new Set([...Object.keys(a), ...Object.keys(b)])) {
    const d = (b[k] || 0) - (a[k] || 0);
    if (d) out[k] = d;
  }
  return Object.fromEntries(Object.entries(out).sort((x, y) => y[1] - x[1]));
}

async function measure(c, secs) {
  const a = await snapshot(c);
  await sleep(secs * 1000);
  return diff(a, await snapshot(c));
}

(async () => {
  const browser = await chromium.connectOverCDP("http://127.0.0.1:9222");
  const c = await consolePage(browser);
  // 모델로 띄웠으면 Zoro-1, 규칙만(run.js --rules)이면 그 문구가 상태 줄에 나올 때까지
  await c.waitForFunction(() => /(엔진|Engine): (Zoro-1|규칙만|Rules)/.test(document.body.innerText), null, { timeout: 900000 });
  await sleep(3000);
  const result = { label: LABEL, secs: SECS, at: new Date().toISOString() };
  result.idle = await measure(c, SECS);
  console.log("[ipc] 가만히", JSON.stringify(result.idle));
  // 사건 하나를 넣고 고릅니다 (시연 판이 있으면 그 안의 주입 칸, 없으면 예전처럼 사건 대기열 안)
  const demo = c.locator(".demo-pill");
  if (await demo.count()) {
    if (await c.locator("#demo-popover").isHidden()) await demo.click();
  } else if (await c.locator("#incident-popover").isHidden()) await c.locator(".queue-pill").click();
  await c.selectOption("select.sim", "P2");
  await c.waitForFunction(() => document.querySelector("main.console")?.dataset.scenario === "P2", null, { timeout: 300000 });
  await sleep(5000);
  result.selected = await measure(c, SECS);
  console.log("[ipc] 사건을 고른 채", JSON.stringify(result.selected));
  await c.locator(".scrubber button", { hasText: /발원점|Origin/ }).click();
  await c.locator(".scrubber button", { hasText: /재생|Play/ }).first().click();
  result.replay_secs = 10;
  result.replay = await measure(c, 10);
  console.log("[ipc] 재생 중 10초", JSON.stringify(result.replay));
  fs.mkdirSync(path.join(ROOT, "out", "e2e"), { recursive: true });
  fs.writeFileSync(path.join(ROOT, "out", "e2e", `ipc_${LABEL}.json`), JSON.stringify(result, null, 2));
  await browser.close().catch(() => {});
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
