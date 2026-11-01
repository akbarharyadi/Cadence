# Karaoke that follows the singer: automatic lyrics-to-vocals alignment

Research and a working test, 2026-11-01. The question was whether Cadence can make the karaoke highlight follow the actual singing automatically, by matching the lyric text to the vocals, in the languages the user listens to: Japanese, Indonesian, English, Korean and Chinese.

**Short answer: yes.** A multilingual forced aligner (Meta's MMS, run through ONNX) placed lyric lines within about 0.1–0.3 s of LRCLIB's human-made timings on 8 songs from the user's library, with no help from those timings. It also gives a time for every word, which the karaoke view can already show. Cadence already has the hard parts around it: romanization for every one of those scripts, an enhanced-LRC parser and a word-by-word sweep. The new pieces are one ONNX model, about 100 lines of Viterbi alignment, and optionally a vocal-separation model that removes most of the bad misses.

## Built: "Follow singer" (2026-11-01)

The design below was implemented in `src-tauri/src/voice/`, with one change after measuring: the vocal separator is **MDX-Net "Voc_FT"** (UVR, packaged as ONNX by sherpa-onnx), not HTDemucs.

| Separator | Line accuracy, Wolf / Moon / Kita (≤1 s, unguided) | Extra RAM while separating | CPU time (2 threads) | Download |
|---|---|---|---|---|
| HTDemucs (ONNX, STFT on the host) | 98% / 88% / 88% | ~1.9 GB per 7.8 s piece, plus 440 MB of weights | ~58 s per 4-min song | 174 MB |
| **MDX-Net Voc_FT** | **98% / 94% / 90%** | **~1.1 GB per 5.9 s piece, plus 71 MB** | ~86 s | **67 MB** |
| Spleeter 2-stems (MIT) | 80% / 74% / 86% | small | ~4 s | 2 × 39 MB |

How it works in Cadence:
- **Download.** Opt-in, about 440 MB, from Settings or the lyrics view: ONNX Runtime 1.31.1, loaded at runtime with `ort` `load-dynamic`, so the installer doesn't grow. Windows takes `onnxruntime.dll` from the PyPI wheel (15 MB), and Linux x64 and macOS arm64 take it from the GitHub release. The download also has MDX-Net Voc_FT and MMS-FA int8 (`Vivek2259/mms-fa-mm8`). Every file is checked against a BLAKE3 hash.
- **Listening.** Decode, then 44.1 kHz stereo into MDX-Net (STFT/iSTFT in Rust with `realfft`), then 16 kHz mono into MMS (30 s windows with 2 s of context). The models load one at a time with the ONNX Runtime arena off, and are dropped after use. ORT's threads are created through a `ThreadManager` that puts them in background mode, two threads at most.
- **What is saved.** The listening is kept at `%LOCALAPPDATA%\com.cadence.player\voice\<id>.bin` (u8 log-probs, about 1.4 KB per second of song, checked against the file stamp). The placed lyrics are kept at `%APPDATA%\com.cadence.player\lyrics\<id>.voice.json` (version, text hash, lines, and the on/off choice). A song is listened to only once. New or edited lyrics are placed again from the saved listening in under a second.
- **Placing.** The Japanese, Chinese and Korean text is split into morae, characters and syllables, so the sweep moves syllable by syllable. The LRC times guide each line (±2.5 s after the song's offset), and a line more than 2 s away keeps its LRC time. Text in parentheses and "la-la"/"oh" lines are left to the blank. If fewer than half the lines can be placed, the lyrics are probably wrong and nothing is saved.
- **UI.**
  - The lyrics toolbar has a "Follow singer" chip. It shows "Listening n%" while working and toggles between the placed timing and the LRC timing.
  - Placed lyrics get their own Timing nudge.
  - The toolbar now stays on one row, with "Search for other lyrics", "Paste lyrics" and "Hide these lyrics" in a ⋯ menu.
  - The sidebar shows "Following the singer n%" while working.
  - Settings has a "Lyrics follow the singer" section for the download, the automatic switch and removal.
  - With automatic on, every song whose lyrics are opened is listened to, plus the next song in the queue if it already has lyrics.
- **Batch** (Settings → "Follow the singer in all songs with lyrics"):
  - **Which songs:** every song that has lyrics without asking LRCLIB (chosen, saved, a `.lrc` file or lyrics in the tags), isn't skipped, and hasn't been listened to. Most-played and most recently played go first.
  - **Scale:** finding them in the 4,629-song library took about 28 s and found 786 songs, roughly 20 hours of background work.
  - **Progress:** shown in the sidebar and Settings, with the current song's share and the time left.
  - **Stopping:** Stop halts the current song within a step.
  - **Restarts:** if Cadence closes while a batch runs, it carries on a minute after the next start (`settings.voiceBatch`).
  - **Order:** songs whose lyrics the user opens, and the next song in the player queue, still go first.
- **Measured** in a debug build of the app, on Wolf: 55 of 55 lines placed, median error 0.22 s, 100% within 1 s with LRC guidance. Peak process memory was 1.37 GB (median 0.83 GB), back to its usual size right after. CPU averaged 1.7 cores, and the song took about 150 s. A release build should be faster, because the STFT loops in a debug build aren't optimized.
- **Not available** on phones, Intel Macs, or ARM Windows and Linux. ONNX Runtime 1.31 ships no build for those, or the models are too heavy for the device.

## What Cadence does today

- Lyrics come from LRCLIB, embedded tags or sidecar files. All 315 cached synced lyrics in the user's library are **line-level only**: none has word timings.
- `lyrics.rs` already parses enhanced LRC (`<mm:ss.xx>word`) into `Line.words`, and `LyricsView.svelte` sweeps word by word when those are present. Without them it spreads the sweep evenly over the line, so the highlight runs ahead of or behind the singer within a line.
- The **Timing −/+** nudge fixes a constant offset per song, but only by hand.
- The **Vocals** slider (`audio/karaoke.rs`) is a mid/side voice-band cut, not real separation.
- `romanize.rs` turns Japanese into Hepburn romaji (kakasi), Chinese into pinyin and Korean into Revised Romanization. That is exactly the input the MMS aligner wants (see below).

## How automatic alignment works

This is **forced alignment**. Recognizing the lyrics from scratch is not needed: the text is already known, and the job is only to find *when* each letter is sung.

1. An acoustic model turns the audio into a matrix of letter probabilities, one row every 20 ms (CTC emissions).
2. The lyrics become a sequence of letters.
3. A Viterbi pass finds the single most likely path through that matrix that visits the letters in order. The "blank" symbol absorbs music, breaths and silence between them.
4. The first and last frame of each letter give word and line start/end times.

The model is the only expensive part. Steps 2–4 are simple dynamic programming: a vectorized NumPy version aligned a whole song in 0.2–0.8 s, and Rust will be faster.

### Model choice

| Option | Languages | Notes | Verdict |
|---|---|---|---|
| **MMS-300M forced aligner** (`MahmoudAshraf/mms-300m-1130-forced-aligner`, the same model as torchaudio's `MMS_FA`) | 1,130, through romanized text (a–z plus `'`) | wav2vec2 CTC. Needs romanized input, which `romanize.rs` already produces. ONNX exports exist: fp16 about 632 MB ([Project42](https://huggingface.co/Project42/mms-300m-align), [tatsu020](https://huggingface.co/tatsu020/mms-300m-fa-onnx)) and int8-MatMul about 356 MB ([Vivek2259](https://huggingface.co/Vivek2259/mms-fa-mm8); fully int8 caused multi-second misses). License **CC BY-NC 4.0**. | **Use this.** It covers all five languages with one model. |
| WhisperX / stable-ts / whisper.cpp timestamps | ~100 | Whisper's own word timestamps are clearly worse than wav2vec2 alignment ([WhisperX paper](https://arxiv.org/pdf/2303.00747)). It hallucinates on singing, for example English words in Japanese lyrics ([whisperX #1308](https://github.com/m-bain/whisperX/issues/1308)). | Only for songs with **no** lyrics at all (transcription), which is a separate feature. |
| Singing-specific aligners (SOFA, Stoller/Huang research models) | Mostly single-language | SOFA targets clean, studio-dry vocals for singing-synthesis datasets and is beta ([SOFA](https://github.com/spicytigermeat/SOFA)). The research models are English or need training. The 2023 multilingual contrastive model (<0.2 s average error on JamendoLyrics) has no released weights ([arXiv 2306.07744](https://arxiv.org/abs/2306.07744)). | Not practical now. |
| Montreal Forced Aligner / Julius | Per-language models | Speech models with a Kaldi toolchain, hard to ship ([MFA Japanese](https://huggingface.co/MontrealCorpusTools/japanese_mfa)). | No. |

### Vocal separation first?

Separating the vocals before alignment is the usual recipe in lyrics-alignment pipelines ([syncalong](https://pypi.org/project/syncalong/), [wispr-lrc](https://pypi.org/project/wispr-lrc/), [Nomad Karaoke](https://github.com/orgs/nomadkaraoke)). The test below confirms it helps on dense mixes. Candidate models:

- **htdemucs** (Demucs v4, MIT, 80 MB). ONNX export works if the STFT is moved outside the graph ([demucs #530](https://github.com/facebookresearch/demucs/issues/530), [HF exports](https://huggingface.co/kickvn/htdemucs-onnx)). Mixxx did a full-graph ONNX export in GSoC 2025 ([Mixxx](https://mixxx.org/news/2025-10-27-gsoc2025-demucs-to-onnx-dhunstack/)).
- **MDX-Net Kim_Vocal_2** (UVR) has better vocal SDR (about 9.5 vs 8.4 dB) and is already ONNX, but its weights carry no stated license ([GGUF port note](https://huggingface.co/thepatch/mdx-net-kim-vocal-2-GGUF), [MVSEP benchmark](https://replicate.com/lucataco/mvsep-mdx23-music-separation/readme)).

A side benefit: the same vocals stem could later drive a real "Vocals" slider, like Apple Music Sing's on-device separation ([TechCrunch](https://techcrunch.com/2022/12/06/apple-music-is-getting-a-new-karaoke-like-feature-apple-sing)), instead of the mid/side cut.

## Test on the user's library

Each song was decoded and resampled to 16 kHz mono. MMS (PyTorch, fp16 on the RTX 3080 Ti) produced the emissions, and the plain CTC Viterbi aligned the cached LRCLIB text, romanized the same way Cadence does (kakasi for Japanese, toneless pinyin for Chinese, NFKD and lowercase for Latin). **The aligner never saw LRCLIB's timestamps.** Its line starts were then compared with them.

"Offset-removed" subtracts the song's median difference first, because a constant shift means LRCLIB's timing is offset, which is exactly what the Timing nudge fixes. LRCLIB's timings are human-made and imperfect themselves, so these numbers measure agreement between two estimates, not true error.

| Song | Language | Full mix: median error / lines within 1 s | Vocals stem (htdemucs) |
|---|---|---|---|
| CHiCO with HoneyWorks – Wolf | Japanese | 0.22 s / 95%, three lines 9–13 s off | **0.23 s / 100%**, worst line 1.0 s off |
| SawanoHiroyuki[nZk]:LiSA – narrative | Japanese (rock) | 0.25 s / 84% | **0.19 s / 97%** |
| LiSA – Brave Freak Out | Japanese + English | 0.08 s / 89% | – |
| Isyana Sarasvati – Tap Tap Tap | Indonesian | 0.09 s / 97% | – |
| Bondan Prakoso & Fade2Black – Kita Selamanya | Indonesian | 0.17 s / 88% | 0.16 s / 90% |
| Green Day – Basket Case | English (punk) | 0.31 s / 100% | 0.30 s / 100% |
| Sam Smith – I'm Not The Only One | English | **+3.07 s constant offset found**; 0.28 s / 70% after it | – |
| Wuthering Waves – Moon Over a Thousand Peaks | Chinese | 0.10 s / 88% | 0.10 s / 88% |

No Korean song in the library has cached synced lyrics, so Korean is untested. MMS covers Korean and `romanize.rs` already produces its input.

What the misses have in common:

- **Backing vocals and vocalizations.** The worst lines were "La-la-la-la, ah-ah-ah", "(Uh...)" and lines in parentheses. They are often quiet, layered or not sung as written. Fix: leave parenthesized text and pure vocalizations out of the alignment and give them the LRC time.
- **Lyrics for another version.** Sam Smith's LRCLIB file ends at 228 s, but the file is 220 s long. The aligner found the 3 s offset at once, then had to squeeze the outro. This is a useful by-product: a large or drifting offset means "probably the wrong lyrics".
- **Repeated choruses in long outros.** In Kita Selamanya the last three lines landed in different places with and without separation. Use low confidence there.
- **Dense mixes without separation.** Wolf's three 9–13 s misses all disappeared on the vocals stem.

The word-level output looks right when read against the song. A demo file is at
`C:\Users\akbar\AppData\Local\Temp\claude\C--Users-akbar-AppWork-Cadence\4e74f414-b4c7-494f-841e-38eeff649d49\scratchpad\Wolf-word-synced.lrc`
(enhanced LRC with the original kanji and kana, timed per word). There is no word-level ground truth to score it against. The literature puts state-of-the-art word onsets at under 0.2 s average error ([Stoller et al.](https://www.researchgate.net/publication/332791880_End-to-end_Lyrics_Alignment_for_Polyphonic_Music_Using_an_Audio-to-character_Recognition_Model)).

### Cost

| Step | GPU (3080 Ti) | CPU, 2 threads, low priority (PyTorch fp32) |
|---|---|---|
| MMS emissions, 3:51 song | 0.7 s | **56 s** (7.3 s per 30 s window) |
| htdemucs vocals stem | 7 s | about 100 s (13.4 s per 30 s) |
| Viterbi (NumPy) | 0.2–0.8 s | same |

ONNX Runtime with the int8-MatMul model will probably be faster than PyTorch fp32 on CPU, but that wasn't measured. Even without separation, alignment runs about 4× faster than real time on Cadence's ≤2-thread background budget, so the playing song can be aligned while it plays.

## Recommended design for Cadence

1. **Optional "Sync lyrics to the singer" download** in Settings: the MMS ONNX model (356 MB int8-MatMul, or 632 MB fp16), loaded with `ort` (already in the roadmap for CLAP). The CC BY-NC weights are fine for a non-commercial project. Download them at runtime rather than bundling them in the GPL release.
2. **Per song, on demand.** When the lyrics view is open, align the playing song in `jobs::background_pool()` with the sidebar indicator. Optionally pre-align the next song in the queue. Do not run a silent library-wide job.
3. **Text.** Reuse `romanize.rs` per line, then NFKD, lowercase, and keep `a–z'`. Keep the mapping from each romanized token back to the original segment (kakasi segments for Japanese, characters for Chinese, syllable blocks for Korean), so word timings land on the original text. A finer per-kana sweep is possible later.
4. **Alignment.**
   - *With LRC:* estimate the global offset first, then align each line inside a window of its LRC time plus the offset ±3 s. This rules out multi-second misses and lets results stream ahead of playback.
   - *With plain lyrics:* align the whole song, which turns unsynced lyrics into synced ones.
   - Score each line by its mean letter log-probability. Low-confidence lines keep the LRC time and the even sweep.
5. **Output.** Save the word timings into the lyrics cache as `Line.words`. The UI needs no new rendering. Keep the original timings so "Use original timing" can revert, and set the Timing nudge automatically from the measured offset.
6. **Phase 2:** an htdemucs vocals stem before alignment (+80 MB, about 100 s of CPU per song), and later the same stem for a real Vocals slider.
7. **Android:** later or never. A 356 MB model and the CPU cost suit the desktop. Android could reuse timings synced from a desktop.

### Alternatives that don't replace alignment

- **Ready-made word-timed lyrics.** Musixmatch richsync (crowd-synced, paid or unofficial API), Apple Music TTML (needs a subscription), and NetEase YRC / QQ QRC (unofficial APIs, but strong on Japanese and Chinese) could fill gaps. The [AMLL TTML DB](https://github.com/Steve-xmh/amll-ttml-db) is CC0 but small and Chinese-community focused. Each has coverage, terms-of-service or stability problems, so use them as an extra source at most.
- **DSP-only auto-offset** (vocal-energy onsets cross-correlated with LRC line starts) needs no model but gives only a line-level offset. The aligner gives the offset for free, so it isn't worth building separately.

## Test scripts

These are throwaway Python scripts in the session scratchpad (`align_test.py`, `separate.py`, `to_elrc.py`). The model sits in `models\mms` (1.26 GB fp32). The Viterbi in `align_test.py` is the reference for a Rust port.
