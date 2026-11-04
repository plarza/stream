# <img src="docs/icon.png" width="40" align="absmiddle" alt=""> Stream

<img src="docs/screenshots/home.png" width="160" alt="Home: pins and recents"> <img src="docs/screenshots/now-playing.png" width="160" alt="Now Playing"> <img src="docs/screenshots/lyrics.png" width="160" alt="Synced lyrics"> <img src="docs/screenshots/album.png" width="160" alt="An album"> <img src="docs/screenshots/search.png" width="160" alt="Search">

<img src="docs/screenshots/mac.png" width="832" alt="The Mac app">

<table>
<tr><td><b>Platform</b></td><td>iPhone, iOS 27, and Mac, macOS 27</td></tr>
<tr><td><b>Data</b></td><td>On your devices. The library index is SQLite in the app's container; Spotify credentials are in the Keychain, on this device only. Signed in with aza, the Spotify client ID, Last.fm and the list of your own songs are aza sync data, and your music files are on the Stream node (<code>stream.aza.network</code>, on azalab-0)</td></tr>
<tr><td><b>Network</b></td><td>Spotify, LRCLIB for lyrics, Last.fm if you sign in, and with aza your aza home node and the Stream node</td></tr>
<tr><td><b>Account</b></td><td>Spotify Premium and your own Spotify developer app for Spotify. Local files need nothing. Sign in with aza is offered once, on first launch</td></tr>
<tr><td><b>Permissions</b></td><td>None</td></tr>
<tr><td><b>Install</b></td><td><code>scripts/deploy.sh</code> to this Mac and a connected iPhone, or TestFlight. Not for the App Store: it plays Spotify through librespot</td></tr>
</table>

## Features

### One library

- Your Spotify account and your own files side by side: FLAC, ALAC, AAC, MP3, Ogg Vorbis, Opus, WAV and AIFF.
- Home is a grid of pins (albums, songs, artists, playlists) above your recents; long-press anything to pin it.
- Library is Playlists, Artists, Albums, Songs and Downloaded. Made For You, Discover Weekly and Daily Mixes sit in Playlists.
- One search across your library and Spotify, ranked together, with type chips and recent searches.
- Download a Spotify album or playlist to play offline; one ⬇ badge marks anything playable offline.
- Artist pages as on Spotify: header photo, top songs, every album, single and EP with filter pills, bio, listeners and top cities.
- Sort a playlist by title, artist, album or length; a song by several artists opens any one of them.

### Playing

- Now Playing is tinted from the album art, with the format and quality ("FLAC 24/96", "OGG 320") under the cover.
- Synced lyrics, word by word where the source has it; tap a line to seek.
- A queue of Play Next and Play Later above the rest of the album or playlist.
- Lock Screen, Control Center, AirPlay, and Siri and Shortcuts through App Intents.

### Mac

- A floating glass sidebar and player over the page, and ⌘K to play or go to anything.
- A menu bar mini player; closing the window keeps the music going there.
- The Mac's music folders go up to the Stream node; the iPhone streams them, or downloads an album or playlist to play offline. Signed in with aza, Spotify's client ID and Last.fm come along too.

### You

- Last.fm scrobbling for Spotify and your own files, theme, colour and your aza account, all in Profile.

## Rules

| Topic | Rule |
|---|---|
| Spotify sign-in | Two approvals in the system's sign-in sheet: your account through your own developer app (Web API), then playback through Spotify's desktop client (`streaming` only). Saved playback credentials are tried before asking again |
| Audio | One pipeline for every source: Rust decodes into a ring buffer that an `AVAudioSourceNode` pulls from. Gapless always; crossfade off by default |
| Loudness | One toggle, on by default: ReplayGain tags for files, Spotify's own normalization for Spotify |
| Lyrics | Spotify's, the file's own tags and LRCLIB; the best sync wins (word over line over plain) and is cached |
| Scrobbling | Spotify and local files alike: Spotify's own Last.fm link never hears of plays through librespot. A track counts past half or 4 minutes if it's over 30 seconds; scrobbles queue offline and go out 50 at a time |
| Sync | aza sync data: `settings`, and `songs/<sha256>` for each music file. The files are on the Stream node (`node/`), by hash, up to each aza identity's quota; they stream while they play and download on request. Originals, never transcoded, checked by hash |
| Library | Spotify and local copies of the same song stay separate items |
| Likes and toggles | Change on screen at once, and roll back if the core or Spotify refuses |

## Data

| Where | Holds |
|---|---|
| `Application Support/network.aza.stream/library.sqlite` | The library: local files, Spotify's library, pins, recents |
| `Caches/network.aza.stream/` | Artwork, lyrics, Spotify downloads (encrypted by librespot), and the last 2 GB of songs streamed from the Stream node |
| `Application Support/network.aza.stream/Downloads` | Songs from the Stream node you downloaded |
| Keychain | Spotify and Last.fm credentials, this device only |
| Mac music folders | Your files, read in place and never copied or changed |

## Code

```
Shared/        SwiftUI for both apps: AppModel, views, audio output, Now Playing, artwork
iOS/, macOS/   Each app's entry point and platform pieces (menu bar, folder watching)
core/          The Rust core: Spotify (librespot), decoding, queue, SQLite library, lyrics, Last.fm, the Stream node
node/          The Stream node, at stream.aza.network: your music files, behind an aza sign-in
Generated/     StreamCore.xcframework and its UniFFI bindings, built by scripts/build-core.sh
tests/         End-to-end UI tests, a fake Spotify, and a small fixture library
scripts/       build-core, deploy, testflight, test, demo
```

The UniFFI bindings compile as their own module, `StreamBindings`, without main-actor isolation, so calls into the core don't land on the main thread. The app is Swift 6 with the main actor as the default. Sign-in, the profile button and Appearance come from [AzaUI](https://github.com/plarza/ui). Decisions and scope are in [SPEC.md](SPEC.md).

## Develop

Xcode 27, Rust, `brew install xcodegen cmake`, and [uv](https://docs.astral.sh/uv/) for the screenshot script.

| Task | Command |
|---|---|
| Build the Rust core | `scripts/build-core.sh` |
| Open in Xcode | `xcodegen generate && open Stream.xcodeproj` |
| Install on this Mac and the iPhone | `scripts/deploy.sh` (`DEVICE=<udid>` for another iPhone) |
| Upload to TestFlight | `scripts/testflight.sh [build]` |
| UI tests in the Simulator | `scripts/test.sh` |
| Refresh these screenshots | `python3 -I scripts/demo.py --screenshots` |

Spotify sign-in needs your own app at [developer.spotify.com](https://developer.spotify.com/dashboard) with the redirect URI `http://127.0.0.1:8989/login`; paste its Client ID in Profile.

The UI tests build their own debug core for the Simulator, run against a fake Spotify and the fixture library, and launch the app once, returning to Home between tests with `network-aza-stream://reset`.

| Launch with | Opens |
|---|---|
| `-musicFolder <path>` | That folder as the local library |
| `-uiTesting YES` | Isolated data, silent output, nothing saved |
| `-spotifyAccounts <url>`, `-spotifyApi <url>` | A fake Spotify for tests |
| `-signInAnswered YES`, `SAMPLE_ACCOUNT=1` | Past the sign-in screen; signed in to the sample profile |
