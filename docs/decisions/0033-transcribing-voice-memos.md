# 33. Transcribing voice memos

- **Status:** Accepted
- **Date:** November 16, 2026

## Context

[Decision 31](0031-notes.md) says a voice memo records in the browser, saves as an audio file beside its note, as images do, and "gets a transcript in the note if the server runs the optional local speech model." The spec asks for voice memos "transcribed on-box" in Notes' quick capture, and lists "Local model runtime and Whisper for transcription" under optional local AI. Its roadmap puts local AI in v1.5, and its open questions ask whether that's v1.5 or v2. This decision covers only voice memos in Notes. Summaries, semantic search, and transcripts of other audio in Drive stay with the later local AI work.

Recording and playing memos needs no new decision. It follows images: a file beside the note, named after it, linked by its path from the top of the Notes place, shown to anyone who can open the note, stored against the owner, and trashed with the note. Chrome and Firefox record WebM files with Opus audio, and Safari records MP4 files with AAC audio. Notes keeps whichever one the browser makes, and Obsidian plays both.

Transcribing is different:

- **It runs a speech model,** a file of hundreds of megabytes that Accordion doesn't have today.
- **It decodes audio people upload,** so it's untrusted input, and [decision 1](0001-where-core-services-run.md) puts it in a sandboxed worker ([decision 14](0014-thumbnails-and-sandboxed-workers.md), [decision 29](0029-sandboxing-workers-without-landlock.md)).
- **It's slow on small servers.** A Raspberry Pi 5 has four cores and no GPU that speech models run well on.
- **Supported systems don't package an engine.** Ubuntu 26.04 has whisper.cpp 1.8.3, but Debian 12, Ubuntu 24.04, and Raspberry Pi OS based on Debian 12, which [decision 9](0009-releases-and-installing.md) supports, don't. No distribution packages the models.

## Decision drivers

1. **Audio never leaves the server.** No cloud speech service, and no connection while transcribing.
2. **Works on the Minimum tier,** a Raspberry Pi 5 with 4 GB of memory, without making the server slow for people using it.
3. **Good transcripts:** punctuation and capital letters, so a transcript reads as a note, in the languages a household speaks.
4. **A compromised decoder or model runner reaches nothing,** as decision 14 requires.
5. **Optional.** A server without it works as now, and a memo just has no transcript.
6. **The same in the .deb and the Umbrel container** ([decision 27](0027-running-on-umbrelos.md)).
7. **Licenses that fit AGPL,** for the engine and the model.
8. **One developer.**

## Options: the engine

### Option A: whisper.cpp with Whisper models

[whisper.cpp](https://github.com/ggml-org/whisper.cpp) runs OpenAI's Whisper models on the processor, in C and C++, with ARM NEON and x86 AVX code. Both the program and the models are MIT-licensed.

| Model | Download | Memory, from whisper.cpp's figures |
|---|---|---|
| base | 142 MB | About 390 MB |
| small | 466 MB | About 850 MB |
| large-v3-turbo, 5-bit | About 550 MB | To be measured |

Pros:

- **99 languages,** detected from the speech, with punctuation and capital letters.
- **The best-known open speech model,** with years of use on small computers.
- **One engine and one file format** from the smallest model to the most accurate, so a faster server picks a larger model and nothing else changes.
- **Ignores silence** with Silero voice activity detection, which whisper.cpp includes (MIT). Whisper otherwise makes up text, such as "Thanks for watching", during long silences.
- **A command-line program,** so the worker runs it the way it runs ffmpeg.

Cons:

- **Speed on a Pi isn't known yet.** Reports put the base model at about real time on four Pi 5 cores, so a 1-minute memo takes about a minute, and larger models slower. Nobody has published careful numbers, so step 1 measures them.
- **Not packaged** on most supported systems, so Accordion builds it.
- **Whisper can still make things up** when speech is unclear, as every model of its kind can.

### Option B: sherpa-onnx with NVIDIA Parakeet

[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) (Apache 2.0) runs speech models with ONNX Runtime. NVIDIA's Parakeet TDT 0.6B v3 (CC BY 4.0) is among the most accurate open models and faster than Whisper on a processor.

Pros:

- **Accurate and fast,** with punctuation and capital letters.

Cons:

- **25 European languages,** with no Asian languages.
- **ONNX Runtime is a large C++ library** to build and sandbox, besides sherpa-onnx.
- **The model is around 0.6 GB** even at 8 bits, with no smaller version for a Pi.
- **CC BY 4.0 needs credit** wherever the model goes, which is manageable but new.

### Option C: Moonshine

[Moonshine](https://github.com/moonshine-ai/moonshine) models are made for small devices and are much faster than Whisper at the same accuracy.

Pros:

- **Fast on a Pi,** with small models.

Cons:

- **Only the English models are MIT-licensed.** The others use the Moonshine Community License, which limits commercial use, so they can't ship with AGPL software that anyone may use for anything.
- **One language** breaks driver 3 for any household that speaks another.

### Option D: Vosk

[Vosk](https://alphacephei.com/vosk/) (Apache 2.0) runs Kaldi models, from about 40 MB.

Pros:

- **Small and fast,** even on older Pis.

Cons:

- **No punctuation or capital letters,** so a transcript reads as one long line of words.
- **Less accurate** than Whisper's base model.

### Option E: The browser's speech recognition

Browsers have a speech recognition API.

Cons:

- **Chrome sends the audio to Google** to recognize it, which breaks driver 1 and the spec's zero-telemetry promise. Firefox has none.

## Options: how the model gets to the server

### Option 1: Download it from Hugging Face when it's turned on

Pros:

- **No hosting.**

Cons:

- **A connection to a company the owner didn't choose,** which sees the server's address.
- **The file can change** under the same name.

### Option 2: Download it from Accordion's own release when it's turned on

The release that first uses a model carries it as a file on GitHub, and the server program knows its address and SHA-256 hash.

Pros:

- **The same place as updates** ([decision 12](0012-updates.md)), so no new company.
- **A changed or damaged file is refused,** because the hash is in the signed server program.
- **Only servers that turn it on download anything.**
- **The same in the .deb and the container.**

Cons:

- **Hosting about 1 GB of models,** though only once per model: the server downloads each one from the release that first carried it.

### Option 3: A separate package

Pros:

- **apt installs it.**

Cons:

- **Accordion has no apt repository.** The install script downloads one package from a release, so a second package would need its own way to install and update.
- **The container** would need it built in, about 0.5 GB larger for everyone, or a different way to add it.

## Comparison

| Driver | A: whisper.cpp | B: Parakeet | C: Moonshine | D: Vosk | E: Browser |
|---|---|---|---|---|---|
| Audio stays home | Yes | Yes | Yes | Yes | No |
| Pi 5 | To be measured | Large model only | Fast | Fast | Not applicable |
| Punctuation | Yes | Yes | Yes | No | Varies |
| Languages | 99 | 25 European | English only | About 20, each its own model | Varies |
| License | MIT | Apache 2.0 and CC BY 4.0 | MIT for English only | Apache 2.0 | Not applicable |
| To build | One program | Two libraries | Its own runtime | Its own library | Nothing |

## Recommendation

**whisper.cpp with Whisper models (option A), downloaded from Accordion's own release when an admin turns it on (option 2), and run in the sandboxed worker.**

### Turning it on

- **Off by default.** In **Settings** > **Notes**, an admin finds **Transcribe voice memos**, with what it does, the model's size, and the space it needs. Turning it on downloads the model, with progress, and the setting says when it's ready.
- **A choice of model,** worded for families: **Faster** (base), **Balanced** (small), and **More accurate** (large-v3-turbo). The recommended one follows the server's hardware, as decision 26's video setting does. Step 1's measurements set the rule. Changing the model downloads the new one before it removes the old.
- **The Network page lists the download** ([decision 11](0011-the-network-page.md)) as a connection to GitHub, made only when the setting is turned on or the model changes.
- **Turning it off** removes the model. Transcripts already in notes stay, since they're the notes' own text.

### Where the program and model live

- **Accordion builds whisper.cpp's command-line program** from a pinned release in its release workflow, for amd64 and arm64, and ships it in the package at `/usr/lib/accordion/whisper`. When every supported system packages whisper.cpp, the package depends on that instead.
- **The models live in the data folder's cache,** under `cache/models/`, which backups leave out. A restored server downloads its model again.
- **A new release can move to a newer model** by changing the hash it knows. The server then downloads the new model and keeps the old one until the new one passes the check.

### Transcribing

- **Notes asks for a transcript when a memo recorded in Notes is saved.** An audio file added another way, such as from Obsidian, gets none.
- **It runs as a background job,** one at a time, at low priority, with at most four threads, so people's requests come first. Memos wait in order.
- **Two steps, both in the sandbox:** ffmpeg turns the memo into 16 kHz mono WAV, reading only WebM or MP4 with its Opus or AAC decoder. Then whisper.cpp reads the WAV and the model and writes plain text. The worker sees only its job folder, which holds the WAV and a hard link to the model, as decision 29 gives a worker its file. It has no network.
- **Limits:** 1.5 GB of memory, as decision 26 gives conversions, and processor time of 10 times the memo's length, plus a minute. Memos over 30 minutes get no transcript.
- **Silence is skipped** with Silero voice activity detection, which ships with the model.
- **The language is detected** from the speech, since a household may speak more than one. Whisper never translates.
- **Without ffmpeg,** memos get no transcripts, and **Server health** says so without a warning, as for other recommended programs.

### Where the transcript goes

- **In the note's text, as a paragraph at the end,** the way Keep puts a memo's words in the note. Search finds it, Obsidian shows it, and the person can edit or remove it like any other text. For a note that is only a memo, the transcript is the whole text.
- **Notes writes it through the co-editing service** ([decision 30](0030-documents-and-co-editing.md)), so it merges with anyone typing in the note at the time, and history shows it as an edit by the person who recorded the memo.
- **While it waits,** the memo in the editor shows **Transcribing…**, and a live update fills in the text when it's ready.
- **If the note no longer links to the memo** when the transcript is ready, because someone removed the memo or deleted the note, the transcript is dropped.
- **If transcribing fails,** the memo has no transcript and the editor says nothing alarming. The memo still plays.

### Security

- **whisper.cpp is C and C++, and it reads the model and audio,** so it runs with decision 14's limits under decision 29's worker service: its own user, no network, and only its job folder.
- **ffmpeg is told the one format to read,** as for previews.
- **The model is checked against its hash** before it's used, not only after it's downloaded, so a model changed on disk is downloaded again.

### Testing

- **The worker transcribes a short recorded clip** with the base model and finds its words. CI caches the model.
- **The sandbox test** runs the transcriber with each kind of access taken away, as decision 17 did for LibreOffice.
- **A wrong hash is refused,** and the setting says the download failed.
- **CI's connection check** fails if transcribing opens any connection.
- **Notes' API tests** check that a transcript lands at the end of the note, merges with an edit made meanwhile, and is dropped when the memo is gone.

## Steps

1. **Measure.** Build whisper.cpp for amd64 and arm64, and time base, small, and large-v3-turbo on a Raspberry Pi 5 (on the test SD card) and on an x86 server, with memos of 15 seconds, 1 minute, and 5 minutes. Record memory use. Set the recommended model for each tier, and turn the feature down on the Minimum tier if even base is too slow.
2. **Recording and playback,** which decision 31 already covers.
3. **The server:** the setting, the download and its check, the job, the sandboxed worker, and writing the transcript.
4. **The web app:** the setting with its progress, and **Transcribing…** on the memo.

## Later

- **A GPU,** through whisper.cpp's Vulkan support, on servers that have one.
- **Transcripts of audio and video in Drive** for search, with the rest of local AI.
- **Words appearing while recording,** which needs streaming and is slower on a Pi.
- **Keep's import** brings memos with the transcripts Keep already made, so they aren't transcribed again.

## Revisit this decision if

- **Step 1 finds base too slow on a Pi 5,** such as more than twice a memo's length. Then English-only households might get Moonshine, if its license allows.
- **Every supported system packages whisper.cpp,** so Accordion stops building it.
- **A better open model** with a permissive license appears.

## Consequences

- **The release workflow** builds whisper.cpp, and attaches a model to the first release that uses it.
- **The package** gains `/usr/lib/accordion/whisper`, a few megabytes.
- **Core's worker** gains a transcription job, with its limits.
- **Notes** gains the transcription job and writes transcripts through co-editing.
- **The web app** gains the setting and the waiting state.
- **The Network page** lists the model download.
