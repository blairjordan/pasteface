#!/usr/bin/env sh
set -eu
# One-time download. Synthesis and playback subsequently run offline.
root="${HOME}/.local/lib/pasteface/tts"
mkdir -p "$root/voices"
if command -v uv >/dev/null 2>&1; then
    test -x "$root/venv/bin/python" || uv venv --python 3.12 "$root/venv"
    uv pip install --python "$root/venv/bin/python" 'kokoro-onnx==0.6.1' 'pysbd==0.3.4'
else
    test -x "$root/venv/bin/python" || python3 -m venv "$root/venv"
    "$root/venv/bin/python" -m pip install 'kokoro-onnx==0.6.1' 'pysbd==0.3.4'
fi
"$root/venv/bin/python" - "$root/voices" <<'PYMODEL'
from pathlib import Path
import sys
import urllib.request
root = Path(sys.argv[1])
base = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.1/"
for name in ("kokoro-v1.0.onnx", "voices-v1.0.bin"):
    target = root / name
    if not target.is_file():
        print("Downloading", name, flush=True)
        temp = target.with_suffix(target.suffix + ".part")
        urllib.request.urlretrieve(base + name, temp)
        temp.replace(target)
PYMODEL
printf 'Installed local Kokoro speech engine and voices in %s\n' "$root"
