"""Zoro-1 비전(mmproj) 수치 대조: transformers 의 Qwen3_5VisionModel 로 같은 패치를 계산해 엔진 결과와 견줍니다.

    cargo run -p blueprint-core --release --example zoro_vision -- dump <이미지> out/zoro_vision   (BLUEPRINT_DEVICE=cpu 권장)
    python scripts/zoro_vision_reference.py out/zoro_vision

엔진의 패치(`patches.f32`, 패치마다 채널 · 행 · 열 한 프레임)를 시간축 두 프레임으로 늘려 transformers 의 입력 모양
`[패치, 3 × 2 × 16 × 16]` 으로 넣고, mmproj GGUF 의 BF16 가중치를 F32 로 올려 이미지 토큰을 계산합니다.
필요한 것: numpy, torch, transformers 5.x (검증 전용, 앱은 쓰지 않습니다).
"""

from __future__ import annotations

import json
import os
import struct
import sys
from pathlib import Path

import numpy as np

TYPES = {0: ("F32", 4), 1: ("F16", 2), 30: ("BF16", 2)}


def read_gguf(path: Path):
    """메타데이터와 텐서(이름 → (dims, 형식, 절대 위치)). 비전 mmproj 는 BF16 · F32 뿐입니다."""
    with open(path, "rb") as f:
        assert f.read(4) == b"GGUF"
        (version,) = struct.unpack("<I", f.read(4))
        n_tensors, n_kv = struct.unpack("<QQ", f.read(16))

        def rstr():
            (n,) = struct.unpack("<Q", f.read(8))
            return f.read(n).decode("utf-8")

        def rval(t):
            fmt = {0: "B", 1: "b", 2: "H", 3: "h", 4: "I", 5: "i", 6: "f", 7: "?", 10: "Q", 11: "q", 12: "d"}
            if t in fmt:
                return struct.unpack("<" + fmt[t], f.read(struct.calcsize(fmt[t])))[0]
            if t == 8:
                return rstr()
            if t == 9:
                (et,) = struct.unpack("<I", f.read(4))
                (n,) = struct.unpack("<Q", f.read(8))
                return [rval(et) for _ in range(n)]
            raise ValueError(t)

        meta = {}
        for _ in range(n_kv):
            k = rstr()
            (t,) = struct.unpack("<I", f.read(4))
            meta[k] = rval(t)
        infos = []
        for _ in range(n_tensors):
            name = rstr()
            (nd,) = struct.unpack("<I", f.read(4))
            dims = struct.unpack("<" + "Q" * nd, f.read(8 * nd))
            (ty,) = struct.unpack("<I", f.read(4))
            (off,) = struct.unpack("<Q", f.read(8))
            infos.append((name, dims, ty, off))
        align = meta.get("general.alignment", 32)
        base = (f.tell() + align - 1) // align * align
    return meta, {n: (d, t, base + o) for n, d, t, o in infos}


def load_tensor(path: Path, dims, ty, offset) -> np.ndarray:
    kind, size = TYPES[ty]
    count = int(np.prod(dims))
    raw = np.fromfile(path, dtype=np.uint8, count=count * size, offset=offset)
    if kind == "BF16":
        v = (raw.view(np.uint16).astype(np.uint32) << 16).view(np.float32)
    elif kind == "F16":
        v = raw.view(np.float16).astype(np.float32)
    else:
        v = raw.view(np.float32)
    return v.reshape(tuple(reversed(dims)))  # GGUF 는 ne0(가장 빠른 축)부터


def main() -> None:
    import torch
    from transformers.models.qwen3_5.configuration_qwen3_5 import Qwen3_5VisionConfig
    from transformers.models.qwen3_5.modeling_qwen3_5 import Qwen3_5VisionModel

    out = Path(sys.argv[1] if len(sys.argv) > 1 else "out/zoro_vision")
    models = Path(os.environ.get("BLUEPRINT_MODELS_DIR", Path(os.environ["LOCALAPPDATA"]) / "blueprint" / "models"))
    mmproj = Path(sys.argv[2]) if len(sys.argv) > 2 else models / "zoro" / "mmproj-zoro-bf16.gguf"
    meta, tensors = read_gguf(mmproj)
    source = json.loads(meta.get("zeze.source_config_json") or meta["zorogravity.source_config_json"])
    vcfg = Qwen3_5VisionConfig(**source["vision_config"])
    model = Qwen3_5VisionModel(vcfg).eval().float()
    state = {}
    for name, (dims, ty, off) in tensors.items():
        if not name.startswith("model.visual."):
            continue
        t = load_tensor(mmproj, dims, ty, off)
        key = name[len("model.visual.") :]
        if key == "patch_embed.proj.weight":
            t = t.reshape(t.shape[0], 3, 2, t.shape[2], t.shape[3])  # [D, 3×2, 16, 16] → [D, 3, 2, 16, 16]
        state[key] = torch.from_numpy(np.ascontiguousarray(t))
    missing, unexpected = model.load_state_dict(state, strict=False)
    assert not unexpected, unexpected
    assert not [m for m in missing if "inv_freq" not in m], missing

    info = json.loads((out / "meta.json").read_text(encoding="utf-8"))
    gh, gw, ps = info["grid_h"], info["grid_w"], info["patch"]
    one = np.fromfile(out / "patches.f32", dtype=np.float32).reshape(gh * gw, 3, ps * ps)
    two = np.stack([one, one], axis=2).reshape(gh * gw, 3 * 2 * ps * ps)  # (채널, 시간, 행, 열)
    with torch.no_grad():
        ref = model(torch.from_numpy(two), grid_thw=torch.tensor([[1, gh, gw]])).pooler_output
    ref = ref.numpy()
    got = np.fromfile(out / "tokens.f32", dtype=np.float32).reshape(ref.shape)
    rel = float(np.linalg.norm(got - ref) / np.linalg.norm(ref))
    cos = (got * ref).sum(-1) / (np.linalg.norm(got, axis=-1) * np.linalg.norm(ref, axis=-1) + 1e-12)
    report = {"device": info["device"], "tokens": list(ref.shape), "rel_err": rel, "min_cos": float(cos.min()), "mean_cos": float(cos.mean())}
    (out / "reference.json").write_text(json.dumps(report, indent=1), encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
