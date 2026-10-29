"""Offline Kokoro synthesis with progress measured in bounded text parts."""
import json
import os
import sys
import pysbd
import wave
from pathlib import Path


def text_parts(text):
    sentences = pysbd.Segmenter(language="en", clean=False).segment(text)
    parts = []
    pending = ""
    for sentence in sentences:
        if not sentence.strip():
            continue
        # Release the first complete sentence promptly; batch later sentences.
        if not parts:
            parts.append(sentence)
        elif pending and len(pending) + len(sentence) > 300:
            parts.append(pending)
            pending = sentence
        else:
            pending += sentence
    if pending:
        parts.append(pending)
    return parts


def synthesize(root, voice, output, text):
    root, output = Path(root), Path(output)
    progress = output.with_suffix(".progress.json")
    parts = text_parts(text)
    if not parts:
        raise ValueError("No text to synthesize")

    def report(stage, completed):
        temp = progress.with_suffix(".tmp")
        temp.write_text(json.dumps(dict(stage=stage, completed=completed, total=len(parts))))
        temp.replace(progress)

    report("loading", 0)
    import numpy as np
    import onnxruntime as rt
    from kokoro_onnx import Kokoro

    options = rt.SessionOptions()
    options.intra_op_num_threads = min(4, os.cpu_count() or 1)
    options.inter_op_num_threads = 1
    session = rt.InferenceSession(str(root / "kokoro-v1.0.onnx"), sess_options=options, providers=["CPUExecutionProvider"])
    engine = Kokoro.from_session(session, str(root / "voices-v1.0.bin"))
    report("generating", 0)
    for index, part in enumerate(parts):
        samples, rate = engine.create(part, voice=voice, speed=1.0, lang="en-gb" if voice.startswith("b") else "en-us")
        target = output.parent / f"part-{index:06d}.wav"
        temp = target.with_suffix(".tmp")
        with wave.open(str(temp), "wb") as audio:
            audio.setnchannels(1)
            audio.setsampwidth(2)
            audio.setframerate(rate)
            audio.writeframes((np.clip(samples, -1, 1) * 32767).astype("<i2").tobytes())
        temp.replace(target)
        report("generating", index + 1)
    report("done", len(parts))


if __name__ == "__main__":
    synthesize(*sys.argv[1:], sys.stdin.read())
