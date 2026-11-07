# Stream — spec

A personal music player for iPhone and Mac. It plays your Spotify account and your
own audio files as one library, with Liquid Glass design, very low memory use, and
fast startup. Built for one user and sideloaded, never shipped to the App Store
(embedding librespot is against Spotify's terms).

- Name: **Stream** (App Store: azaStream)
- Bundle ID: `network.aza.stream`
- Platforms: iOS 26+, macOS 26+ (Liquid Glass APIs only, no fallbacks)
- Appearance: System (default), Light or Dark in Settings; on the Mac also OLED Dark, pure black behind everything (iPhone's dark mode is already black)
- Mockups: `mockups/*.html` (each chosen option is noted below)

## Architecture

```
SwiftUI app (iOS + macOS, one shared codebase)
  │  UniFFI (async + callback interfaces)
  ▼
Rust core  (static lib, packed as StreamCore.xcframework)
  ├─ spotify   librespot 0.8 session + Web API client + PKCE sign-in
  ├─ library   SQLite (FTS5) index of local files + cached Spotify library
  ├─ engine    decoding (symphonia / librespot), queue, gapless, normalization
  ├─ lyrics    Spotify synced lyrics + embedded lyrics + LRCLIB, "best sync wins"
  ├─ scrobble  Last.fm (Spotify and local files), offline queue
  ├─ soundcloud  search, HLS playback and playlists through api-v2
  ├─ playlists your own playlists, kept with aza and copied to Spotify and SoundCloud
  ├─ node      your own files on the Stream node, streamed or downloaded
  └─ sync      aza sync items: settings, pins, what's playing, History
  │  C ABI render callback (lock-free ring buffer)
  ▼
AVAudioEngine (AVAudioSourceNode) → system output, Now Playing, AirPlay
```

- **Swift** owns the UI, the AVAudioSession/AVAudioEngine output, MPNowPlayingInfoCenter and
  remote commands, App Intents (Siri/Shortcuts), Handoff (NSUserActivity), the Mac menu bar
  item, and sign-in browser sheets.
- **Rust** owns everything else. There is one audio pipeline for every source, so gapless
  playback and loudness normalization behave the same for Spotify and local files.
- Audio crosses the FFI as interleaved `f32` frames pulled by the render callback. No
  allocation and no locks on the audio thread.
- **Memory:** artwork goes through a size-bounded on-disk cache and is downsampled with ImageIO
  at display size. The library is paged out of SQLite, never held in RAM in full. Lists are
  lazy.

## Spotify

Works the way spotifast does:

1. **Web API grant**: OAuth PKCE using your own Spotify developer app's Client ID (you
   register it once). Used for library, playlists, search, playback control, and
   artist info.
2. **Playback grant**: OAuth PKCE with Spotify's desktop client identity, only the
   `streaming` scope, exchanged for librespot's reusable credentials. These are stored in the
   Keychain. Premium is required.
3. **Session reads**: catalogue reads over the librespot session (no per-app quota) for things
   the Web API restricts: Made For You, other people's playlists, related artists, synced lyrics,
   artist pages.

Sign-in is one button, the same on iPhone and Mac (core `signin.rs`): it does only the steps
still missing (approval 1, approval 2, connecting) and Settings draws each step from the core.
Saved playback credentials are tried before asking again; refused ones are dropped. Every
failure ends as a sentence and a "Try again". On iPhone the playback session presents itself
as macOS (vendored librespot-core, one line): with a desktop client's token, a session that
says "iPhone" is turned away by every access point ("Try another access point").

Rules:

- librespot only. Spotify tops out at OGG Vorbis 320 kbps; no Soloist.
- The app is **not** a Spotify Connect receiver. It can control and transfer to other Connect
  devices (device picker).
- Mixed playlists live **on Spotify**, using `spotify:local:` entries that resolve to your files.
- Made For You, Discover Weekly and Daily Mixes are tucked inside Library → Playlists.
- **Offline:** manual "Download" on a song, album or playlist (its page, or a long-press). Uses
  librespot's encrypted audio cache for Spotify songs; Uploads are downloaded as their files.
  Both land in Library → **Downloaded**.
- There are no liked songs: the songs you keep go in your playlists. The **+** beside the song
  playing (Now Playing, and the Mac's player bar) adds it to one of them or a new one. An
  album's + saves it to your Spotify library (Library → Albums).

## SoundCloud

Through soundcloud.com's own web player API (api-v2), with no developer app (core `soundcloud.rs`):

- The web player's public client ID is read from soundcloud.com's scripts and found again when
  SoundCloud turns it over (a 401/403). Search and playback need nothing else.
- Signing in (Settings → Services → SoundCloud) opens soundcloud.com in a private web sheet and
  takes the web player's `oauth_token` cookie. It's checked against `/me`, kept in the Keychain and
  synced in `settings`, so other devices are signed in too. It brings your playlists, and lets Stream keep playlists there.
- Audio is HLS: AAC 160 in fragmented MP4 (or MP3), fetched whole into a 1 GB cache and played
  like a file. Songs SoundCloud only offers encrypted (most major-label releases) play from the
  same song found elsewhere (your files or Spotify, by ISRC or tags, the best sounding) instead.

## Services

Settings → Services lists your music (files and Uploads), Spotify, SoundCloud and Last.fm, each
opening its own page. There's no order to set: when a song is on more than one service, it plays
from its best copy. Your files (here or on the Stream node) and Spotify start about as fast, so the
best sounding wins: any lossless file, then by bitrate, Ogg, AAC and Opus counting a quarter more
than MP3 (Spotify is Ogg 320). A SoundCloud song not yet on the device downloads whole before it
plays, so it comes after them. Search shows such a song once, as its best copy, where the first of
them was listed: your library, then Spotify, then SoundCloud. A switch turns a service off without
signing out: it leaves search, Library, Home and playback, and Spotify drops its playback
connection. The switches are synced `settings` (`servicesOff`). Last.fm's page signs in and turns
scrobbling on and off.

If the connection to Spotify's servers drops (sleep, a network change, the phone in the
background), the song playing picks up where it got to on a new session, and the next time the
app comes back a dead session is replaced rather than kept.

## Your playlists

New Playlist (Library → Playlists +, or Add to Playlist in any long-press menu) makes a playlist
kept with aza: the `playlists/<id>` item, merged three ways like `pins` with each song its own
entry, and written empty when deleted. Each one can also be kept on Spotify and SoundCloud
(core `playlists.rs`, `matching.rs`):

- A song remembers where it was found on each service and a file's hash, so finding it happens
  once, on whichever device gets there first. Found means the same ISRC, or the title, first
  artist and length agreeing, with the same version (a remix, live take or edit is another song).
  A song not found is looked for again after a week.
- Stream writes the whole list to its copy there (private on both). What was added or taken out
  there since Stream last wrote it comes back first. A copy deleted there stops being kept; turning
  a service off for the playlist leaves the copy there as yours.
- **Import from Link** (Library → Playlists +) takes a Spotify playlist or album link
  (`open.spotify.com/…`, `spotify:…`) or a SoundCloud set (`soundcloud.com/…/sets/…`, short
  `on.soundcloud.com` links too) and makes it one of your playlists, with its name and songs.
  `network-aza-stream://newplaylist?import=<link>` does the same.
- **Mac:** a click on a song plays it without picking it; ⇧-click picks a run from it without
  playing, ⌘-click picks one more, ⌘A all. Only picked songs are tinted. Your own playlist takes
  ⌘V as soon as its page is open, even empty. ⌘C copies the picked songs (as Spotify links or "title — artist" elsewhere), ⌘X cuts them out
  of your own playlist, ⌘V pastes copied songs into one, and Delete takes them out. A right-click
  on a picked song acts on all of them. Playlist sort choices show inline in the sort menu, as on iPhone.
- Copies don't show again in Library. The playlist's page shows where it's kept (aza, Spotify
  12/14…) and its … menu renames, deletes, and picks the services.

## Local files

- **Mac:** watched folders (FSEvents), linked in place and never copied. Formats: FLAC, ALAC,
  AAC/M4A, MP3, Ogg Vorbis, Opus, WAV, AIFF. Tags read with `lofty`. Artwork comes from
  embedded images or `cover.jpg`/`folder.jpg`.
- **iPhone:** has no music folders. It plays the Mac's files from the Stream node (below):
  streamed while they play, or downloaded from an album or playlist to play offline.
- The library is **merged but not deduplicated**: Spotify and local versions show as separate
  items.
- **Uploads** (Library → Uploads) are your own music files, on the Stream node, and the one
  place to find them: its Songs / Albums / Artists pills list them by song, album and artist.
  They're in search, stream when played, and can be downloaded. **Albums** and **Artists** are
  only what you saved and follow on Spotify. **Downloaded** is only what's on this device to
  play offline; the Mac's own folders are Uploads, not downloads.
- Downloads work as Spotify's do. Songs download one at a time from a queue. An album's or
  playlist's download button turns into a ring that fills as its songs arrive (tap to stop),
  with "Downloading 3 of 12 · About 2 min left" under the actions row. Each song row shows a
  filled arrow once it's here, a filling ring while it downloads and a dotted arrow while it
  waits. Library → Downloaded (with a ring beside it while downloading) lists the song
  downloading now and the ones waiting above what's downloaded, and can cancel them.
- Every list of songs (albums, playlists, an artist's popular songs, Uploads,
  Downloaded) sorts from the same ⇅ menu: the list's own order, title, artist, album or length,
  and reversed. The choice is kept per kind of list.
- Artist info comes from Spotify only. An artist that exists only locally gets a plain page.

## aza sync and the Stream node

Stream signs in with aza (app ID `stream.aza.network`, scopes `sign-in`, `read`, `write`). The
iPhone approves in the aza app on the same phone; the Mac shows a QR code that the aza app scans.

- **aza sync data** (encrypted on your aza home node): `settings` holds the Spotify client ID,
  the Last.fm API key and sign-in, the playback switches (normalization, autoplay, scrobbling)
  and the apps' own preferences (theme, accent, the Now Playing visualizer, how playlists sort),
  so you set them once. A switch or preference is there only when it isn't the default.
  Spotify's own sign-in stays per device: its tokens rotate and are tied to the device that
  asked. So do volume, the Mac's zoom and its music folders. `songs/<sha256>` is one item
  per music file: its tags, path, size, ReplayGain and cover hash, written by the device that has
  the file and written empty when the file goes.
- **History** is synced as `history/<device>/<day>` items, one per device per UTC day, written only
  by that device. A song counts once it's 10 seconds in (past halfway, for a song shorter than 10 seconds). Files are
  matched across devices by their hash.
- **Pins** are the `pins` item: every pin in order. A pinned song that's a file goes by its hash.
- `settings` and `pins` are merged three ways: what a device changed since the version it last
  read or wrote (added, changed, taken out) is kept, the rest comes from the newer version, and
  where both changed the same entry the newer version wins. The first time a device syncs, its
  pins join the ones already synced and it takes your settings over its defaults.
- **What's playing** is the `playing` item: the song, its position, when that was, and whether it
  was playing. Each device writes it when it changes song, plays, pauses or seeks. A device that
  isn't playing takes up a newer one, paused where the other device is (or would be by now).
- **The Stream node** (`node/`, at `stream.aza.network` on azalab-0) stores the files themselves,
  by SHA-256, one folder per aza identity, up to each identity's quota: 10 MB by default, 10 GB for
  aiden. Stream signs in to it with its aza certificate and a signed challenge for a 90-day
  token. Every device uploads its own music folders' files (and covers) the node doesn't have yet,
  then writes their items. A file over the quota stays off the node and Profile says so.
- **Playing a song from the node** streams it: the file arrives into the cache and plays as it
  arrives, and the last 2 GB streamed stay cached. **Download** on an album or playlist keeps its
  songs on the device. A device's own file always wins over the same song from the node.
- Profile → Stream node shows the storage used, uploads in progress and how many songs came from
  other devices.

## Playback

- Gapless by default. Crossfade is a setting, off by default.
- Loudness normalization is one toggle, on by default: ReplayGain tags for local files,
  Spotify's normalization data for Spotify tracks.
- **Mac bit-perfect:** when enabled, the output device's nominal sample rate follows the
  track, with optional hog mode. No resampling and no volume scaling in that mode.
- No EQ.
- **Queue:** Play Next / Play Later above the remaining context (album or playlist).
  Shuffle sits left of the transport and repeat right of it; repeat only loops the current
  song forever. Toggles and saves change on screen at once and roll back if the
  core or Spotify refuses. Autoplay is a setting, off by default.
- **Handoff:** Spotify tracks only (track, position, context URI).

## Lyrics

- Sources: Spotify synced lyrics (word-level where available), embedded lyrics (USLT/SYLT,
  Vorbis `LYRICS`, enhanced LRC), and LRCLIB. **Best sync wins:** word-level beats line-level,
  which beats plain text. Results are cached on disk.
- Shown only inside the app. There's no lyrics widget, menu bar text, or overlay.

## Scrobbling

- Last.fm, **Spotify and local files alike**. Spotify's own Last.fm link only scrobbles plays
  Spotify hears about, and librespot never reports any.
- Standard rules: a track scrobbles once it's past 50% or 4 minutes, and only if it's longer
  than 30 s. "Now playing" updates are sent. Scrobbles queue offline and go out in batches of
  50.
- No Last.fm love/unlove.

## Screens (chosen mockups)

**iPhone**: tabs **Home · Library · Search**, with the mini player as the tab bar's glass
bottom accessory.

| Screen | Choice |
|---|---|
| Now Playing | `01` **A Immersive**: blurred full-bleed art, big art, format/quality chip (e.g. FLAC 24/96, OGG 320). Behind it, Aurora: a 4×4 mesh of the cover's colours, each row lit by its part of the spectrum (bass at the bottom) and pushed outward on beats. The bottom row is Lyrics, AirPlay, Visualizer (waveform, accent when on, slashed when off; saved per device) and Queue |
| Home | `02` **D Pinned grid + recents**, without the floating search pill. You can pin albums, songs, artists and playlists |
| Library | Category list: Playlists, Podcasts, Artists, Albums, Uploads, Downloaded, History |
| Podcasts | Library → Podcasts: the shows you follow on Spotify as a grid. A show's page lists its episodes newest first with the date, description, length or time left, and Played. Play starts the newest episode you haven't finished; an episode resumes where you left it, and counts as played within 15 seconds of the end |
| History | Library → History (not a tab): every song played on any of your devices, newest first, by day, with the time and device |
| Lyrics | `04` **A**: left-aligned, word fill on the active line, distance blur on the others, tap a line to seek |
| Album / Playlist / Artist | `05` **C**: compact header (art + metadata), art-tinted page, actions row (play, shuffle, save, pin). Their songs sort from the ⇅ menu in the actions row (the list's own order, title, artist, album, length, reversed), the same for every playlist, every album and every artist. Artists add their header photo, the whole discography newest first with Albums / Singles / EPs pills, and About (bio, monthly listeners, followers, rank, top cities) |
| Queue | `06` **A** without the toggle row: "Playing Next" + "Continue from …" |
| Settings → Services | `09` **A** One Services list: a row per service and Last.fm with its status, each opening its own page |
| Search | `07` **C**: one relevance-ranked list across library + Spotify, type chips (All, Songs, Albums, Artists, Playlists); the empty state shows recent searches |

**Mac**

| Element | Choice |
|---|---|
| Shell | Glass sidebar floating at the top left, only as tall as its contents, holding the traffic lights; one search field at the top center that stays put across root pages (Home and Search search everything, library sections filter themselves); a floating glass player capsule, centered with a max width, over the content |
| Library sections | Playlists, albums and artists (circles) as grids of covers with names below; songs as lists without separators |
| Artists | An artist's name opens their page everywhere it appears; artist pages come from Spotify's own artist query over the playback session (top songs, monthly listeners, releases) |
| Now Playing | Clicking the art (which lifts under the pointer) opens a full-window immersive Now Playing (same design as iPhone A); a chevron at the top center or Esc returns |
| Menu bar | `08` **A** compact card. Closing the window keeps music playing in the menu bar and hides the Dock icon |
| Keyboard | Shortcuts for every view and transport action, plus a ⌘K command palette (search, play, navigate) |

**Color:** the accent is always Apple Music red (#FA2D48, #FA586A in dark mode), and so is
the app icon. Backgrounds such as Now Playing and album pages are always tinted from the
artwork; each album's palette is computed once and cached.

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
- Haptics on play/pause, skip, pin, save and queue reorder (iPhone).
- Respects Reduce Motion (transitions become crossfades).

## System integration

- iOS: lock screen and Control Center through the system Now Playing card (no Live Activity,
  no widgets), AirPlay, Siri + Shortcuts via App Intents (play / pause / skip, "play album X",
  "play pinned X").
- macOS: media keys, Now Playing, menu bar mini player, and a keep-running-when-closed
  lifecycle.

## Testing

- End-to-end UI tests run in the **iOS Simulator** (`tests/iOSUITests`), which never touches
  the Mac's own mouse or keyboard. Most screens are shared SwiftUI code, so they cover the
  Mac too. macOS XCUITest is not used: it drives the real cursor.
- Tests launch the app with `-musicFolder <tests/fixtures/Music> -uiTesting YES`: an isolated
  library, silent output, nothing persisted. Regenerate the fixtures with
  `tests/make-fixtures.sh`.
- `TourTests` walks every screen and saves screenshots to `/tmp/stream-tour` for layout review.

## Out of scope

App Store, CarPlay, Apple Watch, widgets, Live Activities, EQ, smart playlists, tag editor,
stats, world map, Wikipedia, ListenBrainz, Discord, Soloist / lossless Spotify, acting as a
Connect receiver, Handoff for local tracks, transcoding, lyrics outside the app.

## Repo layout

```
SPEC.md
project.yml           XcodeGen project: Stream-iOS, Stream-macOS, StreamUITests-iOS
Shared/               SwiftUI views and app model used by both apps
iOS/, macOS/          each app's entry point and platform extras
core/                 Rust core (stream-core crate, UniFFI)
node/                 the Stream node (stream-node crate), deployed from .forgejo/workflows/node.yml
Generated/            StreamCore.xcframework + Swift bindings (built, not committed)
tests/                Simulator UI tests and fixture music
scripts/              build-core.sh, deploy.sh, test.sh, demo.py
docs/                 screenshots for the README, design mockups
```
