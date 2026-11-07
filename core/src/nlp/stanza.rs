//! Stanza 품사 · 원형 실행기. `stanza-onnx-export` 의 format 2 변환본(vocab.json + ONNX)을 읽어 tract 로 돌립니다.
//!
//! 입력 규약은 그 저장소 `verify_onnx.py` 의 `RefRuntime` 과 같습니다 (그 대조가 PyTorch Stanza 와 en · ko 30문장 100 % 일치).
//! - 낱말: `pos.word` 번호 (word_lower 면 소문자), 없으면 UNK(1)
//! - 사전학습: `pretrain_vocab.json` 에서 소문자 · 공백을 `\u{a0}` 로 바꿔 찾음 (25만 낱말, 없으면 UNK)
//! - 문자: 한국어는 낱말마다 글자 번호 (뒤는 0), 영어는 문자 언어모델 글자열 (`'\n' 낱말 ' ' 낱말 ' '…`, 역방향은 낱말 순서 · 글자 모두 뒤집음)
//! - 원형: 빈도 사전(품사별 → `*`) 먼저, 없으면 인코더 + 한 걸음 디코더로 탐욕 디코딩, 편집 분류(그대로 · 소문자)
//!
//! 모델은 ONNX 에 적힌 동적 축 이름(n · c · lf · l …)을 그대로 써서 한 번만 최적화합니다 (길이마다 다시 최적화하지 않음).
//!
//! `stanza` 기능 없이 빌드하면(CI 의 `--no-default-features`) 사전 해석만 남고 `load` 가 거부합니다 — 검색은 규칙 줄기로 돕니다.
#![cfg_attr(not(feature = "stanza"), allow(dead_code, unused_imports))]

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;

/// 품사 · 원형이 붙은 낱말 하나
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tagged {
    pub text: String,
    pub upos: String,
    /// 한국어는 세종 품사를 `+` 로 이은 꼴 (`ncn+jco`), 영어는 PTB (`NNS`). 없으면 `_`
    pub xpos: String,
    pub feats: String,
    /// 한국어는 형태소를 `+` 로 이은 꼴 (`밸브+를`, `높+ㄴ`)
    pub lemma: String,
}

const UNK: i64 = 1;
const EMPTY: i64 = 2;

fn units(v: &Value) -> Result<Vec<String>> {
    v.as_array()
        .ok_or_else(|| anyhow!("목록이 아닙니다"))?
        .iter()
        .map(|s| s.as_str().map(str::to_string).ok_or_else(|| anyhow!("문자열이 아닌 항목")))
        .collect()
}

fn index(list: &[String]) -> HashMap<String, i64> {
    list.iter().enumerate().map(|(i, s)| (s.clone(), i as i64)).collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// 태그 열 하나 (xpos · feats). 구성형은 stanza CompositeVocab 의 id2unit 과 같게 잇습니다.
#[derive(Debug, Clone)]
enum TagSpec {
    Plain(Vec<String>),
    Composite { sep: String, keyed: bool, keys: Vec<String>, units: Vec<Vec<String>> },
}

impl TagSpec {
    fn parse(v: &Value) -> Result<Self> {
        if let Some(obj) = v.as_object() {
            let parts = obj.get("parts").and_then(Value::as_array).ok_or_else(|| anyhow!("parts 가 없습니다"))?;
            let mut keys = Vec::new();
            let mut list = Vec::new();
            for p in parts {
                keys.push(p[0].as_str().unwrap_or_default().to_string());
                list.push(units(&p[1])?);
            }
            Ok(TagSpec::Composite {
                sep: obj.get("sep").and_then(Value::as_str).unwrap_or("").to_string(),
                keyed: obj.get("keyed").and_then(Value::as_bool).unwrap_or(false),
                keys,
                units: list,
            })
        } else {
            Ok(TagSpec::Plain(units(v)?))
        }
    }

    fn heads(&self) -> usize {
        match self {
            TagSpec::Plain(_) => 1,
            TagSpec::Composite { keys, .. } => keys.len(),
        }
    }

    fn unit(&self, ids: &[usize]) -> String {
        match self {
            TagSpec::Plain(u) => u.get(ids[0]).cloned().unwrap_or_else(|| "_".into()),
            TagSpec::Composite { sep, keyed, keys, units } => {
                let items: Vec<String> = ids
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| **v as i64 != EMPTY)
                    .map(|(i, v)| {
                        let u = units[i].get(*v).cloned().unwrap_or_default();
                        if *keyed {
                            format!("{}={u}", keys[i])
                        } else {
                            u
                        }
                    })
                    .collect();
                let joined = items.join(sep);
                if joined.is_empty() {
                    "_".into()
                } else {
                    joined
                }
            }
        }
    }
}

#[cfg(feature = "stanza")]
mod engine {
    use std::sync::Arc;

    use tract_onnx::prelude::*;

    use super::*;

    pub type Plan = Arc<TypedRunnableModel>;

    /// ONNX 를 읽어 최적화합니다. 입력 이름이 사전에 적힌 순서와 같은지 확인합니다.
    pub fn load(path: &Path, inputs: &[String]) -> Result<Plan> {
        let model = tract_onnx::onnx().model_for_path(path).map_err(|e| anyhow!("{e:?}")).with_context(|| format!("ONNX 를 읽을 수 없습니다: {}", path.display()))?;
        let names: Vec<String> = model.input_outlets().map_err(|e| anyhow!("{e:?}"))?.iter().map(|o| model.node(o.node).name.clone()).collect();
        if names != inputs {
            bail!("{} 의 입력이 사전과 다릅니다: ONNX {names:?} · 사전 {inputs:?}", path.display());
        }
        let plan = model.into_optimized().map_err(|e| anyhow!("{e:?}"))?.into_runnable().map_err(|e| anyhow!("{e:?}"))?;
        Ok(plan)
    }

    pub fn i64_2d(rows: usize, cols: usize, data: Vec<i64>) -> Result<Tensor> {
        Ok(tract_ndarray::Array2::from_shape_vec((rows, cols), data)?.into())
    }

    pub fn i64_1d(data: Vec<i64>) -> Tensor {
        tract_ndarray::Array1::from_vec(data).into()
    }

    pub fn run(plan: &Plan, inputs: Vec<Tensor>) -> Result<Vec<Tensor>> {
        let out = plan.run(inputs.into_iter().map(|t| t.into()).collect()).map_err(|e| anyhow!("{e:?}"))?;
        Ok(out.into_iter().map(|v| v.into_tensor()).collect())
    }

    /// (…, k) 로짓의 마지막 축에서 처음 나온 최댓값 자리 (torch.max · numpy.argmax 와 같음)
    pub fn argmax_rows(t: &Tensor) -> Result<Vec<usize>> {
        let view = t.to_plain_array_view::<f32>().map_err(|e| anyhow!("{e:?}"))?;
        let k = *view.shape().last().ok_or_else(|| anyhow!("빈 모양"))?;
        let flat: Vec<f32> = view.iter().copied().collect();
        Ok(flat
            .chunks(k)
            .map(|row| {
                let mut best = 0;
                for (i, v) in row.iter().enumerate() {
                    if *v > row[best] {
                        best = i;
                    }
                }
                best
            })
            .collect())
    }
}

#[cfg(feature = "stanza")]
struct CharLm {
    forward: HashMap<String, i64>,
    backward: HashMap<String, i64>,
    start: String,
    end: String,
}

#[cfg(feature = "stanza")]
struct PosModel {
    plan: engine::Plan,
    inputs: Vec<String>,
    outputs: Vec<String>,
    word: HashMap<String, i64>,
    word_lower: bool,
    chars: HashMap<String, i64>,
    charlm: Option<CharLm>,
    upos: Vec<String>,
    /// tag_order[1..] 순서 (xpos · feats)
    tags: Vec<(String, TagSpec)>,
}

#[cfg(feature = "stanza")]
struct LemmaModel {
    encoder: Option<(engine::Plan, engine::Plan)>,
    chars: HashMap<String, i64>,
    units: Vec<String>,
    pos: HashMap<String, i64>,
    vocab_size: i64,
    oov_slots: usize,
    sos: i64,
    eos: i64,
    max_dec_len: usize,
    edit: bool,
    caseless: bool,
    dict_only: bool,
    use_identity: bool,
    dict: HashMap<String, HashMap<String, String>>,
}

/// 한 언어의 Stanza (품사 + 원형)
pub struct StanzaLang {
    pub lang: String,
    #[cfg(feature = "stanza")]
    pretrain: Option<HashMap<String, i64>>,
    #[cfg(feature = "stanza")]
    pos: PosModel,
    #[cfg(feature = "stanza")]
    lemma: Option<LemmaModel>,
}

impl std::fmt::Debug for StanzaLang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StanzaLang").field("lang", &self.lang).finish_non_exhaustive()
    }
}

impl StanzaLang {
    /// `<models>/stanza/<lang>` 폴더. vocab.json 이 format 2 가 아니면(이전 변환본) 거부합니다 — 이전 판은 품사가 무작위에 가깝습니다.
    #[cfg(feature = "stanza")]
    pub fn load(dir: &Path) -> Result<Self> {
        let read = |name: &str| -> Result<Value> {
            let p = dir.join(name);
            let text = std::fs::read_to_string(&p).with_context(|| format!("읽을 수 없습니다: {}", p.display()))?;
            serde_json::from_str(&text).with_context(|| format!("JSON 이 아닙니다: {}", p.display()))
        };
        let v = read("vocab.json")?;
        if v.get("format").and_then(Value::as_i64) != Some(2) {
            bail!("{} 는 이전 변환본입니다 (format 2 아님). stanza-onnx-export 로 다시 변환하세요", dir.display());
        }
        let lang = v.get("lang").and_then(Value::as_str).unwrap_or_default().to_string();
        let pretrain = match v.get("pretrain").and_then(|p| p.get("file")).and_then(Value::as_str) {
            Some(file) => Some(index(&units(&read(file)?)?)),
            None => None,
        };
        let p = v.get("pos").ok_or_else(|| anyhow!("vocab.json 에 pos 가 없습니다"))?;
        let inputs = strings(&p["inputs"]);
        let charlm = match p.get("charlm") {
            Some(lm) if !lm.is_null() => Some(CharLm {
                forward: index(&units(&lm["forward"])?),
                backward: index(&units(&lm["backward"])?),
                start: lm["start"].as_str().unwrap_or("\n").to_string(),
                end: lm["end"].as_str().unwrap_or(" ").to_string(),
            }),
            _ => None,
        };
        let order = strings(&p["tag_order"]);
        let mut tags = Vec::new();
        for name in order.iter().skip(1) {
            tags.push((name.clone(), TagSpec::parse(&p["tags"][name])?));
        }
        let pos = PosModel {
            plan: engine::load(&dir.join("pos.onnx"), &inputs)?,
            outputs: strings(&p["outputs"]),
            inputs,
            word: index(&units(&p["word"])?),
            word_lower: p["word_lower"].as_bool().unwrap_or(true),
            chars: index(&units(&p["char"])?),
            charlm,
            upos: units(&p["upos"])?,
            tags,
        };
        let lemma = match v.get("lemma") {
            Some(l) => {
                let dict: HashMap<String, HashMap<String, String>> = match l.get("dict_file").and_then(Value::as_str) {
                    Some(f) if dir.join(f).exists() => serde_json::from_value(read(f)?).context("lemma_dict.json")?,
                    _ => HashMap::new(),
                };
                let enc = l.get("encoder").map(|e| strings(&e["inputs"]));
                let dec = l.get("decoder").map(|e| strings(&e["inputs"]));
                let encoder = match (enc, dec) {
                    (Some(e), Some(d)) => Some((engine::load(&dir.join("lemma_encoder.onnx"), &e)?, engine::load(&dir.join("lemma_decoder.onnx"), &d)?)),
                    _ => None,
                };
                let units_list = units(&l["char"])?;
                Some(LemmaModel {
                    encoder,
                    chars: index(&units_list),
                    units: units_list,
                    pos: index(&units(&l["pos"])?),
                    vocab_size: l["vocab_size"].as_i64().unwrap_or(0),
                    oov_slots: l["oov_slots"].as_u64().unwrap_or(0) as usize,
                    sos: l["sos"].as_i64().unwrap_or(2),
                    eos: l["eos"].as_i64().unwrap_or(3),
                    max_dec_len: l["max_dec_len"].as_u64().unwrap_or(50) as usize,
                    edit: l["edit"].as_array().is_some_and(|a| !a.is_empty()),
                    caseless: l["caseless"].as_bool().unwrap_or(false),
                    dict_only: l["dict_only"].as_bool().unwrap_or(false),
                    use_identity: l["use_identity"].as_bool().unwrap_or(false),
                    dict,
                })
            }
            None => None,
        };
        Ok(Self { lang, pretrain, pos, lemma })
    }

    #[cfg(not(feature = "stanza"))]
    pub fn load(dir: &Path) -> Result<Self> {
        bail!("이 빌드는 stanza 기능 없이 만들어졌습니다 ({})", dir.display())
    }

    /// 공백으로 나눈 낱말들 (한 문장)에 품사 · 원형을 붙입니다.
    #[cfg(feature = "stanza")]
    pub fn tag(&self, words: &[&str]) -> Result<Vec<Tagged>> {
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let (upos, cols) = self.pos_tags(words)?;
        let mut out = Vec::with_capacity(words.len());
        for (i, w) in words.iter().enumerate() {
            let xpos = cols.iter().find(|(n, _)| n == "xpos").map(|(_, c)| c[i].clone()).unwrap_or_else(|| "_".into());
            let feats = cols.iter().find(|(n, _)| n == "feats").map(|(_, c)| c[i].clone()).unwrap_or_else(|| "_".into());
            let lemma = match &self.lemma {
                Some(l) => self.lemmatize(l, w, &upos[i])?,
                None => w.to_string(),
            };
            out.push(Tagged { text: w.to_string(), upos: upos[i].clone(), xpos, feats, lemma });
        }
        Ok(out)
    }

    #[cfg(not(feature = "stanza"))]
    pub fn tag(&self, _words: &[&str]) -> Result<Vec<Tagged>> {
        bail!("stanza 기능 없음")
    }

    #[cfg(feature = "stanza")]
    fn pos_tags(&self, words: &[&str]) -> Result<(Vec<String>, Vec<(String, Vec<String>)>)> {
        use engine::*;
        let m = &self.pos;
        let n = words.len();
        let lookup = |map: &HashMap<String, i64>, key: &str| *map.get(key).unwrap_or(&super::stanza::UNK);
        let mut feeds = Vec::new();
        for name in &m.inputs {
            let t = match name.as_str() {
                "word_ids" => i64_2d(1, n, words.iter().map(|w| lookup(&m.word, &if m.word_lower { w.to_lowercase() } else { w.to_string() })).collect())?,
                "pretrain_ids" => {
                    let pre = self.pretrain.as_ref().ok_or_else(|| anyhow!("pretrain_vocab.json 이 없습니다"))?;
                    i64_2d(1, n, words.iter().map(|w| lookup(pre, &w.to_lowercase().replace(' ', "\u{a0}"))).collect())?
                }
                "char_ids" => {
                    let width = words.iter().map(|w| w.chars().count()).max().unwrap_or(1).max(1);
                    let mut data = vec![0i64; n * width];
                    for (i, w) in words.iter().enumerate() {
                        for (j, c) in w.chars().enumerate() {
                            data[i * width + j] = lookup(&m.chars, &c.to_string());
                        }
                    }
                    i64_2d(n, width, data)?
                }
                "char_lens" => i64_1d(words.iter().map(|w| w.chars().count() as i64).collect()),
                "charlm_f_ids" | "charlm_f_offsets" | "charlm_b_ids" | "charlm_b_offsets" => {
                    let lm = m.charlm.as_ref().ok_or_else(|| anyhow!("문자 언어모델 사전이 없습니다"))?;
                    let forward = name.contains("_f_");
                    let (ids, offsets) = charlm_feed(words, if forward { &lm.forward } else { &lm.backward }, &lm.start, &lm.end, forward);
                    if name.ends_with("ids") {
                        i64_2d(1, ids.len(), ids)?
                    } else {
                        i64_1d(offsets)
                    }
                }
                other => bail!("모르는 POS 입력: {other}"),
            };
            feeds.push(t);
        }
        let outs = run(&m.plan, feeds)?;
        let by_name: HashMap<&str, &tract_onnx::prelude::Tensor> = m.outputs.iter().map(String::as_str).zip(outs.iter()).collect();
        let upos_ids = argmax_rows(by_name.get("upos").ok_or_else(|| anyhow!("upos 출력이 없습니다"))?)?;
        let upos: Vec<String> = upos_ids.iter().map(|i| m.upos.get(*i).cloned().unwrap_or_else(|| "X".into())).collect();
        let mut cols = Vec::new();
        for (name, spec) in &m.tags {
            let per_head: Vec<Vec<usize>> = if spec.heads() == 1 && matches!(spec, TagSpec::Plain(_)) {
                vec![argmax_rows(by_name.get(name.as_str()).ok_or_else(|| anyhow!("{name} 출력이 없습니다"))?)?]
            } else {
                (0..spec.heads()).map(|k| argmax_rows(by_name.get(format!("{name}_{k}").as_str()).ok_or_else(|| anyhow!("{name}_{k} 출력이 없습니다"))?)).collect::<Result<_>>()?
            };
            let col: Vec<String> = (0..n).map(|i| spec.unit(&per_head.iter().map(|h| h[i]).collect::<Vec<_>>())).collect();
            cols.push((name.clone(), col));
        }
        Ok((upos, cols))
    }

    /// stanza LemmaProcessor (ensemble_dict): 사전 → 탐욕 디코딩 → 편집 → 빈 원형은 낱말 그대로
    #[cfg(feature = "stanza")]
    fn lemmatize(&self, l: &LemmaModel, word: &str, upos: &str) -> Result<String> {
        if l.use_identity {
            return Ok(word.to_string());
        }
        let key = if l.caseless { word.to_lowercase() } else { word.to_string() };
        let hit = l.dict.get(upos).and_then(|d| d.get(&key)).or_else(|| l.dict.get("*").and_then(|d| d.get(&key))).cloned();
        let lemma = match hit {
            Some(h) => h,
            None if l.dict_only => word.to_string(),
            None => self.seq2seq(l, &key, word, upos)?,
        };
        Ok(if lemma.is_empty() { "_".into() } else { lemma })
    }

    #[cfg(feature = "stanza")]
    fn seq2seq(&self, l: &LemmaModel, key: &str, word: &str, upos: &str) -> Result<String> {
        use engine::*;
        let Some((encoder, decoder)) = &l.encoder else { return Ok(word.to_string()) };
        // 학습 사전에 없는 글자는 V 부터 번호를 매겨 복사 칸으로 (stanza DeltaVocab: 정렬한 순서)
        let mut delta: Vec<char> = key.chars().filter(|c| !l.chars.contains_key(&c.to_string())).collect();
        delta.sort_unstable();
        delta.dedup();
        delta.truncate(l.oov_slots);
        let code = |c: char| -> i64 {
            l.chars.get(&c.to_string()).copied().or_else(|| delta.iter().position(|d| *d == c).map(|p| l.vocab_size + p as i64)).unwrap_or(super::stanza::UNK)
        };
        let mut src = vec![l.sos];
        src.extend(key.chars().map(code));
        src.push(l.eos);
        let len = src.len();
        let pos_id = *l.pos.get(upos).unwrap_or(&super::stanza::UNK);
        let enc = run(encoder, vec![i64_2d(1, len, src.clone())?, i64_1d(vec![pos_id])])?;
        let (h_in, mut hn, mut cn, edit) = (enc[0].clone(), enc[1].clone(), enc[2].clone(), enc[3].clone());
        let mut token = l.sos;
        let mut out: Vec<i64> = Vec::new();
        for _ in 0..l.max_dec_len {
            let step = run(decoder, vec![i64_2d(1, 1, vec![token])?, hn, cn, h_in.clone(), i64_2d(1, len, src.clone())?])?;
            let t = argmax_rows(&step[0])?[0] as i64;
            hn = step[1].clone();
            cn = step[2].clone();
            if t == l.eos {
                break;
            }
            out.push(t);
            token = t;
        }
        let mut pred = String::new();
        for t in out {
            if t < l.vocab_size {
                pred.push_str(&l.units[t as usize]);
            } else {
                match delta.get((t - l.vocab_size) as usize) {
                    Some(c) => pred.push(*c),
                    None => pred.push_str("<UNK>"),
                }
            }
        }
        if l.edit {
            match argmax_rows(&edit)?[0] {
                1 => pred = word.to_string(),
                2 => pred = word.to_lowercase(),
                _ => {}
            }
        }
        if pred.is_empty() || pred.contains("<UNK>") {
            pred = word.to_string();
        }
        Ok(pred)
    }
}

/// build_char_representation 과 같은 글자열 · 낱말 끝 자리
#[cfg(feature = "stanza")]
fn charlm_feed(words: &[&str], vocab: &HashMap<String, i64>, start: &str, end: &str, forward: bool) -> (Vec<i64>, Vec<i64>) {
    let seq: Vec<String> = if forward { words.iter().map(|w| w.to_string()).collect() } else { words.iter().rev().map(|w| w.chars().rev().collect()).collect() };
    let id = |s: &str| *vocab.get(s).unwrap_or(&UNK);
    let mut ids = vec![id(start)];
    let mut offsets = Vec::new();
    for w in &seq {
        for c in w.chars() {
            ids.push(id(&c.to_string()));
        }
        ids.push(id(end));
        offsets.push(ids.len() as i64 - 1);
    }
    if !forward {
        offsets.reverse();
    }
    (ids, offsets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_tags_join_like_stanza() {
        let spec = TagSpec::Composite { sep: "+".into(), keyed: false, keys: vec!["0".into(), "1".into()], units: vec![vec!["<PAD>".into(), "<UNK>".into(), "<EMPTY>".into(), "<ROOT>".into(), "ncn".into()], vec!["<PAD>".into(), "<UNK>".into(), "<EMPTY>".into(), "<ROOT>".into(), "jco".into()]] };
        assert_eq!(spec.unit(&[4, 4]), "ncn+jco");
        assert_eq!(spec.unit(&[4, 2]), "ncn");
        assert_eq!(spec.unit(&[2, 2]), "_");
        let feats = TagSpec::Composite { sep: "|".into(), keyed: true, keys: vec!["Number".into(), "Tense".into()], units: vec![vec!["<PAD>".into(), "<UNK>".into(), "<EMPTY>".into(), "<ROOT>".into(), "Plur".into()], vec!["<PAD>".into(), "<UNK>".into(), "<EMPTY>".into(), "<ROOT>".into(), "Past".into()]] };
        assert_eq!(feats.unit(&[4, 2]), "Number=Plur");
        assert_eq!(feats.unit(&[4, 4]), "Number=Plur|Tense=Past");
    }

    #[cfg(feature = "stanza")]
    #[test]
    fn charlm_sequences_match_build_char_representation() {
        let vocab: HashMap<String, i64> = ["<PAD>", "<UNK>", "<EMPTY>", "<ROOT>", "\n", " ", "a", "b", "c"].iter().enumerate().map(|(i, s)| (s.to_string(), i as i64)).collect();
        // 정방향: '\n' a b ' ' c ' ' → 끝 자리 3, 5
        assert_eq!(charlm_feed(&["ab", "c"], &vocab, "\n", " ", true), (vec![4, 6, 7, 5, 8, 5], vec![3, 5]));
        // 역방향: 낱말 순서와 글자를 뒤집음 '\n' c ' ' b a ' ' → 자리 [2, 5] 를 다시 뒤집어 [5, 2]
        assert_eq!(charlm_feed(&["ab", "c"], &vocab, "\n", " ", false), (vec![4, 8, 5, 7, 6, 5], vec![5, 2]));
    }

    /// 실제 변환본이 있을 때만 (`BLUEPRINT_STANZA_DIR` 또는 모델 폴더의 stanza). 기대값은 PyTorch Stanza 1.15 결과입니다.
    #[cfg(feature = "stanza")]
    #[test]
    #[ignore]
    fn real_models_match_pytorch_stanza() {
        let base = std::env::var_os("BLUEPRINT_STANZA_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| crate::ai::model_store::model_dir("stanza"));
        let ko = StanzaLang::load(&base.join("ko")).expect("ko");
        let got = ko.tag(&["급수펌프의", "토출", "압력이", "높은", "밸브를", "찾아줘"]).unwrap();
        let pick = |f: fn(&Tagged) -> &str| got.iter().map(f).collect::<Vec<_>>();
        assert_eq!(pick(|t| t.upos.as_str()), ["PROPN", "NOUN", "NOUN", "ADJ", "NOUN", "VERB"]);
        assert_eq!(pick(|t| t.lemma.as_str())[..5], ["급수펌프+의", "토출", "압력+이", "높+ㄴ", "밸브+를"]);
        assert_eq!(pick(|t| t.xpos.as_str())[..5], ["nq+jcm", "ncn", "ncn+jcs", "paa+etm", "ncn+jco"]);
        let tagged = ko.tag(&["고착된", "밸브를", "보여줘"]).unwrap();
        assert_eq!(tagged.iter().map(|t| t.lemma.as_str()).collect::<Vec<_>>(), ["고착+되+ㄴ", "밸브+를", "보이+어+주+ㄴ"]);
        let en = StanzaLang::load(&base.join("en")).expect("en");
        let got = en.tag(&["valves", "on", "floor", "2", "of", "building", "103"]).unwrap();
        assert_eq!(got.iter().map(|t| t.upos.as_str()).collect::<Vec<_>>(), ["NOUN", "ADP", "NOUN", "NUM", "ADP", "NOUN", "NUM"]);
        assert_eq!(got[0].lemma, "valve");
        let got = en.tag(&["building", "105", "valve", "ID", "fan"]).unwrap();
        assert_eq!(got[0].upos, "VERB", "PyTorch Stanza 도 building 을 동사로 봅니다 (질의 조각이라)");
    }
}
