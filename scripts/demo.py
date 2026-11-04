"""README screenshots on a clean simulator, plus one of the Mac app.

  python3 -I scripts/demo.py --screenshots                          docs/screenshots/*.png
  python3 -I scripts/demo.py --screenshots --cast NAME --out DIR    another cast, somewhere else
  python3 -I scripts/demo.py --screenshots --only lyrics,album      just those shots
  python3 -I scripts/demo.py --screenshots --app-args "-key value"  with extra launch arguments

Each shot is one launch: deep links set the screen up (pins, what's playing, the page,
the lyric line), the app writes tmp/links-done when they're all done, and the shot is taken
then. No UI test, no tapping, no fixed waits.

The library they show is built from scripts/demo-library.json: real albums with their
covers and track lengths, silent audio, and synced lyrics from LRCLIB. A cast there picks
which albums and artists go where, and which lyric is lit; the first is the README's. A cast
with a "base" takes the rest from that one.
"""
import json, os, re, subprocess, sys, time, urllib.error, urllib.parse, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUNDLE = "network.aza.stream"
DATA = os.path.join(ROOT, "scripts", "demo-library.json")
LIBRARY = os.path.join(ROOT, "build", "demo", "Music")
OUT = os.path.join(ROOT, "docs", "screenshots")


def arg(flag, default=None):
    return sys.argv[sys.argv.index(flag) + 1] if flag in sys.argv else default


def run(*cmd, check=True, env=None):
    done = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env={**os.environ, **(env or {})})
    if check and done.returncode != 0:
        sys.exit(f"{' '.join(cmd[:2])} failed:\n" + "\n".join((done.stdout + done.stderr).splitlines()[-25:]))
    return done


def simctl(*args, check=True, env=None):
    return run("xcrun", "simctl", *args, check=check, env=env)


def simulator():
    """$SIM, or the "Stream Screenshots" iPhone on the newest iOS, made when it's missing."""
    if os.environ.get("SIM"):
        return os.environ["SIM"]
    ios = [r for r in json.loads(simctl("list", "-j", "runtimes").stdout)["runtimes"]
           if r["platform"] == "iOS" and r["isAvailable"]]
    runtime = max(ios, key=lambda r: [int(n) for n in r["version"].split(".")])["identifier"]
    for d in json.loads(simctl("list", "-j", "devices").stdout)["devices"].get(runtime, []):
        if d["name"] == "Stream Screenshots":
            return d["udid"]
    phones = json.loads(simctl("list", "-j", "devicetypes").stdout)["devicetypes"]
    pro = [t["identifier"] for t in phones if re.fullmatch(r"iPhone \d+ Pro", t["name"])]
    return simctl("create", "Stream Screenshots", sorted(pro)[-1], runtime).stdout.strip()


SIM = simulator()
DEST = f"platform=iOS Simulator,id={SIM}"


def local_ids(names):
    """The core's local ids (core/src/db.rs): blake3 of the artist key, then the album, lowercased.
    names: [artist] or [artist, album]."""
    code = ("import sys, json, blake3\n"
            "out = []\n"
            "for parts in json.loads(sys.argv[1]):\n"
            "    h = blake3.blake3()\n"
            "    for p in parts:\n"
            "        h.update(p.lower().strip().encode()); h.update(b'\\0')\n"
            "    out.append(('local:album:' if len(parts) == 2 else 'local:artist:') + h.hexdigest()[:16])\n"
            "print(json.dumps(out))\n")
    key = lambda a: "".join(c for c in a if c.isalnum()).lower() or a.strip().lower()
    parts = [[key(n[0]), *n[1:]] for n in names]
    return json.loads(run("uv", "run", "--quiet", "--with", "blake3", "python", "-I", "-c", code, json.dumps(parts)).stdout)


def fetch(url):
    req = urllib.request.Request(url, headers={"User-Agent": "Stream screenshots (network.aza.stream)"})
    for attempt in range(4):
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code < 500 or attempt == 3:
                raise
            time.sleep(2 ** attempt)


def name(s):
    """A file or folder name: the tags keep the real title, slashes and all."""
    return s.replace("/", "∕")


def library(cast):
    """Writes the albums in demo-library.json as silent FLACs at their real lengths, with
    covers and a synced .lrc for the cast's lyrics shot, plus demo.json naming what goes where."""
    data = json.load(open(DATA))
    stamp = os.path.join(LIBRARY, ".albums")
    albums = json.dumps(data["albums"])
    fresh = os.path.isfile(stamp) and open(stamp).read() == albums
    if not fresh and os.path.isdir(LIBRARY):
        subprocess.run(["rm", "-rf", LIBRARY], check=True)
    os.makedirs(LIBRARY, exist_ok=True)
    if not fresh:
        print(f"library: {len(data['albums'])} albums…", flush=True)
    for a in [] if fresh else data["albums"]:
        folder = os.path.join(LIBRARY, name(f"{a['artist']} - {a['title']}"))
        os.makedirs(folder)
        open(os.path.join(folder, "cover.jpg"), "wb").write(fetch(a["cover"]))
        for t in a["tracks"]:
            m, sec = t["length"].split(":")
            artist = a["artist"] + "".join(f" feat. {f}" if i == 0 else f" & {f}" for i, f in enumerate(t["feat"]))
            path = os.path.join(folder, name(f"{t['disc']}-{t['n']:02d} {t['title']}.flac"))
            run("ffmpeg", "-loglevel", "error", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo",
                "-t", str(int(m) * 60 + int(sec)), "-c:a", "flac", "-sample_fmt", "s16",
                "-metadata", f"title={t['title']}", "-metadata", f"artist={artist}",
                "-metadata", f"album={a['title']}", "-metadata", f"album_artist={a['artist']}",
                "-metadata", f"track={t['n']}", "-metadata", f"disc={t['disc']}",
                *(["-metadata", f"date={a['year']}"] if "year" in a else []), path)

    if not fresh:
        open(stamp, "w").write(albums)

    # A shot names an album by its title, or an artist by name.
    artists = sorted({a["artist"] for a in data["albums"]})
    names = [[a["artist"], a["title"]] for a in data["albums"]] + [[a] for a in artists]
    ids = dict(zip([n[-1] for n in names], local_ids(names)))
    pick = lambda title: {"id": ids[title], "title": title}
    casts = data["casts"]
    shots = casts[cast or next(iter(casts))]
    if "base" in shots:
        shots = {**casts[shots["base"]], **shots}
    lyrics_album = next(a for a in data["albums"] if a["title"] == shots["lyrics"][0])
    track = next(t for t in lyrics_album["tracks"] if t["title"] == shots["lyrics"][1])
    q = urllib.parse.urlencode({"artist_name": lyrics_album["artist"], "track_name": track["title"]}, quote_via=urllib.parse.quote)
    lrc = json.loads(fetch("https://lrclib.net/api/get?" + q))["syncedLyrics"]
    open(os.path.join(LIBRARY, name(f"{lyrics_album['artist']} - {lyrics_album['title']}"),
                      name(f"{track['disc']}-{track['n']:02d} {track['title']}.lrc")), "w").write(lrc)
    # The shot's line: where it starts, less the lyrics view's 250 ms lead, so it's the one lit.
    line = shots["lyrics"][2]
    start = next(int(m) * 60000 + round(float(sec) * 1000)
                 for m, sec, text in re.findall(r"\[(\d+):([\d.]+)\](.*)", lrc) if text.strip() == line)

    json.dump({"pins": [pick(t) for t in shots["pins"]], "recents": [pick(t) for t in shots["recents"]],
               "album": pick(shots["album"]), "playing": pick(shots["playing"]),
               "nowPlaying": pick(shots.get("nowPlaying", shots["playing"])),
               "macPlaying": pick(shots.get("macPlaying", shots["playing"])),
               "lyrics": pick(lyrics_album["title"]), "lyricsTrack": track["title"], "lyricsMs": max(0, start - 150),
               "search": shots["search"], "result": shots["result"]},
              open(os.path.join(os.path.dirname(LIBRARY), "demo.json"), "w"), indent=2)


def changed(stamp, *paths):
    """True when the stamp is missing or anything under the paths is newer."""
    if not os.path.isfile(stamp):
        return True
    t = os.path.getmtime(stamp)
    for p in paths:
        for d, _, files in os.walk(os.path.join(ROOT, p)):
            if any(os.path.getmtime(os.path.join(d, f)) > t for f in files):
                return True
    return False


def prepare(mac=True):
    """Builds the core and the apps when their sources changed, and gives the simulator a tidy status bar."""
    core_stamp = os.path.join(ROOT, "build", ".demo-core")
    if not os.path.isdir(os.path.join(ROOT, "Generated", "StreamCore.xcframework")) or changed(core_stamp, "core/src", "core/Cargo.toml"):
        print("building the core…", flush=True)
        run("taskpolicy", "-c", "utility", "scripts/build-core.sh", env={"PROFILE": "debug"})
        open(core_stamp, "w").close()
    sources = ("project.yml", "Shared", "iOS", "macOS", "Generated")
    stamp = os.path.join(ROOT, "build", ".demo-ios")
    if changed(stamp, *sources):
        print("building…", flush=True)
        run("xcodegen", "generate")
        run("taskpolicy", "-c", "utility", "xcodebuild", "-project", "Stream.xcodeproj", "-scheme", "Stream-iOS",
            "-destination", DEST, "-derivedDataPath", "build", "build")
        open(stamp, "w").close()
    mac_stamp = os.path.join(ROOT, "build", ".demo-mac")
    if mac and changed(mac_stamp, *sources):
        run("taskpolicy", "-c", "utility", "xcodebuild", "-project", "Stream.xcodeproj", "-scheme", "Stream-macOS",
            "-destination", "platform=macOS", "-derivedDataPath", "build", "build")
        open(mac_stamp, "w").close()
    simctl("bootstatus", SIM, "-b")
    simctl("install", SIM, os.path.join(ROOT, "build/Build/Products/Debug-iphonesimulator/Stream.app"))
    simctl("status_bar", SIM, "override", "--time", "9:41", "--batteryState", "charged", "--batteryLevel", "100",
           "--wifiBars", "3", "--cellularBars", "4")
    simctl("ui", SIM, "appearance", "dark", check=False)


def wait_for(path, what, secs=30):
    end = time.time() + secs
    while not os.path.exists(path):
        if time.time() > end:
            sys.exit(f"{what}: the app never finished its deep links")
        time.sleep(0.05)


def frame(path, phone=True):
    """Phone: crops off the status bar and rounds the corners like the screen. Mac: halves the size."""
    script = (
        "import sys\n"
        "from PIL import Image, ImageDraw\n"
        "im = Image.open(sys.argv[1]).convert('RGBA')\n"
        "w, h = im.size\n"
        "if sys.argv[2] == 'phone':\n"
        "    im = im.crop((0, round(h * 0.062), w, h))\n"
        "    mask = Image.new('L', im.size, 0)\n"
        "    ImageDraw.Draw(mask).rounded_rectangle((0, 0, im.width - 1, im.height - 1), radius=round(w * 0.12), fill=255)\n"
        "    im.putalpha(mask)\n"
        "im.resize((im.width // 2, im.height // 2), Image.LANCZOS).save(sys.argv[1])\n"
    )
    run("uv", "run", "--quiet", "--with", "pillow", "python", "-I", "-c", script, path, "phone" if phone else "mac")


def mac_window(out):
    """Launches the Mac app in the background on the demo library and captures its window, without taking focus."""
    # The real path: build is a symlink to build.noindex, and pgrep sees the resolved one.
    app = os.path.realpath(os.path.join(ROOT, "build/Build/Products/Debug/Stream.app"))
    subprocess.run(["pkill", "-f", app + "/Contents/MacOS/Stream"], check=False)
    demo = json.load(open(os.path.join(os.path.dirname(LIBRARY), "demo.json")))
    done = os.path.join(subprocess.run(["getconf", "DARWIN_USER_TEMP_DIR"], capture_output=True, text=True).stdout.strip(), "links-done")
    if os.path.exists(done):
        os.remove(done)
    subprocess.run(["open", "-g", "-n", "--env", "SAMPLE_ACCOUNT=1", app, "--args", "-musicFolder", LIBRARY, "-uiTesting", "YES",
                    "-signInAnswered", "YES",
                    "-url", "network-aza-stream://play?id=" + demo["macPlaying"]["id"],
                    "-url", "network-aza-stream://open?id=" + demo["album"]["id"]], check=True)
    wait_for(done, "mac")
    pid = subprocess.run(["pgrep", "-n", "-f", app + "/Contents/MacOS/Stream"], capture_output=True, text=True).stdout.split()[0]
    find = ("import CoreGraphics\n"
            "let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]\n"
            f"for w in list where (w[kCGWindowOwnerPID as String] as? Int) == {pid} && (w[kCGWindowLayer as String] as? Int) == 0 && (w[kCGWindowIsOnscreen as String] as? Bool) == true {{\n"
            "  if let b = w[kCGWindowBounds as String] as? [String: Any], let h = b[\"Height\"] as? Double, h > 200 { print(w[kCGWindowNumber as String]!); break }\n"
            "}\n")
    wid = subprocess.run(["swift", "-e", find], capture_output=True, text=True).stdout.strip()
    path = os.path.join(out, "mac.png")
    shot = subprocess.run(["screencapture", "-x", "-o", f"-l{wid}", path])
    subprocess.run(["pkill", "-f", app + "/Contents/MacOS/Stream"], check=False)
    if shot.returncode != 0:
        # macOS refuses window captures while the screen is locked or asleep.
        print("skipped mac: unlock the screen and run again", flush=True)
        return
    frame(path, phone=False)
    print("shot mac", flush=True)



def links(demo):
    """The deep links that set up each README shot, from a fresh launch."""
    play = lambda item, **q: "play?" + urllib.parse.urlencode({"id": item["id"], **q}, quote_via=urllib.parse.quote)
    return {
        # Pins, then Recents oldest first (each play goes to the top), then what's playing.
        "home": [f"pin?id={p['id']}" for p in demo["pins"]] + [play(a) for a in demo["recents"]] + [play(demo["playing"])],
        "now-playing": [play(demo["nowPlaying"]), "nowplaying", "seek?ms=7000"],
        # On the shot's line, lit and scrolled into place.
        "lyrics": [play(demo["lyrics"], track=demo["lyricsTrack"]), "nowplaying?mode=lyrics", f"seek?ms={demo['lyricsMs']}"],
        "album": [play(demo["album"]), f"open?id={demo['album']['id']}"],
        "search": ["search?" + urllib.parse.urlencode({"q": demo["search"]}, quote_via=urllib.parse.quote)],
    }


def shoot(name, urls, out):
    """One launch of the installed app, its deep links, one screenshot."""
    done = os.path.join(simctl("get_app_container", SIM, BUNDLE, "data").stdout.strip(), "tmp", "links-done")
    if os.path.exists(done):
        os.remove(done)
    # --app-args "-key value …" goes to the app as well (e.g. to try a setting).
    args = ["-musicFolder", LIBRARY, "-uiTesting", "YES", "-signInAnswered", "YES", *(arg("--app-args") or "").split()]
    for u in urls:
        args += ["-url", "network-aza-stream://" + u]
    simctl("launch", "--terminate-running-process", SIM, BUNDLE, *args, env={"SIMCTL_CHILD_SAMPLE_ACCOUNT": "1"})
    wait_for(done, name)
    path = os.path.join(out, name + ".png")
    simctl("io", SIM, "screenshot", path)
    frame(path)
    print("shot", name, flush=True)


def screenshots(cast, out, only=None):
    library(cast)
    prepare(mac=only is None or "mac" in only)
    os.makedirs(out, exist_ok=True)
    demo = json.load(open(os.path.join(os.path.dirname(LIBRARY), "demo.json")))
    for name, urls in links(demo).items():
        if only is None or name in only:
            shoot(name, urls, out)
    simctl("terminate", SIM, BUNDLE, check=False)
    simctl("status_bar", SIM, "clear", check=False)
    if only is None or "mac" in only:
        mac_window(out)


if __name__ == "__main__":
    if "--screenshots" in sys.argv:
        only = arg("--only")
        screenshots(arg("--cast"), os.path.abspath(arg("--out", OUT)), only.split(",") if only else None)
    else:
        sys.exit(__doc__)
