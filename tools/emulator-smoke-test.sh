#!/usr/bin/env bash
set -e

echo "=== YR Music Android Emulator Verification ==="

adb wait-for-device
while [ "$(adb shell getprop sys.boot_completed | tr -d '\r')" != "1" ]; do
    echo "Waiting for sys.boot_completed..."
    sleep 3
done

APK_FILE=$(find apk-to-test -type f -name "yr_app.apk" 2>/dev/null | head -n 1)
if [ -z "$APK_FILE" ]; then
    APK_FILE=$(find apk-to-test -type f -name "*.apk" 2>/dev/null | head -n 1)
fi
if [ -z "$APK_FILE" ]; then
    echo "ERROR: APK not found in apk-to-test/"
    exit 1
fi
echo "Found APK: $APK_FILE"

adb install -r -d -t "$APK_FILE"
mkdir -p screenshots

adb logcat -c
START_RES=$(adb shell am start -W -S -n com.yr.music/android.app.NativeActivity)
echo "$START_RES"
# Allow home feed & album thumbnails to load over network
sleep 9

PID=$(adb shell pidof com.yr.music | tr -d '\r' || true)
if [ -z "$PID" ]; then
    echo "ERROR: com.yr.music crashed on startup!"
    adb logcat -d > emulator-logcat.log
    cat emulator-logcat.log | tail -n 80
    exit 1
fi
echo "YR Music running with PID: $PID"

# 1. Home tab (with moods, quick picks, new albums)
adb shell screencap -p /data/local/tmp/01_home.png
adb pull /data/local/tmp/01_home.png screenshots/01_home.png

# 2. Search tab (second column at bottom nav)
adb shell input tap 405 2160
sleep 2
# Tap search box, type query, tap Buscar button
adb shell input tap 350 245
sleep 1
adb shell input text "daft%spunk"
sleep 1
adb shell input tap 960 245
sleep 5
adb shell screencap -p /data/local/tmp/02_search.png
adb pull /data/local/tmp/02_search.png screenshots/02_search.png

# 3. Open Settings (top right gear), trigger local audio demo (2 tracks)
adb shell input tap 1000 135
sleep 1
adb shell input tap 300 1320
sleep 4
adb shell screencap -p /data/local/tmp/03_local_playing.png
adb pull /data/local/tmp/03_local_playing.png screenshots/03_local_playing.png

# Verify ForegroundService, Notification, and MediaSession
adb shell dumpsys notification --noredact | grep -A 25 "com.yr.music" > screenshots/dumpsys_notif.txt || true
adb shell dumpsys media_session | grep -A 25 "YrMusicSession" > screenshots/dumpsys_media.txt || true
adb shell dumpsys activity services com.yr.music > screenshots/dumpsys_service.txt || true
cat screenshots/dumpsys_service.txt

# 4. Seek on progress bar (swipe slider) & Pause
adb shell input swipe 180 1490 700 1490 300
sleep 1
adb shell input tap 540 1700
sleep 1
adb shell screencap -p /data/local/tmp/04_seek_pause.png
adb pull /data/local/tmp/04_seek_pause.png screenshots/04_seek_pause.png

# 5. Next track (⏭)
adb shell input tap 755 1700
sleep 2
adb shell screencap -p /data/local/tmp/05_next_track.png
adb pull /data/local/tmp/05_next_track.png screenshots/05_next_track.png

# 5b. Swipe gesture right on cover art (Prev -> back to Pista 1)
adb shell input swipe 200 750 850 750 250
sleep 2
adb shell screencap -p /data/local/tmp/05b_swipe_prev.png
adb pull /data/local/tmp/05b_swipe_prev.png screenshots/05b_swipe_prev.png

# 6. Open Queue tab (Cola) & Letra tab
adb shell input tap 556 266
sleep 1
adb shell screencap -p /data/local/tmp/06_queue_editable.png
adb pull /data/local/tmp/06_queue_editable.png screenshots/06_queue_editable.png

adb shell input tap 404 266
sleep 2
adb shell screencap -p /data/local/tmp/06b_lyrics.png
adb pull /data/local/tmp/06b_lyrics.png screenshots/06b_lyrics.png

# 7. Minimize Now Playing (v) to show Mini-Player with progress bar
adb shell input tap 68 266
sleep 1
adb shell screencap -p /data/local/tmp/07_mini_player.png
adb pull /data/local/tmp/07_mini_player.png screenshots/07_mini_player.png

# 8. Screen off: verify background service survives AND media_session next/pause work with screen OFF
adb shell input keyevent 26
sleep 3
adb shell dumpsys activity services com.yr.music | grep -q "YrPlaybackService" && echo "YrPlaybackService alive with screen off!"
{
    echo "=== BEFORE media_session dispatch (screen OFF) ==="
    adb shell dumpsys media_session | grep -A 25 "YrMusicSession" || true
    echo "=== Dispatching NEXT via media_session (screen OFF) ==="
    adb shell cmd media_session dispatch next || true
    sleep 2
    adb shell dumpsys media_session | grep -A 25 "YrMusicSession" || true
    echo "=== Dispatching PAUSE via media_session (screen OFF) ==="
    adb shell cmd media_session dispatch pause || true
    sleep 2
    adb shell dumpsys media_session | grep -A 25 "YrMusicSession" || true
    echo "=== Dispatching Notification PendingIntent ACTION_NEXT (screen OFF) ==="
    adb shell am start-foreground-service -n com.yr.music/.YrPlaybackService -a com.yr.music.ACTION_NEXT || true
    sleep 2
    adb shell dumpsys media_session | grep -A 25 "YrMusicSession" || true
    echo "=== YrPlaybackService logcat ==="
    adb logcat -d -s YrPlaybackService:I || true
} > screenshots/dumpsys_media_screen_off.txt
cat screenshots/dumpsys_media_screen_off.txt
adb shell input keyevent 26
sleep 1
adb shell input swipe 540 1900 540 700 300
sleep 1

# 9. 5-minute stress test with Monkey (1500 events * 200ms = 300s)
adb shell monkey -p com.yr.music --throttle 200 --pct-syskeys 0 1500 || true
sleep 2
adb shell screencap -p /data/local/tmp/08_after_monkey.png
adb pull /data/local/tmp/08_after_monkey.png screenshots/08_after_monkey.png

PID_AFTER=$(adb shell pidof com.yr.music | tr -d '\r' || true)
adb logcat -d > emulator-logcat.log
if [ -z "$PID_AFTER" ]; then
    echo "ERROR: com.yr.music exited during UI navigation!"
    tail -n 80 emulator-logcat.log
    exit 1
fi
echo "YR Music survived navigation (PID: $PID_AFTER)"
