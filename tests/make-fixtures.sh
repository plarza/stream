#!/bin/zsh
# Builds the small library the UI tests play: three albums in three formats,
# a featured artist, folder artwork and a synced .lrc. Quiet 6-second tones.
set -euo pipefail
cd "$(dirname "$0")"
OUT=fixtures/Music
rm -rf $OUT && mkdir -p $OUT
tone() { # file freq title artist album albumartist track
  ffmpeg -loglevel error -f lavfi -i "sine=frequency=${2}:duration=6" -af "volume=-30dB" -ac 2 -ar 44100 \
    -metadata title="$3" -metadata artist="$4" -metadata album="$5" -metadata album_artist="$6" -metadata track="$7" \
    "${@:8}" "$1"
}
cover() { ffmpeg -loglevel error -f lavfi -i "gradients=s=600x600:c0=${2}:c1=${3}:x0=0:y0=0:x1=600:y1=600:d=1" -frames:v 1 "$1"; }

D="$OUT/Neon Coast - Night Drive"; mkdir -p "$D"; cover "$D/cover.jpg" 0xFF3B5C 0x2A0A4A
tone "$D/01 Neon Lights.flac" 220 "Neon Lights" "Neon Coast" "Night Drive" "Neon Coast" 1
tone "$D/02 Tunnel.flac" 247 "Tunnel" "Neon Coast" "Night Drive" "Neon Coast" 2
tone "$D/03 Overpass.flac" 262 "Overpass" "Neon Coast" "Night Drive" "Neon Coast" 3
tone "$D/04 Last Exit.flac" 294 "Last Exit" "Neon Coast" "Night Drive" "Neon Coast" 4

D="$OUT/Ada Field - Morning Light"; mkdir -p "$D"; cover "$D/cover.jpg" 0xFFC371 0x2E6BD8
tone "$D/01 Sunrise.mp3" 330 "Sunrise" "Ada Field" "Morning Light" "Ada Field" 1 -b:a 192k
tone "$D/02 Dew.mp3" 349 "Dew" "Ada Field feat. Juno" "Morning Light" "" 2 -b:a 192k
tone "$D/03 Kettle.mp3" 392 "Kettle" "Ada Field" "Morning Light" "Ada Field" 3 -b:a 192k

D="$OUT/Singles"; mkdir -p "$D"; cover "$D/cover.jpg" 0x43CEA2 0x185A9D
tone "$D/Loose Song.opus" 440 "Loose Song" "Juno & Kai" "Singles" "" 1 -c:a libopus -b:a 128k -ar 48000
cat > "$D/Loose Song.lrc" <<'LRC'
[00:00.00]First line of the loose song
[00:02.00]Second line comes in
[00:04.00]Third line to finish
LRC
du -sh fixtures
