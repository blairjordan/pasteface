#!/usr/bin/env sh
set -eu
# One-time download. Synthesis and playback subsequently run offline.
root="${HOME}/.local/lib/pasteface/tts"
mkdir -p "$root/voices"
if command -v uv >/dev/null 2>&1; then
    test -x "$root/venv/bin/python" || uv venv --python 3.12 "$root/venv"
    uv pip install --python "$root/venv/bin/python" 'piper-tts==1.4.2'
else
    test -x "$root/venv/bin/python" || python3 -m venv "$root/venv"
    "$root/venv/bin/python" -m pip install 'piper-tts==1.4.2'
fi
cd "$root/voices"
"$root/venv/bin/python" -m piper.download_voices en_US-lessac-medium
printf 'Installed local speech engine and English voice in %s\n' "$root"
