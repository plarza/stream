"""README screenshots on a clean simulator (taps come from tests/iOSUITests/DemoFlow.swift), plus one of the Mac app.

  python3 -I scripts/demo.py --screenshots   docs/screenshots/*.png

The library they show is built from scripts/demo-library.json: real albums with their
covers and track lengths, silent audio, and synced lyrics from LRCLIB.
"""
import json, os, pty, re, subprocess, sys, tempfile, time, urllib.parse, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SIM = os.environ.get("SIM", "4C675D57-6CF7-4DDA-ACF4-4075857B2010")
BUNDLE = "network.aza.stream"
DEST = f"platform=iOS Simulator,id={SIM}"
DATA = os.path.join(ROOT, "scripts", "demo-library.json")
LIBRARY = os.path.join(ROOT, "build", "demo", "Music")
OUT = os.path.join(ROOT, "docs", "screenshots")


def run(*cmd, check=True, env=None):
    return subprocess.run(cmd, cwd=ROOT, check=check, capture_output=True, text=True,
                          env={**os.environ, **(env or {})})


def simctl(*args, check=True):
    return run("xcrun", "simctl", *args, check=check)


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
    with urllib.request.urlopen(req, timeout=30) as r:
        return r.read()


def name(s):
    """A file or folder name: the tags keep the real title, slashes and all."""
    return s.replace("/", "∕")


def library():
    """Writes the albums in demo-library.json as silent FLACs at their real lengths, with
    covers and a synced .lrc for the lyrics shot, plus demo.json for DemoFlow."""
    data = json.load(open(DATA))
    if os.path.isdir(LIBRARY):
        subprocess.run(["rm", "-rf", LIBRARY], check=True)
    os.makedirs(LIBRARY)
    print(f"library: {len(data['albums'])} albums…", flush=True)
    for a in data["albums"]:
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

    # A shot names an album by its title, or an artist by name.
    artists = sorted({a["artist"] for a in data["albums"]})
    names = [[a["artist"], a["title"]] for a in data["albums"]] + [[a] for a in artists]
    ids = dict(zip([n[-1] for n in names], local_ids(names)))
    pick = lambda title: {"id": ids[title], "title": title}
    shots = data["shots"]
    lyrics_album = next(a for a in data["albums"] if a["title"] == shots["lyrics"][0])
    track = next(t for t in lyrics_album["tracks"] if t["title"] == shots["lyrics"][1])
    q = urllib.parse.urlencode({"artist_name": lyrics_album["artist"], "track_name": track["title"]})
    lrc = json.loads(fetch("https://lrclib.net/api/get?" + q))["syncedLyrics"]
    open(os.path.join(LIBRARY, name(f"{lyrics_album['artist']} - {lyrics_album['title']}"),
                      name(f"{track['disc']}-{track['n']:02d} {track['title']}.lrc")), "w").write(lrc)

    json.dump({"pins": [pick(t) for t in shots["pins"]], "recents": [pick(t) for t in shots["recents"]],
               "album": pick(shots["album"]),
               "playing": pick(shots["playing"]), "lyrics": pick(lyrics_album["title"]),
               "lyricsTrack": track["title"], "search": shots["search"], "result": shots["result"]},
              open(os.path.join(os.path.dirname(LIBRARY), "demo.json"), "w"), indent=2)


def prepare():
    """Builds the app and its tests, and gives the simulator a tidy status bar."""
    print("building…", flush=True)
    if not os.path.isdir(os.path.join(ROOT, "Generated", "StreamCore.xcframework")):
        run("scripts/build-core.sh", env={"PROFILE": "debug"})
    run("xcodegen", "generate")
    run("xcodebuild", "-project", "Stream.xcodeproj", "-scheme", "Stream-iOS", "-destination", DEST,
        "-derivedDataPath", "build", "build-for-testing")
    run("xcodebuild", "-project", "Stream.xcodeproj", "-scheme", "Stream-macOS", "-destination", "platform=macOS",
        "-derivedDataPath", "build", "build")
    simctl("bootstatus", SIM, "-b")
    simctl("status_bar", SIM, "override", "--time", "9:41", "--batteryState", "charged", "--batteryLevel", "100",
           "--wifiBars", "3", "--cellularBars", "4")
    simctl("ui", SIM, "appearance", "dark", check=False)


def run_flow(method, on_line):
    """Runs one DemoFlow method, calling on_line for each output line as it happens."""
    # A pseudo-terminal keeps xcodebuild line-buffered, so marks arrive when they happen.
    master, slave = pty.openpty()
    # TEST_RUNNER_ variables reach the test runner without the prefix.
    test = subprocess.Popen(["xcodebuild", "test-without-building", "-project", "Stream.xcodeproj", "-scheme", "Stream-iOS",
                             "-destination", DEST, "-derivedDataPath", "build",
                             f"-only-testing:StreamUITests-iOS/DemoFlow/{method}"],
                            cwd=ROOT, stdout=slave, stderr=slave, text=True,
                            env={**os.environ, "TEST_RUNNER_STREAM_LIBRARY": LIBRARY})
    os.close(slave)
    log, buf = [], b""
    while True:
        try:
            chunk = os.read(master, 4096)
        except OSError:
            break
        if not chunk:
            break
        buf += chunk
        *lines, buf = buf.split(b"\n")
        for raw_line in lines:
            line = raw_line.decode("utf-8", "replace")
            log.append(line + "\n")
            on_line(line)
    test.wait()
    if test.returncode != 0:
        path = os.path.join(tempfile.mkdtemp(prefix="stream-demo-"), "flow.log")
        open(path, "w").writelines(log)
        sys.exit(f"{method} failed, log in {path}")


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


def mac_window():
    """Launches the Mac app in the background on the demo library and captures its window, without taking focus."""
    app = os.path.join(ROOT, "build/Build/Products/Debug/Stream.app")
    subprocess.run(["pkill", "-f", app + "/Contents/MacOS/Stream"], check=False)
    demo = json.load(open(os.path.join(os.path.dirname(LIBRARY), "demo.json")))
    subprocess.run(["open", "-g", "-n", app, "--args", "-musicFolder", LIBRARY, "-uiTesting", "YES",
                    "-url", "network-aza-stream://play?id=" + demo["playing"]["id"],
                    "-url", "network-aza-stream://open?id=" + demo["album"]["id"]], check=True)
    time.sleep(8)
    pid = subprocess.run(["pgrep", "-n", "-f", app + "/Contents/MacOS/Stream"], capture_output=True, text=True).stdout.split()[0]
    find = ("import CoreGraphics\n"
            "let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]\n"
            f"for w in list where (w[kCGWindowOwnerPID as String] as? Int) == {pid} && (w[kCGWindowLayer as String] as? Int) == 0 && (w[kCGWindowIsOnscreen as String] as? Bool) == true {{\n"
            "  if let b = w[kCGWindowBounds as String] as? [String: Any], let h = b[\"Height\"] as? Double, h > 200 { print(w[kCGWindowNumber as String]!); break }\n"
            "}\n")
    wid = subprocess.run(["swift", "-e", find], capture_output=True, text=True).stdout.strip()
    path = os.path.join(OUT, "mac.png")
    shot = subprocess.run(["screencapture", "-x", "-o", f"-l{wid}", path])
    subprocess.run(["pkill", "-f", app + "/Contents/MacOS/Stream"], check=False)
    if shot.returncode != 0:
        # macOS refuses window captures while the screen is locked or asleep.
        print("skipped mac: unlock the screen and run again", flush=True)
        return
    frame(path, phone=False)
    print("shot mac", flush=True)



def screenshots():
    library()
    prepare()
    os.makedirs(OUT, exist_ok=True)

    def on_line(line):
        m = re.search(r"SHOT_([\w-]+)", line)
        if m:
            time.sleep(0.8)  # the flow holds still for 2 s after each mark
            path = os.path.join(OUT, m.group(1) + ".png")
            simctl("io", SIM, "screenshot", path)
            frame(path)
            print("shot", m.group(1), flush=True)

    run_flow("testScreenshots", on_line)
    simctl("status_bar", SIM, "clear", check=False)
    mac_window()


if __name__ == "__main__":
    if "--screenshots" in sys.argv:
        screenshots()
    else:
        sys.exit(__doc__)
