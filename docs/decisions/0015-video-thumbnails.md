# 15. Video thumbnails

- **Status:** Accepted
- **Date:** November 1, 2026
- **Updated:** November 1, 2026, while building it: ffprobe's audio decoders, small videos, leaving ffmpeg out, and how servers get it
- **Partly replaced:** November 9, 2026, by [decision 26](0026-a-thumbnail-and-preview-for-every-file.md), which drops the rule of only the formats and codecs families have

## Context

Drive's grid shows thumbnails of photos, PDFs, and text files ([decision 14](0014-thumbnails-and-sandboxed-workers.md)), but videos still show an icon. Google Drive shows a frame from each video, and families keep a lot of video: phone clips of children, pets, and holidays. Decision 14 left videos for their own decision, in the same sandbox.

The videos families have:

- **iPhones and iPads** save `.MOV` files with HEVC video, or H.264 with **Most Compatible** turned on. Newer models record HDR as HLG, with Dolby Vision information on top.
- **Android phones** save `.mp4` files with H.264 or HEVC, and some record HDR as PQ (HDR10).
- **Camcorders and older cameras** save `.MTS` (AVCHD) and `.AVI` files.
- **Downloads and screen recordings** are often `.webm` or `.mkv`, with VP9 or AV1.

Making a frame means decoding the video, and video is a larger attack surface than images. ffmpeg, the usual tool, reads hundreds of formats. Some of them can point at other files or web addresses, such as HLS playlists and concat lists, and in 2017 a crafted video made ffmpeg send local files to an attacker this way.

## Decision drivers

1. **A compromised decoder reaches nothing.** The same as decision 14: no data folder, no writing, no network.
2. **Only the formats families have.** Every format and codec ffmpeg reads is more code an attacker can aim at.
3. **Fast and small on a Raspberry Pi,** including 4K video from a phone.
4. **HDR videos look right,** not gray and washed out.
5. **Optional.** ffmpeg is large, and an admin with no videos, or no space, should be able to leave it out.
6. **Security fixes come with system updates,** as for libvips.

## Options

### Option A: ffmpeg from the distribution, in the sandbox

The worker runs `ffprobe` to learn the video's length and whether it's HDR, then `ffmpeg` to draw one frame, both through `accordion sandbox`. libvips turns the frame into the thumbnail, as it does for a PDF's page.

Pros:

- **Every format above,** and fixes come with system updates.
- **It can be told exactly what to read.** ffmpeg takes a list of allowed formats, codecs, and protocols, so Accordion turns off everything families don't need.
- **Tone mapping for HDR,** through its `zscale` and `tonemap` filters.

Cons:

- **A large install:** about 168 MB in 92 packages on Ubuntu, measured. Some are shared with libvips.
- **C decoders,** so everything rests on the sandbox and the allowlists, as with libvips.

### Option B: ffmpegthumbnailer

A small program that makes video thumbnails, built on ffmpeg's libraries.

Pros:

- **Made for this job,** with frame choice built in.

Cons:

- **The same decoders as ffmpeg,** and the same install, plus another package.
- **No allowlists.** It reads any format ffmpeg's libraries know, with no way to turn formats or codecs off.
- **No tone mapping.**

### Option C: No video thumbnails

Videos keep their icons, as now.

Pros:

- **Nothing new to install or attack.**

Cons:

- **Drive looks unfinished next to Google Drive,** in the folders families open most.

## Comparison

| Driver | A: ffmpeg | B: ffmpegthumbnailer | C: None |
|---|---|---|---|
| Formats families have | Yes | Yes | No |
| Can turn other formats off | Yes | No | Not needed |
| HDR looks right | Yes | No | Not needed |
| Install size | About 168 MB | About 168 MB | None |
| Optional | Yes | Yes | Not needed |

## Recommendation

**ffmpeg from the distribution (option A), in the sandbox from decision 14, as an optional package.**

### Making a frame

- **Two runs in the sandbox.** `ffprobe` reports the video's length, its first video stream, and its color transfer. `ffmpeg` then draws one frame at most 480 pixels on its longer side, as PNG, and libvips makes the WebP.
- **A video already smaller than 480 pixels stays its size,** as a photo does.
- **The frame from 10% in,** which skips black frames and fade-ins at the start. ffmpeg jumps there through the video's index, so long videos cost no more than short ones. If that finds no frame, the worker takes the first one.
- **The worker reads the video by its path,** with Landlock allowing that one file, as for a HEIC's header. MOV and MP4 files often keep their index at the end, so ffmpeg has to jump around in the file, and Debian 12's ffmpeg can't do that on standard input.
- **Two threads,** like libvips. Using every core made the measured iPhone clip peak at 235 MB instead of 98 MB.
- **Rotation is applied.** Phones store portrait videos sideways with a rotation flag, and ffmpeg turns the frame upright.
- **No metadata.** The frame is a new picture, and libvips strips anything left, so the thumbnail never shows where a video was filmed.

### Only the formats families have

- **The server picks the format from the file's first bytes** and tells ffmpeg to use only that one, so ffmpeg never tries its other formats:

  | First bytes | ffmpeg format | Files |
  |---|---|---|
  | `ftyp`, without a HEIC or AVIF brand | `mov` | `.mov`, `.mp4`, `.m4v`, `.3gp` |
  | Matroska's header | `matroska` | `.mkv`, `.webm` |
  | `RIFF` and `AVI` | `avi` | `.avi` |
  | MPEG transport stream packets | `mpegts` | `.mts`, `.m2ts`, `.ts` |

- **Only these decoders:** H.264, HEVC, VP8, VP9, AV1, MPEG-4 Part 2, MPEG-2, Motion JPEG, and ProRes, which iPhone Pro models can record. Older codecs, such as Windows Media and H.263, are left out. They're rare in family libraries, and each is more code to attack.
- **ffprobe may also open the audio decoders families' videos have:** AAC, MP3, Opus, Vorbis, AC-3, E-AC-3, FLAC, ALAC, and PCM. It opens a decoder for every track and stops at one it may not use, even when asked only about the video. ffmpeg skips the audio, so it never opens them. If a video's audio is something else, ffprobe stops, and the worker takes the first frame without knowing the length or whether it's HDR.
- **Only local files.** ffmpeg may open only the file it's given, so a playlist or a reference inside a MOV file can't make it open anything else, even before the sandbox stops it. MOV's external references are off by default, and stay off.
- **A file with no video,** such as an `.m4a` song in the same format, gets no thumbnail and isn't counted as a failure.

### HDR

- **Tone mapping only for HDR video,** when `ffprobe` reports HLG or PQ. The same filters would shift the colors of an ordinary video.
- **The frame is shrunk first, then tone-mapped.** Tone mapping a whole 4K frame needed more than 750 MB and failed under the 1 GB limit. A 480-pixel frame costs almost nothing.
- **iPhone HDR is HLG,** which is made to look acceptable on ordinary screens, so it looks only a little flat without tone mapping. Android's PQ video looks washed out without it.

### Limits

- **The same as for photos:** 1 GB of writable memory, 20 seconds of CPU time, and 30 seconds in all, for each run.
- **No file size limit.** The 200 MB limit for photos doesn't apply, because ffmpeg reads the index and a few frames, not the whole video. A minute of 4K iPhone video is about 170–400 MB.
- **Measured in the sandbox,** on an x86_64 computer, with two threads:

  | Video | Time | Peak memory |
  |---|---|---|
  | 31-second 1080p HEVC from an iPad, `ffprobe` | 0.07 s | 58 MB |
  | The same, one frame | 0.4 s | 98 MB |
  | 4K H.264 | 0.2 s | 150 MB |
  | 4K HDR HEVC (10-bit HLG), tone-mapped after shrinking | 0.5 s | 313 MB |
  | The same, tone-mapped before shrinking | Stopped | Over 750 MB |
  | 10-minute, 148 MB H.264, a frame at one minute | 0.09 s | 70 MB |
  | A cut-off or garbled MOV | 0.05 s | 53 MB, fails cleanly |

### In Drive

- **The grid shows the frame with a small play symbol,** so a video isn't mistaken for a photo.
- **The Thumbnails setting** says videos show a frame.

### Without ffmpeg

- **The package recommends ffmpeg rather than depending on it.** apt installs recommended packages by default, so the install command brings it in. An admin who doesn't want it can remove it with `sudo apt remove ffmpeg`, which leaves Accordion installed.
- **Videos then keep their icons,** and the **Thumbnails** check in **Server health** says so without a warning, since leaving ffmpeg out is a choice, not a problem.
- **Installing ffmpeg later** makes the worker try the videos again within an hour, without a restart.

### Testing

- CI installs ffmpeg. Tests make short clips with ffmpeg's own test patterns and check:
  - the size of the frame
  - that a portrait clip comes out upright
  - that an HDR clip goes through tone mapping
  - that an audio-only file gets no thumbnail
  - that a cut-off file fails within its limits
- The sandbox tests from decision 14 already check that a worker can't read other files or connect anywhere.
- The upgrade test checks that a video gets a thumbnail.

### Revisit this decision if

- Families ask for formats left out here, such as Windows Media.
- A Raspberry Pi can't decode 4K HEVC within the limits.
- A memory-safe video decoder reads HEVC and H.264 well enough to replace ffmpeg.

## Consequences

- **A new recommended package,** `ffmpeg`.
- **Core's thumbnail service** learns video formats, runs `ffprobe` and `ffmpeg` in the sandbox, and raises its worker version, so videos it skipped get another try.
- **Server health** says when videos have no thumbnails because ffmpeg isn't installed.
- **CI installs ffmpeg** for core's tests.
- **The web app** adds a play symbol to video thumbnails.
- **The next release's notes say `Installs itself: no`.** The update helper installs with `dpkg`, which doesn't install new packages, and that release needs libvips and the HEIC decoder (decision 14) as well as ffmpeg. The install command uses apt, which brings in all three.

## Open questions

- **Speed on a Raspberry Pi.** Measure 4K HEVC there. The Pi 5's hardware HEVC decoder isn't available to a sandboxed ffmpeg, so this is software decoding.
- **Debian 12's ffmpeg** is version 5.1. Check that it has `zscale` and takes the same options.
- **Real HDR video.** Check tone mapping with an iPhone's HDR video and an Android phone's HDR10 video. The measurements above used a test pattern.
- **Install size** of `ffmpeg` on Debian 12 and Raspberry Pi OS, next to libvips.
- **8K video,** which some Android phones record, probably needs more than 1 GB and will fail. That's acceptable unless families ask.
