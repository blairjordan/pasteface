"""Offline Kokoro synthesis. Model and voices must already be installed."""
import sys
import wave
from pathlib import Path

import numpy as np
from kokoro_onnx import Kokoro

root, voice, output = sys.argv[1:]
root = Path(root)
engine = Kokoro(str(root / "kokoro-v1.0.onnx"), str(root / "voices-v1.0.bin"))
samples, rate = engine.create(
    sys.stdin.read(), voice=voice, speed=1.0, lang="en-gb" if voice.startswith("b") else "en-us"
)
with wave.open(output, "wb") as audio:
    audio.setnchannels(1)
    audio.setsampwidth(2)
    audio.setframerate(rate)
    audio.writeframes((np.clip(samples, -1, 1) * 32767).astype("<i2").tobytes())
