import { invoke } from "./ipc";
import { listen } from "./ipc";
import { useCallback, useEffect, useState } from "react";
import { useT } from "./i18n";
import { tr } from "./i18n/lang";

interface Job {
  running: boolean;
  percent: number;
  message: string;
  error: string | null;
}

interface ModelRow {
  name: string;
  label: string;
  license: string;
  repo: string | null;
  total_bytes: number;
  bytes_on_disk: number;
  installed: boolean;
  importable: boolean;
  job: Job;
}

interface ModelReport {
  dir: string;
  import_from: string | null;
  models: ModelRow[];
}

interface Alert {
  action: string;
  model?: string;
  message: string;
}

const size = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`);

/** 모델마다 가져올 곳: 판단 · 인식 · 크롭은 zoro-gravity, 검색 임베딩은 logis-center, Stanza 는 stanza-onnx-export 변환본 */
const SOURCE: Record<string, [string, string]> = {
  embed: ["logis-center", "logis-center"],
  stanza: ["stanza-onnx-export 변환본 (BLUEPRINT_STANZA_FROM)", "stanza-onnx-export output (BLUEPRINT_STANZA_FROM)"],
};
const source = (m: ModelRow) => tr(...(SOURCE[m.name] ?? ["zoro-gravity", "zoro-gravity"]));

function note(m: ModelRow): string {
  if (m.job.running) return m.job.message;
  if (m.installed) return tr(`${size(m.bytes_on_disk)} 사용 중 (${source(m)} 와 같은 모델)`, `${size(m.bytes_on_disk)} in use (same model as ${source(m)})`);
  if (m.importable)
    return tr(
      `${source(m)} 에 있습니다. 가져오면 큰 가중치는 하드 링크로 걸려 디스크를 더 쓰지 않습니다.`,
      `Found in ${source(m)}. Importing hard-links the large weights, so it uses no extra disk space.`,
    );
  if (m.bytes_on_disk > 0) return tr(`받다 멈춤 (${size(m.bytes_on_disk)}). 다시 누르면 이어 받습니다.`, `Download stopped (${size(m.bytes_on_disk)}). Press again to resume.`);
  if (m.name === "stanza")
    return tr(
      "공개 저장소가 없습니다. stanza-onnx-export 로 en · ko 를 변환한 뒤 `blueprint-models import stanza <onnx_final>` 로 가져오세요. 없어도 검색은 규칙 줄기로 돕니다.",
      "No public repository. Convert en · ko with stanza-onnx-export, then run `blueprint-models import stanza <onnx_final>`. Search still works with rule stems without it.",
    );
  return m.repo ? `huggingface.co/${m.repo}` : tr("내려받을 곳이 없습니다. 직접 변환해서 모델 폴더에 넣으세요.", "No download source. Convert it yourself and put it in the model folder.");
}

/** 모델 가져오기 · 다운로드 · 삭제 설정. 판단 · 인식 · 크롭 모델은 zoro-gravity 와, 검색 임베딩은 logis-center 와 같아서 먼저 그 폴더에서 가져오고, 없을 때만 내려받습니다. */
export function ModelSettings({ open, onOpen }: { open: boolean; onOpen: () => void }) {
  const t = useT();
  const [report, setReport] = useState<ModelReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);

  const refresh = useCallback(() => invoke<ModelReport>("check_model_status").then(setReport).catch((e) => setError(String(e))), []);

  const run = useCallback(
    (command: string, args?: Record<string, string>) => {
      setError(null);
      setConfirming(null);
      invoke(command, args).catch((e) => setError(String(e))).finally(refresh);
    },
    [refresh],
  );

  useEffect(() => {
    const stops = [
      listen<Alert>("app_error_alert", (e) => {
        if (e.payload.action !== "open_settings") return setError(e.payload.message);
        onOpen();
        if (e.payload.model) run("download_model", { modelName: e.payload.model });
      }),
      listen<{ model: string; percent: number; message: string }>("download_progress", (e) => {
        const { model, percent, message } = e.payload;
        setReport((r) => r && { ...r, models: r.models.map((m) => (m.name === model ? { ...m, job: { ...m.job, running: true, percent, message } } : m)) });
      }),
      listen("download_complete", refresh),
      listen("download_error", refresh),
    ];
    // 듣기 시작한 뒤에 물어야 알림을 놓치지 않습니다.
    Promise.all(stops).then(() => invoke("ensure_models")).catch((e) => setError(String(e))).finally(refresh);
    return () => void stops.forEach((s) => s.then((f) => f()));
  }, [onOpen, refresh, run]);

  if (!open) return null;
  const models = report?.models ?? [];
  const missing = models.filter((m) => m.repo && !m.installed && !m.importable && !m.job.running);
  const importable = models.filter((m) => m.importable);
  const busy = models.some((m) => m.job.running);

  return (
    <aside className="settings">
      <h2>{t("모델 관리", "Model Management")}</h2>
      <p className="muted path">
        {t("모델 폴더:", "Model folder:")} {report?.dir}
      </p>
      <p className="muted path">
        {t("가져올 곳 (zoro-gravity):", "Import source (zoro-gravity):")} {report?.import_from ?? t("없음", "none")}
      </p>
      <div className="settings-actions">
        <button className="green" disabled={importable.length === 0} onClick={() => importable.forEach((m) => run("import_model", { modelName: m.name }))}>
          {t(`가져오기 (${importable.length})`, `Import (${importable.length})`)}
        </button>
        <button className="teal" disabled={missing.length === 0} onClick={() => missing.forEach((m) => run("download_model", { modelName: m.name }))}>
          {t("없는 모델 모두 받기", "Download all missing models")}
        </button>
        <button
          className="danger"
          disabled={busy || !models.some((m) => m.bytes_on_disk > 0)}
          onClick={() => (confirming === "all" ? run("delete_all_models") : setConfirming("all"))}
        >
          {confirming === "all" ? t("한 번 더 누르면 모두 삭제", "Press again to delete all") : t("모델 모두 삭제", "Delete all models")}
        </button>
      </div>
      {error && <p className="error">{error}</p>}
      {models.map((m) => (
        <article className="model" key={m.name}>
          <div className="model-top">
            <strong>
              {m.label} / {m.license}
            </strong>
            <span className="buttons">
              <button
                className={m.repo && !m.installed && !m.job.running ? "green" : "grey"}
                disabled={!m.repo || m.installed || m.job.running}
                onClick={() => run("download_model", { modelName: m.name })}
              >
                {m.job.running
                  ? t(`받는 중 (${m.job.percent}%)`, `Downloading (${m.job.percent}%)`)
                  : m.installed
                    ? t("받음", "Downloaded")
                    : m.repo
                      ? t(`다운로드 (${size(m.total_bytes)})`, `Download (${size(m.total_bytes)})`)
                      : t("링크 없음", "No link")}
              </button>
              {m.bytes_on_disk > 0 && !m.job.running && (
                <button className="danger" onClick={() => (confirming === m.name ? run("delete_model", { modelName: m.name }) : setConfirming(m.name))}>
                  {confirming === m.name ? t("한 번 더 누르면 삭제", "Press again to delete") : t("삭제", "Delete")}
                </button>
              )}
            </span>
          </div>
          {m.job.running && (
            <div className="bar" role="progressbar" aria-valuenow={m.job.percent}>
              <div style={{ width: `${m.job.percent}%` }} />
            </div>
          )}
          <p className={m.job.error ? "error" : "muted"}>{m.job.error ? t(`오류: ${m.job.error}`, `Error: ${m.job.error}`) : note(m)}</p>
        </article>
      ))}
    </aside>
  );
}
