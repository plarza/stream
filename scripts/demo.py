"""README screenshots on a clean simulator (taps come from tests/iOSUITests/DemoFlow.swift), plus one of the Mac app.

  python3 -I scripts/demo.py --screenshots   docs/screenshots/*.png
"""
import os, pty, re, subprocess, sys, tempfile, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SIM = os.environ.get("SIM", "4C675D57-6CF7-4DDA-ACF4-4075857B2010")
BUNDLE = "network.aza.music"
DEST = f"platform=iOS Simulator,id={SIM}"
FIXTURES = os.path.join(ROOT, "tests", "fixtures", "Music")
OUT = os.path.join(ROOT, "docs", "screenshots")


def run(*cmd, check=True, env=None):
    return subprocess.run(cmd, cwd=ROOT, check=check, capture_output=True, text=True,
                          env={**os.environ, **(env or {})})


def simctl(*args, check=True):
    return run("xcrun", "simctl", *args, check=check)


def prepare():
    """Builds the app and its tests, and gives the simulator a tidy status bar."""
    print("building…", flush=True)
    if not os.path.isdir(os.path.join(ROOT, "Generated", "MusicCore.xcframework")):
        run("scripts/build-core.sh", env={"PROFILE": "debug"})
    run("xcodegen", "generate")
    run("xcodebuild", "-project", "Music.xcodeproj", "-scheme", "Music-iOS", "-destination", DEST,
        "-derivedDataPath", "build", "build-for-testing")
    run("xcodebuild", "-project", "Music.xcodeproj", "-scheme", "Music-macOS", "-destination", "platform=macOS",
        "-derivedDataPath", "build", "build")
    simctl("bootstatus", SIM, "-b")
    simctl("status_bar", SIM, "override", "--time", "9:41", "--batteryState", "charged", "--batteryLevel", "100",
           "--wifiBars", "3", "--cellularBars", "4")
    simctl("ui", SIM, "appearance", "dark", check=False)


def run_flow(method, on_line):
    """Runs one DemoFlow method, calling on_line for each output line as it happens."""
    # A pseudo-terminal keeps xcodebuild line-buffered, so marks arrive when they happen.
    master, slave = pty.openpty()
    test = subprocess.Popen(["xcodebuild", "test-without-building", "-project", "Music.xcodeproj", "-scheme", "Music-iOS",
                             "-destination", DEST, "-derivedDataPath", "build",
                             f"-only-testing:MusicUITests-iOS/DemoFlow/{method}"],
                            cwd=ROOT, stdout=slave, stderr=slave, text=True)
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
        path = os.path.join(tempfile.mkdtemp(prefix="music-demo-"), "flow.log")
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
    """Launches the Mac app in the background on the fixtures and captures its window, without taking focus."""
    app = os.path.join(ROOT, "build/Build/Products/Debug/Music.app")
    subprocess.run(["pkill", "-f", app + "/Contents/MacOS/Music"], check=False)
    album = "network-aza-music://open?id=" + mac_album_id()
    subprocess.run(["open", "-g", "-n", app, "--args", "-musicFolder", FIXTURES, "-uiTesting", "YES",
                    "-url", "network-aza-music://play?id=" + mac_album_id(), "-url", album], check=True)
    time.sleep(8)
    pid = subprocess.run(["pgrep", "-n", "-f", app + "/Contents/MacOS/Music"], capture_output=True, text=True).stdout.split()[0]
    find = ("import CoreGraphics\n"
            "let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]\n"
            f"for w in list where (w[kCGWindowOwnerPID as String] as? Int) == {pid} && (w[kCGWindowLayer as String] as? Int) == 0 && (w[kCGWindowIsOnscreen as String] as? Bool) == true {{\n"
            "  if let b = w[kCGWindowBounds as String] as? [String: Any], let h = b[\"Height\"] as? Double, h > 200 { print(w[kCGWindowNumber as String]!); break }\n"
            "}\n")
    wid = subprocess.run(["swift", "-e", find], capture_output=True, text=True).stdout.strip()
    path = os.path.join(OUT, "mac.png")
    shot = subprocess.run(["screencapture", "-x", "-o", f"-l{wid}", path])
    subprocess.run(["pkill", "-f", app + "/Contents/MacOS/Music"], check=False)
    if shot.returncode != 0:
        # macOS refuses window captures while the screen is locked or asleep.
        print("skipped mac: unlock the screen and run again", flush=True)
        return
    frame(path, phone=False)
    print("shot mac", flush=True)


def mac_album_id():
    """The fixtures' Night Drive album, by the same rule the core uses (blake3 of artist + album)."""
    code = ("import sys, blake3\n"
            "h = blake3.blake3()\n"
            "for p in ['neon coast', 'night drive']:\n"
            "    h.update(p.encode()); h.update(b'\\0')\n"
            "print('local:album:' + h.hexdigest()[:16])\n")
    return run("uv", "run", "--quiet", "--with", "blake3", "python", "-I", "-c", code).stdout.strip()


def screenshots():
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
