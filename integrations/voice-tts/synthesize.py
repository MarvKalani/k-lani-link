"""Run one voice synthesis with the operator's locally configured reference."""

import io
import json
import os
from pathlib import Path
import re
import sys
from textwrap import wrap


def text_chunks(text):
    """Keep short sentences together within the Audio8 model's generation context."""
    current = ""
    for sentence in re.split(r"(?<=[.!?])\s+", text):
        for piece in wrap(sentence, width=300, break_on_hyphens=False):
            if current and len(current) + len(piece) + 1 > 300:
                yield current
                current = ""
            current = f"{current} {piece}".strip()
    if current:
        yield current


def audio8(config, text, reference_wav, reference_text):
    import torch
    from transformers import AutoModel, AutoProcessor

    checkpoint = config["audio8_model"]
    processor = AutoProcessor.from_pretrained(checkpoint, local_files_only=True, trust_remote_code=True)
    model = AutoModel.from_pretrained(
        checkpoint, local_files_only=True, trust_remote_code=True, dtype=torch.bfloat16,
    ).eval().to("cuda")
    rate = model.config.codec_sample_rate
    pause = torch.zeros(rate // 5)
    samples = []
    for chunk in text_chunks(text):
        inputs = processor(
            text=[chunk], reference_audio=[str(reference_wav)],
            reference_text=[reference_text], return_tensors="pt",
        )
        result = model.generate(
            **{key: value.to("cuda") for key, value in inputs.items()},
            max_new_tokens=1024, temperature=0.95, top_p=0.92, top_k=60,
            do_sample=True, return_dict_in_generate=True,
        )
        waveforms, lengths = model.decode_audio(result.codes)
        samples.extend((waveforms[0, :int(lengths[0])].float().cpu(), pause))
    return torch.cat(samples[:-1]), rate


def ming(config, text, reference_wav, reference_text):
    sys.path.insert(0, config["ming_source"])
    from ming_tts import MingTTS

    model = MingTTS(config["ming_model"], device="cuda:0", text_normalization=False)
    waveform = model.synthesize(
        task="MULTILINGUAL", language_id="German", text=text,
        prompt_wav_path=str(reference_wav), prompt_text=reference_text,
        use_spk_emb=True, cfg=2.4, temperature=0.35,
    )
    return waveform.squeeze(0), model.sample_rate


def main():
    voice = Path(sys.argv[1])
    config_path = Path(sys.argv[2]) if len(sys.argv) > 2 else voice / "model.json"
    config = json.loads(config_path.read_text(encoding="utf-8"))
    request = json.load(sys.stdin)
    reference_wav = voice / "reference.wav"
    reference_text = (voice / "reference.txt").read_text(encoding="utf-8").strip()
    synthesize = {"audio8-tts-realistic-air": audio8, "ming-3.0-tts-0.5b": ming}[request["model"]]
    # Model libraries can print diagnostics on stdout. Reserve the original
    # stdout descriptor for the WAV and discard diagnostics in the parent.
    with os.fdopen(os.dup(1), "wb") as output:
        sys.stdout.flush()
        os.dup2(2, 1)
        import numpy as np
        import soundfile as sf
        import torch

        with torch.inference_mode():
            waveform, rate = synthesize(config, request["input"], reference_wav, reference_text)
        samples = waveform.float().cpu().numpy()
        if samples.ndim != 1 or not samples.size or not np.isfinite(samples).all():
            raise ValueError("invalid waveform")
        wav = io.BytesIO()
        sf.write(wav, samples, rate, format="WAV", subtype="PCM_16")
        output.write(wav.getbuffer())


if __name__ == "__main__":
    main()
