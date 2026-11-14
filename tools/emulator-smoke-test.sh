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
sleep 5

PID=$(adb shell pidof com.yr.music | tr -d '\r' || true)
if [ -z "$PID" ]; then
    echo "ERROR: com.yr.music crashed on startup!"
    adb logcat -d > emulator-logcat.log
    cat emulator-logcat.log | tail -n 80
    exit 1
fi
echo "YR Music running with PID: $PID"

# 1. Home tab
adb shell screencap -p /data/local/tmp/01_home.png
adb pull /data/local/tmp/01_home.png screenshots/01_home.png

# 2. Search tab (second column at bottom nav)
adb shell input tap 405 2280
sleep 2
adb shell screencap -p /data/local/tmp/02_search.png
adb pull /data/local/tmp/02_search.png screenshots/02_search.png

# 3. Library tab (third column)
adb shell input tap 675 2280
sleep 2
adb shell screencap -p /data/local/tmp/03_library.png
adb pull /data/local/tmp/03_library.png screenshots/03_library.png

# 4. Downloads tab (fourth column)
adb shell input tap 945 2280
sleep 2
adb shell screencap -p /data/local/tmp/04_downloads.png
adb pull /data/local/tmp/04_downloads.png screenshots/04_downloads.png

# Verify process is still alive after all tab transitions
PID_AFTER=$(adb shell pidof com.yr.music | tr -d '\r' || true)
adb logcat -d > emulator-logcat.log
if [ -z "$PID_AFTER" ]; then
    echo "ERROR: com.yr.music exited during UI navigation!"
    tail -n 80 emulator-logcat.log
    exit 1
fi
echo "YR Music survived navigation (PID: $PID_AFTER)"
