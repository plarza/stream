# Music — spec

A personal music player for iPhone and Mac. It plays your Spotify account and your
own audio files as one library, with Liquid Glass design, very low memory use, and
fast startup. Built for one user and sideloaded, never shipped to the App Store
(embedding librespot is against Spotify's terms).

- Name: **Music**
- Bundle ID: `network.aza.music`
- Platforms: iOS 26+, macOS 26+ (Liquid Glass APIs only, no fallbacks)
- Appearance: follows the system (light and dark are both designed)
- Mockups: `mockups/*.html` (each chosen option is noted below)

## Architecture

```
SwiftUI app (iOS + macOS, one shared codebase)
  │  UniFFI (async + callback interfaces)
  ▼
Rust core  (static lib, packed as MusicCore.xcframework)
  ├─ spotify   librespot 0.8 session + Web API client + PKCE sign-in
  ├─ library   SQLite (FTS5) index of local files + cached Spotify library
  ├─ engine    decoding (symphonia / librespot), queue, gapless, crossfade, normalization
  ├─ lyrics    Spotify synced lyrics + embedded lyrics + LRCLIB, "best sync wins"
  ├─ scrobble  Last.fm (local files only), offline queue
  └─ sync      Mac → iPhone file sync over the LAN (Bonjour + TCP)
  │  C ABI render callback (lock-free ring buffer)
  ▼
AVAudioEngine (AVAudioSourceNode) → system output, Now Playing, AirPlay
```

- **Swift** owns the UI, the AVAudioSession/AVAudioEngine output, MPNowPlayingInfoCenter and
  remote commands, App Intents (Siri/Shortcuts), Handoff (NSUserActivity), the Mac menu bar
  item, and sign-in browser sheets.
- **Rust** owns everything else. There is one audio pipeline for every source, so gapless
  playback, crossfade and loudness normalization behave the same for Spotify and local files.
- Audio crosses the FFI as interleaved `f32` frames pulled by the render callback. No
  allocation and no locks on the audio thread.
- **Memory:** artwork goes through a size-bounded on-disk cache and is downsampled with ImageIO
  at display size. The library is paged out of SQLite, never held in RAM in full. Lists are
  lazy.

## Spotify

Works the way spotifast does:

1. **Web API grant**: OAuth PKCE using your own Spotify developer app's Client ID (you
   register it once). Used for library, playlists, search, likes, playback control, and
   artist info.
2. **Playback grant**: OAuth PKCE with Spotify's desktop client identity, only the
   `streaming` scope, exchanged for librespot's reusable credentials. These are stored in the
   Keychain. Premium is required.
3. **Session reads**: catalogue reads over the librespot session (no per-app quota) for things
   the Web API restricts: Made For You, other people's playlists, related artists, synced lyrics.

Rules:

- librespot only. Spotify tops out at OGG Vorbis 320 kbps; no Soloist.
- The app is **not** a Spotify Connect receiver. It can control and transfer to other Connect
  devices (device picker).
- Mixed playlists live **on Spotify**, using `spotify:local:` entries that resolve to your files.
- Made For You, Discover Weekly and Daily Mixes are tucked inside Library → Playlists.
- **Offline (iPhone):** manual "Download" on an album or playlist. Uses librespot's encrypted
  audio cache. Only Spotify downloads take space this way, since local files are already
  synced.
- Heart = Spotify Like (Liked Songs). Hearting a local track sets a local "liked" flag so it
  shows up in Liked Songs.

## Local files

- **Mac:** watched folders (FSEvents), linked in place and never copied. Formats: FLAC, ALAC,
  AAC/M4A, MP3, Ogg Vorbis, Opus, WAV, AIFF. Tags read with `lofty`. Artwork comes from
  embedded images or `cover.jpg`/`folder.jpg`.
- **iPhone:** gets *everything* over Wi-Fi from the Mac, with original files kept (no
  transcode). Sync runs automatically whenever the iPhone app is open on the same network as
  the Mac. The Mac advertises over Bonjour, and transfers are resumable and content-hashed.
- The library is **merged but not deduplicated**: Spotify and local versions show as separate
  items.
- One **downloaded** badge (⬇) marks any track playable offline, whether it's a synced local
  file or a Spotify download.
- Artist info comes from Spotify only. An artist that exists only locally gets a plain page.

## Playback

- Gapless by default. Crossfade is a setting, off by default.
- Loudness normalization is one toggle, on by default: ReplayGain tags for local files,
  Spotify's normalization data for Spotify tracks.
- **Mac bit-perfect:** when enabled, the output device's nominal sample rate follows the
  track, with optional hog mode. No resampling and no volume scaling in that mode.
- No EQ.
- **Queue:** Play Next / Play Later above the remaining context (album or playlist).
  Shuffle and repeat live in Now Playing's ⋯ menu. Autoplay is a setting, off by default.
- **Handoff:** Spotify tracks only (track, position, context URI).

## Lyrics

- Sources: Spotify synced lyrics (word-level where available), embedded lyrics (USLT/SYLT,
  Vorbis `LYRICS`, enhanced LRC), and LRCLIB. **Best sync wins:** word-level beats line-level,
  which beats plain text. Results are cached on disk.
- Shown only inside the app. There's no lyrics widget, menu bar text, or overlay.

## Scrobbling

- Last.fm, **local files only** (Spotify's own Last.fm link already scrobbles Spotify).
- Standard rules: a track scrobbles once it's past 50% or 4 minutes, and only if it's longer
  than 30 s. "Now playing" updates are sent. Scrobbles queue offline and go out in batches of
  50.
- No Last.fm love/unlove.

## Screens (chosen mockups)

**iPhone**: tabs **Home · Library · Search**, with the mini player as the tab bar's glass
bottom accessory.

| Screen | Choice |
|---|---|
| Now Playing | `01` **A Immersive**: blurred full-bleed art, big art, format/quality chip (e.g. FLAC 24/96, OGG 320) |
| Home | `02` **D Pinned grid + recents**, without the floating search pill. You can pin albums, songs, artists and playlists |
| Library | Category list: Playlists, Artists, Albums, Songs, Downloaded |
| Lyrics | `04` **A**: left-aligned, word fill on the active line, distance blur on the others, tap a line to seek |
| Album / Playlist / Artist | `05` **C**: compact header (art + metadata), art-tinted page, actions row (play, shuffle, like, pin) |
| Queue | `06` **A** without the toggle row: "Playing Next" + "Continue from …" |
| Search | `07` **C**: one relevance-ranked list across library + Spotify, type chips (All, Songs, Albums, Artists, Playlists); the empty state shows recent searches |

**Mac**

| Element | Choice |
|---|---|
| Shell | `03` **A**: glass sidebar + floating glass player capsule over the content |
| Now Playing | Clicking the art opens a full-window immersive Now Playing (same design as iPhone A); Esc returns |
| Menu bar | `08` **A** compact card. Closing the window keeps music playing in the menu bar and hides the Dock icon |
| Keyboard | Shortcuts for every view and transport action, plus a ⌘K command palette (search, play, navigate) |

**Color:** the accent is Apple Music red by default and can be changed in Settings
(including "Album Art", which follows the playing cover). Backgrounds such as Now Playing
and album pages are always tinted from the artwork; each album's palette is computed once
and cached.

## Motion

Everything should feel like Apple's own apps:

- The mini player expands into Now Playing through a matched geometry transition (the art
  flies and resizes). Interactive swipe-down to dismiss, which follows your finger and settles
  with a spring.
- Glass elements morph with `glassEffectID` + `GlassEffectContainer` (play button, chips, the
  tab bar ↔ mini player).
- Spring-based (`.smooth`/`.snappy`) state changes. Art scales down on pause and back up on
  play (Apple Music style). Lyrics scroll on a spring, and the word fill is interpolated per
  frame with `TimelineView`.
- Scroll transitions on grids and lists (subtle scale/opacity). Album art crossfades between
  tracks, and the background tint animates its color change.
- Haptics on play/pause, skip, pin, like and queue reorder (iPhone).
- Respects Reduce Motion (transitions become crossfades).

## System integration

- iOS: lock screen and Control Center through the system Now Playing card (no Live Activity,
  no widgets), AirPlay, Siri + Shortcuts via App Intents (play / pause / skip, "play album X",
  "play my liked songs", "play pinned X").
- macOS: media keys, Now Playing, menu bar mini player, and a keep-running-when-closed
  lifecycle.

## Testing

- End-to-end UI tests run in the **iOS Simulator** (`tests/iOSUITests`), which never touches
  the Mac's own mouse or keyboard. Most screens are shared SwiftUI code, so they cover the
  Mac too. macOS XCUITest is not used: it drives the real cursor.
- Tests launch the app with `-musicFolder <tests/fixtures/Music> -uiTesting YES`: an isolated
  library, silent output, nothing persisted. Regenerate the fixtures with
  `tests/make-fixtures.sh`.
- `TourTests` walks every screen and saves screenshots to `/tmp/music-tour` for layout review.

## Out of scope

App Store, CarPlay, Apple Watch, widgets, Live Activities, EQ, smart playlists, tag editor,
stats, world map, Wikipedia, ListenBrainz, Discord, Soloist / lossless Spotify, acting as a
Connect receiver, Handoff for local tracks, transcoding, lyrics outside the app.

## Repo layout

```
SPEC.md
core/                 Rust workspace (music-core crate, UniFFI)
apps/project.yml      XcodeGen project: Music-iOS, Music-macOS
apps/Shared/          SwiftUI views, models, design system (glass, motion)
apps/iOS/, apps/macOS/  platform entry points and extras
scripts/build-core.sh builds MusicCore.xcframework + Swift bindings
mockups/              chosen UI mockups
```
