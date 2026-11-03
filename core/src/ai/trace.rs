//! 모델 사용 기록. `BLUEPRINT_TRACE` 에 파일 경로를 주면 모델이 언제 무엇을 했는지 한 줄씩 남깁니다 (JSON Lines).
//! 시연 영상과 시간 분석에 씁니다. 설정하지 않으면 파일에는 아무것도 쓰지 않습니다.
//!
//! 주제어실 콘솔의 단계 상세(가중치 상주 타임라인)를 위해 메모리에도 모을 수 있습니다 ([`capture`]).
//! GPU 작업은 한 번에 하나라서, 모으는 동안 들어온 기록은 그 작업의 것입니다.
//!
//! 기본 기록에는 근무자 정보가 없습니다. 판단의 사실은 식별 정보를 뺀 값이고 (`decision.rs`), 문서와 글쓰기는 좌표 · 개수 · 시간만 남깁니다.
//! `BLUEPRINT_TRACE_TEXT=1` 이면 읽은 글자와 모델이 쓴 글도 남깁니다. 합성 서류 시연 전용이고, 실제 운영에서는 켜면 안 됩니다.

use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

pub const TRACE_ENV: &str = "BLUEPRINT_TRACE";
pub const TRACE_TEXT_ENV: &str = "BLUEPRINT_TRACE_TEXT";

fn sink() -> Option<&'static Mutex<File>> {
    static SINK: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    SINK.get_or_init(|| File::create(std::env::var_os(TRACE_ENV)?).ok().map(Mutex::new)).as_ref()
}

/// 메모리에 모으는 중인 기록 (None 이면 모으지 않음)
fn buffer() -> &'static Mutex<Option<Vec<Value>>> {
    static BUFFER: Mutex<Option<Vec<Value>>> = Mutex::new(None);
    &BUFFER
}

fn capturing() -> bool {
    buffer().lock().map(|b| b.is_some()).unwrap_or(false)
}

/// 기록을 남기는 중인가 (파일이나 메모리). 기록할 값을 만드는 비용을 아낄 때 씁니다.
pub fn enabled() -> bool {
    sink().is_some() || capturing()
}

/// 읽은 글자와 쓴 글까지 남기는가 (파일 기록에서만)
pub fn with_text() -> bool {
    sink().is_some() && std::env::var(TRACE_TEXT_ENV).is_ok_and(|v| v == "1")
}

fn entry(at_ms: u128, model: &str, step: &str, ms: f64, detail: &Value) -> Value {
    json!({ "t": at_ms as u64, "model": model, "step": step, "ms": (ms * 10.0).round() / 10.0, "detail": detail })
}

fn line(at_ms: u128, model: &str, step: &str, ms: f64, detail: &Value) -> String {
    entry(at_ms, model, step, ms, detail).to_string()
}

/// 끝난 일 하나를 남깁니다. `t` 는 끝난 시각이고, `ms` 는 걸린 시간입니다.
pub fn record(model: &str, step: &str, ms: f64, detail: Value) {
    let at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    if let Ok(mut b) = buffer().lock() {
        if let Some(list) = b.as_mut() {
            // 메모리 기록에는 글자를 넣지 않습니다.
            let detail = match &detail {
                Value::Object(map) => Value::Object(map.iter().filter(|(k, _)| !matches!(k.as_str(), "text" | "texts" | "facts")).map(|(k, v)| (k.clone(), v.clone())).collect()),
                other => other.clone(),
            };
            list.push(entry(at, model, step, ms, &detail));
        }
    }
    let Some(sink) = sink() else { return };
    let mut file = sink.lock().unwrap_or_else(|p| p.into_inner());
    // 기록 실패가 진단을 멈추면 안 됩니다.
    let _ = writeln!(file, "{}", line(at, model, step, ms, &detail));
    let _ = file.flush();
}

/// `work` 를 돌리는 동안 남은 기록을 함께 돌려줍니다.
pub fn capture<T>(work: impl FnOnce() -> T) -> (T, Vec<Value>) {
    if let Ok(mut b) = buffer().lock() {
        *b = Some(Vec::new());
    }
    let out = work();
    let records = buffer().lock().ok().and_then(|mut b| b.take()).unwrap_or_default();
    (out, records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_one_json_object_with_the_end_time_and_the_duration() {
        let l = line(1_000, "gocr", "비전", 12.34, &json!({ "inputs": 3 }));
        let v: Value = serde_json::from_str(&l).unwrap();
        assert_eq!(v["t"], 1_000);
        assert_eq!(v["model"], "gocr");
        assert_eq!(v["ms"], 12.3);
        assert_eq!(v["detail"]["inputs"], 3);
        assert!(!l.contains('\n'));
    }

    #[test]
    fn captured_records_come_back_without_text() {
        // 시험 실행에는 환경 변수가 없어서 파일에는 쓰지 않습니다.
        assert!(!with_text());
        let ((), records) = capture(|| {
            assert!(enabled());
            record("zoro", "판단", 630.0, json!({ "task": "근본 원인", "facts": [["a", "b"]] }));
            record("gocr", "결과", 0.0, json!({ "crops": 3, "texts": ["원문"] }));
        });
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["model"], "zoro");
        assert!(records[0]["detail"].get("facts").is_none());
        assert!(records[1]["detail"].get("texts").is_none());
        assert_eq!(records[1]["detail"]["crops"], 3);
        record("zoro", "판단", 1.0, json!({}));
        assert!(!capturing(), "모으기가 끝나면 다시 모으지 않습니다");
    }
}
