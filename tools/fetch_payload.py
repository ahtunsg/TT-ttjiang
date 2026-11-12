"""Rebuild the offline OCR payload in vendor/payload for TT酱 3.3.0.

The payload is not committed (~20 MB of binaries). Run this once on a dev
machine before publish.bat:

    python tools/fetch_payload.py

Contents, all redistributable:
  - PP-OCRv6 tiny det/rec ONNX models (RapidOCR v3.10.0, Apache-2.0)
  - keys.txt, generated from the rec model's own `character` metadata
  - onnxruntime.dll + onnxruntime_providers_shared.dll (MIT)
  - VC++ 2015-2022 runtime DLLs, app-local deployment

IMPORTANT: the dictionary is NOT downloaded from PaddleOCR. The CTC decoder
in src/Capture/PaddleOcr.cs builds a table of (dictionary lines + 2) classes:
index 0 = blank, 1..N = characters, N+1 = space. PP-OCRv6 tiny rec emits 6906
classes for 6904 characters. Feeding it a v5 dictionary (18383 lines) makes
every decoded index land on the wrong character, and drops the space class.
Always derive keys.txt from the exact rec.onnx that ships with the build.
"""
from __future__ import annotations

import hashlib
import shutil
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PAYLOAD = ROOT / "vendor" / "payload"

# Pinned to the RapidOCR v3.10.0 release so a rebuild reproduces the exact
# models the 3.3.0 code was tuned against. Hashes are from RapidOCR's
# default_models.yaml (multi_PP-OCRv6_det_tiny / multi_PP-OCRv6_rec_tiny).
MODEL_BASE = ("https://www.modelscope.cn/models/RapidAI/RapidOCR/"
              "resolve/v3.10.0/onnx/PP-OCRv6")
MODELS = {
    "det.onnx": (
        f"{MODEL_BASE}/det/PP-OCRv6_det_tiny.onnx",
        "f42c0fbd294d95eac1a550e131b277dac97462c8025fa4b6c3cec1b7894bd3d5",
    ),
    "rec.onnx": (
        f"{MODEL_BASE}/rec/PP-OCRv6_rec_tiny.onnx",
        "e16e242de5937ad92609223f19bc2aff3727ee40b095f996907c24749bad251b",
    ),
}

# Shipped app-local next to onnxruntime.dll so target machines do not need a
# machine-wide VC++ redistributable install.
VC_RUNTIME = ["msvcp140.dll", "msvcp140_1.dll", "vcruntime140.dll", "vcruntime140_1.dll"]

REQUIRED = ["det.onnx", "rec.onnx", "keys.txt", "onnxruntime.dll",
            "onnxruntime_providers_shared.dll"] + VC_RUNTIME


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download(url: str, target: Path, expected: str) -> None:
    # A leftover v5 payload must be replaced, not merely skipped by filename.
    if target.exists() and sha256(target) == expected:
        print(f"  keep {target.name} ({target.stat().st_size/1048576:.1f} MB)")
        return
    if target.exists():
        print(f"  replace {target.name} (hash mismatch - not the v6 tiny model)")
    print(f"  get    {target.name} <- {url}")
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = target.with_suffix(target.suffix + ".part")
    try:
        with urllib.request.urlopen(url, timeout=300) as response, open(tmp, "wb") as out:
            shutil.copyfileobj(response, out)
        actual = sha256(tmp)
        if actual != expected:
            print(f"[ERROR] {target.name} sha256 mismatch: {actual}")
            raise SystemExit(2)
        tmp.replace(target)
    finally:
        if tmp.exists():
            tmp.unlink()


def write_keys(rec: Path, keys: Path) -> None:
    try:
        import onnxruntime as ort
    except ImportError:
        print("[ERROR] python package 'onnxruntime' is needed to read the rec "
              "model's character table (pip install onnxruntime).")
        raise SystemExit(2)

    session = ort.InferenceSession(str(rec))
    raw = session.get_modelmeta().custom_metadata_map.get("character")
    if not raw:
        print("[ERROR] rec.onnx has no 'character' metadata; wrong model?")
        raise SystemExit(2)
    chars = raw.split("\n")
    # A trailing newline would add an empty token and shift every class index.
    while chars and chars[-1] == "":
        chars.pop()

    classes = session.get_outputs()[0].shape[-1]
    if classes != len(chars) + 2:
        print(f"[ERROR] rec emits {classes} classes but the dictionary has "
              f"{len(chars)} entries (expected {len(chars) + 2}).")
        raise SystemExit(2)

    # No trailing newline: File.ReadAllLines in C# must see exactly len(chars) rows.
    data = "\n".join(chars).encode("utf-8")
    if keys.exists() and keys.read_bytes() == data:
        print(f"  keep {keys.name} ({len(chars)} entries)")
    else:
        keys.write_bytes(data)
        print(f"  write {keys.name} ({len(chars)} entries, {classes} CTC classes)")


def find_ort_dir() -> Path | None:
    """Locate a matched pair of ORT DLLs (never mix builds).

    C:/Windows/System32/onnxruntime.dll is a different, older Windows ML build
    with no providers_shared.dll beside it. Mixing it with our OrtApi binding
    breaks the native calls. The python wheel for 1.20.1 is exactly the build
    src/Capture/OrtApi.g.cs was generated against.
    """
    try:
        import onnxruntime  # noqa: F401
        capi = Path(onnxruntime.__file__).parent / "capi"
        if (capi / "onnxruntime.dll").exists() and \
           (capi / "onnxruntime_providers_shared.dll").exists():
            if not onnxruntime.__version__.startswith("1.20."):
                print(f"[WARN] onnxruntime python package is {onnxruntime.__version__}; "
                      "OrtApi.g.cs targets 1.20.1. Prefer "
                      "`pip install onnxruntime==1.20.1`.")
            return capi
    except Exception:
        pass
    system = Path(r"C:\Windows\System32")
    if (system / "onnxruntime.dll").exists() and \
       (system / "onnxruntime_providers_shared.dll").exists():
        return system
    return None


def main() -> int:
    PAYLOAD.mkdir(parents=True, exist_ok=True)
    for name, (url, digest) in MODELS.items():
        download(url, PAYLOAD / name, digest)
    write_keys(PAYLOAD / "rec.onnx", PAYLOAD / "keys.txt")

    ort_dir = find_ort_dir()
    if ort_dir is None:
        print("[ERROR] no matched onnxruntime.dll + onnxruntime_providers_shared.dll "
              "pair found. Run `pip install onnxruntime==1.20.1`, or copy both DLLs "
              "from the ocr folder of a 3.3.0 release into vendor/payload/.")
        return 1
    for dll in ["onnxruntime.dll", "onnxruntime_providers_shared.dll"]:
        source = ort_dir / dll
        print(f"  copy   {dll} ({source.stat().st_size/1048576:.1f} MB) from {ort_dir}")
        shutil.copy2(source, PAYLOAD / dll)

    for dll in VC_RUNTIME:
        source = Path(r"C:\Windows\System32") / dll
        if not source.exists():
            print(f"[WARN] missing {dll}; target machines then need the "
                  "VC++ 2015-2022 runtime installed")
            continue
        shutil.copy2(source, PAYLOAD / dll)

    missing = [n for n in REQUIRED if not (PAYLOAD / n).exists()]
    if missing:
        print("[ERROR] payload incomplete: " + ", ".join(missing))
        return 1
    total = sum((PAYLOAD / n).stat().st_size for n in REQUIRED) / 1048576
    print(f"payload ready: {PAYLOAD} ({total:.1f} MB, {len(REQUIRED)} files)")
    print("now run publish.bat; the payload is copied to dist\\ocr")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
