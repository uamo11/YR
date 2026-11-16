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

# 1. Home tab (with moods, 4-row quick picks, Accesos directos grid, carousels)
adb shell screencap -p /data/local/tmp/01_home.png
adb pull /data/local/tmp/01_home.png screenshots/01_home.png

# 1a. Scroll Home down to capture Accesos directos grid + personalized sections
adb shell input swipe 540 1650 540 650 350
sleep 2
adb shell screencap -p /data/local/tmp/01a_home_scrolled.png
adb pull /data/local/tmp/01a_home_scrolled.png screenshots/01a_home_scrolled.png
adb shell input swipe 540 650 540 1650 350
sleep 1

# 1b. Open Google Login modal and WebView
adb shell input tap 820 140
sleep 1
adb shell screencap -p /data/local/tmp/01b_google_login_modal.png
adb pull /data/local/tmp/01b_google_login_modal.png screenshots/01b_google_login_modal.png
# Tap "Iniciar sesión con Google" inside modal
adb shell input tap 365 929
sleep 6
adb shell screencap -p /data/local/tmp/01c_google_webview.png
adb pull /data/local/tmp/01c_google_webview.png screenshots/01c_google_webview.png
adb logcat -d | grep -i "YR_LOGIN" || true
# Close WebView dialog via its top-right "CERRAR" button (940, 80) + fallback BACK if still open
adb shell input tap 940 80
sleep 1
# Tap modal "Cerrar" button (bottom-right inside centered modal)
adb shell input tap 882 1660
sleep 1

# 2. Search tab (third column at 4-tab bottom nav)
adb shell input tap 675 2160
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

# 2b. Open track row ... menu to verify all actions inside menu
adb shell input tap 980 520
sleep 1
adb shell screencap -p /data/local/tmp/02b_track_menu.png
adb pull /data/local/tmp/02b_track_menu.png screenshots/02b_track_menu.png
adb shell input tap 980 520
sleep 1

# 3. Open Settings (top right gear), trigger local audio demo (2 tracks)
adb shell input tap 1000 135
sleep 1
adb shell input tap 300 1415
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

# 6. Open Queue tab (Cola), reorder Pista 2 up (^), & Letra tab
adb shell input tap 556 266
sleep 1
adb shell screencap -p /data/local/tmp/06_queue_editable.png
adb pull /data/local/tmp/06_queue_editable.png screenshots/06_queue_editable.png

adb shell input swipe 875 678 875 678 120
sleep 1
adb shell screencap -p /data/local/tmp/06a_queue_reordered.png
adb pull /data/local/tmp/06a_queue_reordered.png screenshots/06a_queue_reordered.png

adb shell input tap 404 266
sleep 2
adb shell screencap -p /data/local/tmp/06b_lyrics.png
adb pull /data/local/tmp/06b_lyrics.png screenshots/06b_lyrics.png

# 7. Return to Portada and swipe DOWN on cover art to minimize to Mini-Player
adb shell input tap 228 266
sleep 1
adb shell input swipe 540 550 540 1050 250
sleep 1
adb shell screencap -p /data/local/tmp/07_mini_player.png
adb pull /data/local/tmp/07_mini_player.png screenshots/07_mini_player.png

# 8. Expand notification shade, capture MediaStyle notification controls, tap Siguiente & Pausar, and verify screen-off survival
adb shell cmd statusbar expand-notifications
sleep 2
adb shell screencap -p /data/local/tmp/07b_notification_controls.png
adb pull /data/local/tmp/07b_notification_controls.png screenshots/07b_notification_controls.png
adb shell uiautomator dump /data/local/tmp/notif_dump.xml || true
adb pull /data/local/tmp/notif_dump.xml screenshots/notif_dump.xml || true
{
    echo "=== BEFORE Notification Siguiente tap ==="
    adb shell dumpsys media_session | grep -A 25 "YrMusicSession" || true
    NEXT_COORDS=$(python3 -c "
import xml.etree.ElementTree as ET, re
try:
    root = ET.parse('screenshots/notif_dump.xml').getroot()
    for n in root.iter('node'):
        desc = (n.attrib.get('content-desc','') + ' ' + n.attrib.get('text','')).lower()
        if 'siguiente' in desc:
            m = re.findall(r'\d+', n.attrib.get('bounds',''))
            if len(m)==4:
                print((int(m[0])+int(m[2]))//2, (int(m[1])+int(m[3]))//2)
                break
except Exception as e:
    pass
")
    echo "NEXT_COORDS=$NEXT_COORDS"
    if [ -n "$NEXT_COORDS" ]; then
        adb shell input tap $NEXT_COORDS
        sleep 2
    fi
    echo "=== AFTER Notification Siguiente tap ==="
    adb shell dumpsys media_session | grep -A 25 "YrMusicSession" || true
} > screenshots/dumpsys_media_screen_off.txt
adb shell cmd statusbar collapse || true
sleep 1
adb shell locksettings set-pin 1234 || true
adb shell input keyevent 26
sleep 4
adb shell dumpsys activity services com.yr.music | grep -q "YrPlaybackService" && echo "YrPlaybackService alive with screen off!"
{
    echo "=== WHILE SCREEN OFF ==="
    adb shell dumpsys media_session | grep -A 25 "YrMusicSession" || true
} >> screenshots/dumpsys_media_screen_off.txt
cat screenshots/dumpsys_media_screen_off.txt
# Wake to Lockscreen and capture lockscreen media controls
adb shell input keyevent 26
sleep 2
adb shell screencap -p /data/local/tmp/07c_lockscreen_controls.png
adb pull /data/local/tmp/07c_lockscreen_controls.png screenshots/07c_lockscreen_controls.png
adb shell input swipe 540 1900 540 700 300
sleep 1
adb shell input text 1234
sleep 1
adb shell input keyevent 66
sleep 1
adb shell locksettings clear --old 1234 || true

# 8b. Re-open Now Playing -> Cola and remove non-playing track (X)
adb shell input tap 1015 2020
sleep 1
adb shell input tap 556 266
sleep 1
adb shell input swipe 990 520 990 520 120
sleep 1
adb shell screencap -p /data/local/tmp/06c_queue_removed.png
adb pull /data/local/tmp/06c_queue_removed.png screenshots/06c_queue_removed.png

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
