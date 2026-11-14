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

# 4. Seek on progress bar & Pause
adb shell input tap 700 1565
sleep 1
adb shell input tap 540 1810
sleep 1
adb shell screencap -p /data/local/tmp/04_seek_pause.png
adb pull /data/local/tmp/04_seek_pause.png screenshots/04_seek_pause.png

# 5. Next track (⏭)
adb shell input tap 750 1810
sleep 2
adb shell screencap -p /data/local/tmp/05_next_track.png
adb pull /data/local/tmp/05_next_track.png screenshots/05_next_track.png

# 6. Open Queue tab (Cola)
adb shell input tap 650 340
sleep 1
adb shell screencap -p /data/local/tmp/06_queue_editable.png
adb pull /data/local/tmp/06_queue_editable.png screenshots/06_queue_editable.png

# 7. Screen off for 5s and verify background service survives, then wake
adb shell input keyevent 26
sleep 5
adb shell dumpsys activity services com.yr.music | grep -q "YrPlaybackService" && echo "YrPlaybackService alive with screen off!"
adb shell input keyevent 26
sleep 1
adb shell input swipe 540 1900 540 700 300
sleep 1

# 8. Stress test with Monkey (250 events)
adb shell monkey -p com.yr.music --throttle 200 --pct-syskeys 0 250 || true
sleep 2
adb shell screencap -p /data/local/tmp/07_after_monkey.png
adb pull /data/local/tmp/07_after_monkey.png screenshots/07_after_monkey.png

PID_AFTER=$(adb shell pidof com.yr.music | tr -d '\r' || true)
adb logcat -d > emulator-logcat.log
if [ -z "$PID_AFTER" ]; then
    echo "ERROR: com.yr.music exited during UI navigation!"
    tail -n 80 emulator-logcat.log
    exit 1
fi
echo "YR Music survived navigation (PID: $PID_AFTER)"
