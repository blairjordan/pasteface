"""Run with python3 -m unittest discover -s scripts -p 'test_*.py'."""
import json
from pathlib import Path
import struct
import sys
import tempfile
import types
import unittest
from unittest.mock import patch
import wave

from synthesize import synthesize, text_parts


class SynthesisTests(unittest.TestCase):
    def test_parts_preserve_words_and_unicode(self):
        text = "First paragraph.\n" + "A sentence with café and 世界. " * 35
        parts = text_parts(text)
        self.assertGreater(len(parts), 1)
        self.assertEqual("".join(parts), text)
        self.assertTrue(all(len(part) <= 300 for part in parts))

    def test_chunks_never_split_long_sentences_or_abbreviations(self):
        first = "Dr. Smith paid $3.50 for it. "
        long_sentence = "This sentence " + "continues " * 60 + "until here. "
        text = first + long_sentence + "The final sentence."
        parts = text_parts(text)
        self.assertEqual(parts[0], first)
        self.assertIn(long_sentence, parts)
        self.assertEqual("".join(parts), text)

    def test_all_parts_are_written_and_progress_finishes(self):
        calls = []
        class Samples:
            def __mul__(self, _): return self
            def astype(self, _): return self
            def tobytes(self): return struct.pack("<4h", 1, 2, 3, 4)
        class Engine:
            @staticmethod
            def from_session(*_): return Engine()
            def create(self, text, **_):
                calls.append(text)
                return Samples(), 24000
        modules = {
            "numpy": types.SimpleNamespace(clip=lambda *_: Samples()),
            "onnxruntime": types.SimpleNamespace(SessionOptions=types.SimpleNamespace, InferenceSession=lambda *_, **__: None),
            "kokoro_onnx": types.SimpleNamespace(Kokoro=Engine),
        }
        with tempfile.TemporaryDirectory() as root, patch.dict(sys.modules, modules):
            output = Path(root) / "speech.wav"
            text = "This is a synthetic test sentence. " * 40
            synthesize(root, "af_heart", output, text)
            self.assertEqual(calls, text_parts(text))
            chunks = sorted(Path(root).glob("part-*.wav"))
            self.assertEqual(len(chunks), len(calls))
            for chunk in chunks:
                with wave.open(str(chunk)) as audio:
                    self.assertEqual(audio.getnframes(), 4)
                    self.assertEqual(audio.getframerate(), 24000)
            progress = json.loads(output.with_suffix(".progress.json").read_text())
            self.assertEqual(progress, dict(stage="done", completed=len(calls), total=len(calls)))
