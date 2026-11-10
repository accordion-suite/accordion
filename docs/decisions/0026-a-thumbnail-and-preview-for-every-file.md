# 26. A thumbnail and a preview for every file

- **Status:** Accepted
- **Date:** November 9, 2026
- **Replaces:** the "only the formats families have" rule in [decision 15](0015-video-thumbnails.md), for thumbnails and previews

## Context

A test folder of 152 sample files showed how much of Drive falls back to a generic icon:

- **Types:** about half the samples got the generic file icon, because their extensions weren't in Drive's type lists. Some lists left out an extension on purpose because another program also uses it, such as `.sln` or `.pes`.
- **Thumbnails:** decision 14 covers JPEG, PNG, GIF, WebP, TIFF, HEIC, AVIF, PDF, and text. Decision 15 adds video, but only the containers and codecs that phones and camcorders make. Decision 17 adds Office documents. Photoshop files, camera raw files, SVG, DjVu, and older video formats get none.
- **Previews:** the viewer shows only what the browser plays or draws itself: a few image types, PDF, MP4 and WebM, and six audio formats. Everything else offers a download. HEIC and TIFF show their 480-pixel thumbnail instead.

The rule this decision adopts: **every file gets the icon and type of its extension's most common use, and every file gets a thumbnail and a preview wherever one can be made.** Google Drive does the same for most formats by converting them on its servers.

A browser can't show most of these formats, so a preview means Accordion converts the file: a large picture, a PDF, or audio and video the browser plays. Converting takes processor time and disk space. Servers range across the spec's hardware tiers, from a 4-core computer with 4 GB of memory, such as a Raspberry Pi 5, to small team servers with 6 or more cores and servers with a GPU. Some have a hardware video encoder (Intel Quick Sync, AMD and Intel through VA-API, NVIDIA NVENC), and some, including the Pi 5, have none.

## Decision drivers

1. **Every file shows something when it's opened:** its picture, its pages, or it plays.
2. **A compromised decoder reaches nothing.** Every new decoder and encoder runs in decision 14's sandbox, with no data folder, no writing, and no network.
3. **Scales with the server.** It works on the Minimum tier without making the server slow for everyone, and a faster server, or one with a hardware encoder, gets previews sooner.
4. **Previews are copies, so they're cache.** They're left out of backups and can be made again, and they have a size limit.
5. **Types match what most people mean,** not a cautious "file".
6. **New tools are optional,** like ffmpeg and LibreOffice. Without one, its formats keep their icon, as now.

## Types and icons

- **Each extension gets the type of its most common use.** When two programs share an extension, the more common one wins, even when the other is a type Drive knows. For example:

  | Extension | Most common use | Not |
  |---|---|---|
  | `.sln` | Visual Studio solution (Code) | Asterisk telephone audio |
  | `.pfm` | Windows printer font metrics (Fonts) | Portable FloatMap pictures |
  | `.prc` | Mobipocket e-books (E-books) | Psion voice notes |
  | `.paf` | Personal Ancestral File family trees (no type yet) | Ensoniq PARIS audio |
  | `.ima` | Floppy disk images | IMA ADPCM audio |
  | `.wve` | Wondershare video editor projects | Psion voice notes |
  | `.vox` | Dialogic telephone audio (Audio) | MagicaVoxel 3D models |
  | `.hdr` | Radiance HDR pictures (Photos) | Science data headers |
  | `.pes` | Brother embroidery patterns (Design files) | |

  Where the most common use isn't clear, the extension keeps the generic icon until someone reports what their files are.
- **The contents can overrule the name** when the server knows better. A `.ts` file with a video thumbnail is a video, and one with a text thumbnail is TypeScript. The web app already does this for the viewer.
- **New types for kinds of files families and developers keep,** each with its own icon and search filter:

  | Type | Examples |
  |---|---|
  | Code | `.js`, `.py`, `.rs`, `.sln`, `.sh`, `.bat` |
  | Fonts | `.ttf`, `.otf`, `.woff2`, `.pfm` |
  | E-books | `.epub`, `.mobi`, `.azw3`, `.fb2` |
  | Design files | `.psd`, `.ai`, `.xcf`, `.sketch`, and embroidery patterns such as `.pes` |

- **The server's and the web app's lists become one list.** Today they're two copies kept in step by hand. The web app gets the list from the server instead, once when it starts.

## Thumbnails

The worker gains these, each in the sandbox with only the reader it needs:

| Files | Tool | How |
|---|---|---|
| Photoshop, Targa, icons, JPEG 2000, Kodak Photo CD, PCX, PICT, DDS, EXR, SGI, Sun raster, XWD, WBMP, Netpbm | ffmpeg | Its image decoder for that one format, as for BMP |
| Camera raw (Canon, Nikon, Sony, Fujifilm, Olympus, Panasonic, Pentax, Samsung) | LibRaw (`libraw-bin`) | The full-size JPEG preview the camera embeds, which libvips then shrinks. Nearly every raw file has one. |
| SVG | librsvg (`librsvg2-bin`) | Drawn as a picture. librsvg never runs an SVG's scripts. |
| DjVu | djvulibre (`djvulibre-bin`) | The first page |
| Radiance HDR and FITS | libvips | Only the one loader for that format, named by the worker. libvips doesn't trust these loaders with hostile files, so it blocks them by default, but nothing else reads these formats, and the sandbox contains them as it does ffmpeg's older decoders. |
| MNG that starts with a PNG | libvips | The PNG picture, whose chunks MNG keeps as they are. The worker only finds where they start and end. |
| Songs with cover art | ffmpeg | The picture embedded in the song, as MP3, M4A, FLAC, WMA, Matroska, and other formats keep it. ffprobe first says whether there is one, so a song without cover art keeps its icon. |
| Windows TV recordings, MXF, RealMedia | ffmpeg | A frame, as for other video |
| Fonts | FreeType, through ffmpeg's `drawtext` | "Aa" and a line of sample text in the font |

Formats with no decoder, such as MNG that starts with a JPEG, keep their icon. So do MIDI files, which need a synthesizer to play. So do embroidery patterns for now: no packaged program reads them, so a thumbnail would need a stitch reader of Accordion's own.

HEIC files go up to 64 megapixels, from decision 14's 24, since a 51-megapixel HEIC took about 540 MB, within a worker's 1 GB. AVIF stays at 24 until its decoder is measured.

## Previews

A preview is a copy for the viewer. The file itself never changes:

- **The worker only reads the file.** The sandbox lets it write only in an empty scratch folder, or nowhere, so it sends the preview back on its output or leaves it in that folder, and the server keeps it.
- **Downloads, sync, link sharing, export, and backups always use the original.** Only the viewer shows or plays the preview.
- **Previews are named by the hash of the contents they show,** as thumbnails are, so each version has its own and copies share one.
- **Previews are cache.** Backups leave them out, and a removed one is made again the next time someone opens the file.
- **A thumbnail made again gets a new address.** Browsers keep thumbnails for a year, so the address names the thumbnail's version as well as the contents. A better thumbnail from a newer worker shows at once, instead of the one the browser kept.

The worker makes a preview after the thumbnail, in the same sandbox, and keeps it in `cache/previews/`:

| Files | Preview |
|---|---|
| Pictures the browser can't show | A WebP, at most 2560 pixels on its longer side |
| SVG | A WebP, drawn as above, so an SVG's scripts never reach the browser |
| DjVu, Apple Pages, Numbers, and Keynote | A PDF of the first 50 pages, as decision 17 does for Office documents |
| Audio the browser can't play | AAC in MP4, at 128 kbit/s and at most stereo, which every browser plays. ffmpeg reads each format with only its own demuxer, and tracker music (`.mod`, `.xm`, `.s3m`, and `.it`) through libopenmpt. The worker decides by the codec, not the name, so Apple Lossless in an M4A and MP2 named as an MP3 are converted too. Sound over 3 hours isn't converted. |
| Older sound formats only SoX reads: CD audio (`.cdda`), CVSD (`.cvsd`, `.cvs`, `.cvu`, `.dvms`, and `.vms`), raw 8-bit sound (`.fssd` and `.sou`), Grandstream ringtones (`.gsrt`), Sounder and SoundTool (`.sndr` and `.sndt`), Yamaha TX16W (`.txw`), Ensoniq PARIS (`.paf` and `.fap`), SampleVision (`.smp`), Psion voice notes (`.wve`), HTK speech (`.htk`), Dialogic telephone sound (`.vox`), and Sound Designer II (`.sd2`) | The same AAC. SoX (`sox`) reads each with only its own reader and writes WAV to the worker's scratch folder, which ffmpeg converts. Formats with a signature are known by their first bytes, since SoundTool files are often named `.snd`, TX16W sounds `.w01`, and other programs use `.paf`, `.smp`, and `.wve`. Drive calls such a file audio once its sound is converted. Formats without a header play at the rate SoX assumes for the format, which may not be the rate they were made at. Sound Designer II keeps its rate in a Mac resource fork, which an upload leaves behind, so it plays as 16-bit stereo at 44.1 kHz. These formats can't hold cover art, so they keep their icon in the grid. |
| Video in a container the browser can't open, with codecs it can play | The same video and audio, repackaged as MP4 without re-encoding. Fast, and the picture is unchanged. |
| Video with audio the browser can't play, such as AC-3 from a camcorder | The video repackaged, and only the audio converted |
| Video with codecs the browser can't play, such as Windows Media or MPEG-2 | Converted to H.264 and AAC, at most 1080p (see the options below) |
| Fonts | A page of sample text in the font, drawn in the browser from the font file itself |

On this x86 test PC, repackaging a 3-minute MKV took 3 seconds, and converting a minute of 960×400 Windows Media video to H.264 took 2.5 seconds. Both need measuring on the Minimum tier, such as a Raspberry Pi 5, and with a hardware encoder.

### When video is converted

#### Option A: In the background, after upload

Pros:

- **Ready when someone opens it.**

Cons:

- **A large import keeps the processor busy for hours,** such as a Google Takeout with years of old camcorder video.
- **Doubles the disk space** of every video that needs it, including ones nobody watches.

#### Option B: When someone first opens it, then kept

Pros:

- **Only the videos people watch,** so far less work and space.
- **The cache can drop the least recently watched,** and the next viewer waits again.

Cons:

- **The first viewer waits.** The viewer offers the download meanwhile, and a live update ([decision 19](0019-live-updates.md)) starts playback when it's ready.

#### Option C: While it plays

ffmpeg converts in short pieces as the browser asks for them (HLS).

Pros:

- **Plays within seconds,** with nothing kept.

Cons:

- **Much more to build:** seeking, several viewers at once, and stopping conversions nobody's watching.
- **Every viewing costs the full conversion again.**
- **Small servers may not keep up** with 1080p in real time.

#### Recommendation

**Option A for repackaging and audio on every server, since they're fast. For full video conversion, the server picks A or B by what it has, and an admin can change it.**

- **Option A on a server with a hardware video encoder, or 6 or more cores** (the Small team tier and up). Conversions run at low priority, one at a time, so people using the server don't notice.
- **Option B on smaller servers,** such as the Minimum and Recommended tiers.
- **Settings, under Storage,** has **Convert videos for playback** with **After upload**, **When first opened**, and **Never**. The default comes from the rule above, and the page says why it was chosen.
- **A hardware encoder is used when the server has one.** ffmpeg finds it, and falls back to x264 when it fails. The sandbox then allows the GPU's render device (`/dev/dri/renderD*` or NVIDIA's devices), and nothing else new. That adds the GPU driver to what a crafted video can reach, so the encoder only ever reads frames ffmpeg has already decoded in software, never the file itself.

Option C can come later if waiting turns out to matter more than expected.

### Cache limits

- **Previews share one limit,** 10% of the data disk by default, which an admin can change in **Settings** under **Storage**.
- **Past the limit, the least recently opened previews go first.** Thumbnails don't count toward it, since they're small.
- **Files over 4 GB get no converted video preview.** They still get a thumbnail and the download.

## Security

- **Every new reader runs in the sandbox** with decision 14's limits, except conversions, which get more time, since a long file takes minutes: up to 15 minutes of processor time for audio, and up to an hour for video.
- **ffmpeg is still told the one format to read** and the decoders it may use. The lists grow from decision 15's, but each file still reaches only one demuxer.
- **SoX is told the one format to read, too.** It reads the stored file by its full path, which never starts with `|` or an address, so SoX never takes it for a command to run or a file to download.
- **Older decoders have had more security bugs,** especially JPEG 2000, EXR, Photoshop, FITS, the Windows Media codecs, and SoX's format readers. The sandbox contains a compromised one: it can't read other files, write anywhere but its own empty scratch folder, or reach the network.

## Without the tools

- **libraw-bin, librsvg2-bin, djvulibre-bin, and sox are recommended packages,** like ffmpeg. Removing one leaves its formats with an icon and a download, and **Server health** says so without a warning.

## Steps

1. Types: the shared list from the server, the new types, and the most-common-use rule.
2. Picture thumbnails and previews: ffmpeg's image formats, camera raw, SVG, and DjVu.
3. Audio: cover art thumbnails, and converted previews.
4. Video: repackaging, audio conversion, then full conversion when first opened.
5. Fonts and e-books.
6. Measure each on the Minimum tier (a Raspberry Pi 5) and on a server with a hardware encoder, and adjust the defaults.
