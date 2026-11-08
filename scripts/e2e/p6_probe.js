// 알람 사건 초안을 뒤로 미룬 효과를 잽니다 (UI 단순화 P6). 떠 있는 앱에 붙어서 세션을 비우고,
// 시나리오 하나를 주입한 직후 다른 하나를 주입해 ① 사건이 열리기까지 ② 바로 뒤 주입(다음 판단)이 기다린 시간
// ③ 두 초안이 다 채워지기까지를 잽니다. 앱을 BLUEPRINT_DEFER_DRAFT=0 으로 띄우면 예전처럼 한 번에 쓰는 것과 견줄 수 있습니다.
//
//   node scripts/e2e/run.js --app-only                         (미룸, 기본)
//   node scripts/e2e/p6_probe.js --label defer                 → out/e2e/p6_defer.json
//   node scripts/e2e/run.js --stop
//   BLUEPRINT_DEFER_DRAFT=0 node scripts/e2e/run.js --app-only (한 번에)
//   node scripts/e2e/p6_probe.js --label sync                  → out/e2e/p6_sync.json
//
// 화면 언어 · GPU 상태가 같은 두 실행을 견줍니다. 다른 프로세스가 GPU 를 쓰면 글쓰기가 상한에 걸려 템플릿으로 끝나 비교가 되지 않습니다.
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
const FIRST = flag("--first") || "P2";
const SECOND = flag("--second") || "P4";
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

/** 화면의 번역 감싸기를 거치지 않는 백엔드 명령 (판정 · 초안 글은 보지 않고 시간 · 표시만 봄) */
const invoke = (c, cmd, payload = {}) => c.evaluate(([cmd, payload]) => window.__TAURI_INTERNALS__.invoke(cmd, payload), [cmd, payload]);

async function draftDone(c, id, since, timeout = 180000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const d = await invoke(c, "incident_detail", { id }).catch(() => null);
    if (!d) return { ms: Date.now() - since, closed: true };
    if (!d.drafting) return { ms: Date.now() - since, by_model: !!d.analysis?.draft?.by_model, draft_ms: d.analysis?.draft?.ms ?? null, attempts: d.analysis?.draft?.attempts ?? null };
    await sleep(250);
  }
  throw new Error(`${id} 초안이 ${timeout / 1000} s 안에 채워지지 않았습니다`);
}

(async () => {
  const browser = await chromium.connectOverCDP("http://127.0.0.1:9222");
  const c = await consolePage(browser);
  await invoke(c, "reset_session");
  await sleep(1500);
  const t0 = Date.now();
  const first = await invoke(c, "inject_scenario", { id: FIRST });
  const t1 = Date.now();
  const firstDrafting = !!(await invoke(c, "incident_detail", { id: first })).drafting;
  // 첫 초안은 둘째 주입과 함께 지켜봅니다 (둘째 주입이 끝난 뒤에 보면 그만큼 늦게 잼)
  const firstDraft = draftDone(c, first, t0);
  const second = await invoke(c, "inject_scenario", { id: SECOND });
  const t2 = Date.now();
  const [d1, d2] = [await firstDraft, await draftDone(c, second, t0)];
  const out = {
    label: LABEL,
    deferred: firstDrafting,
    first: { scenario: FIRST, id: first, visible_ms: t1 - t0, draft: d1 },
    second: { scenario: SECOND, id: second, visible_ms: t2 - t0, waited_ms: t2 - t1, draft: d2 },
    both_drafts_ms: Math.max(d1.ms, d2.ms),
  };
  const file = path.join(ROOT, "out", "e2e", `p6_${LABEL}.json`);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(out, null, 1));
  console.log(JSON.stringify(out, null, 1));
  await invoke(c, "reset_session");
  await browser.close();
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
