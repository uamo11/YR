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
START_RES=$(adb shell am start -W -S -n com.yr.music/com.yr.music.YrMainActivity)
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

# 1. Home tab (edge-to-edge mood chips, quick picks, Accesos directos)
adb shell screencap -p /data/local/tmp/01_home.png
adb pull /data/local/tmp/01_home.png screenshots/01_home.png

# 1a. Scroll Home down to capture Accesos directos grid + carousels
adb shell input swipe 540 1650 540 650 350
sleep 2
adb shell screencap -p /data/local/tmp/01a_home_scrolled.png
adb pull /data/local/tmp/01a_home_scrolled.png screenshots/01a_home_scrolled.png
adb shell input swipe 540 650 540 1650 350
sleep 1

# 2. Account modal, Tu Recap (nav-bar clearance) & Insignias modal
adb shell input tap 895 140
sleep 1.5
adb shell screencap -p /data/local/tmp/02a_account_modal.png
adb pull /data/local/tmp/02a_account_modal.png screenshots/02a_account_modal.png

adb shell input tap 540 1070
sleep 1.5
adb shell screencap -p /data/local/tmp/02b_recap_modal.png
adb pull /data/local/tmp/02b_recap_modal.png screenshots/02b_recap_modal.png
adb shell input keyevent 4
sleep 1

adb shell input tap 895 140
sleep 1.2
adb shell input tap 540 1218
sleep 1.5
adb shell screencap -p /data/local/tmp/02c_badges_modal.png
adb pull /data/local/tmp/02c_badges_modal.png screenshots/02c_badges_modal.png
adb shell input keyevent 4
sleep 1

# 3. Unified Search ("Todo" default tab)
adb shell input tap 675 2180
sleep 1.5
adb shell input tap 350 265
sleep 1
adb shell input text "daft%spunk"
sleep 1
adb shell input tap 974 265
sleep 5
adb shell screencap -p /data/local/tmp/03_search_all.png
adb pull /data/local/tmp/03_search_all.png screenshots/03_search_all.png

# 3b. Open top Artist ("Daft Punk") from unified search results
adb shell input tap 450 745
sleep 4
adb shell screencap -p /data/local/tmp/03b_artist_detail.png
adb pull /data/local/tmp/03b_artist_detail.png screenshots/03b_artist_detail.png
adb shell input keyevent 4
sleep 1

# 4. Samples discovery (Video quick button + interactive seek bar)
adb shell input tap 405 2180
sleep 2
adb shell input tap 450 1100
sleep 4
adb shell screencap -p /data/local/tmp/04_samples_playing.png
adb pull /data/local/tmp/04_samples_playing.png screenshots/04_samples_playing.png

# Verify ForegroundService, Notification, and MediaSession
adb shell dumpsys notification --noredact | grep -A 25 "com.yr.music" > screenshots/dumpsys_notif.txt || true
adb shell dumpsys media_session | grep -A 25 "YrMusicSession" > screenshots/dumpsys_media.txt || true
adb shell dumpsys activity services com.yr.music > screenshots/dumpsys_service.txt || true
cat screenshots/dumpsys_service.txt

# 5. Now Playing (edge-to-edge action pills) + 5-speed Action Sheet
adb shell input tap 350 1960
sleep 1.5
adb shell screencap -p /data/local/tmp/05a_now_playing.png
adb pull /data/local/tmp/05a_now_playing.png screenshots/05a_now_playing.png

adb shell input tap 988 152
sleep 1.5
adb shell screencap -p /data/local/tmp/05b_action_sheet_5speeds.png
adb pull /data/local/tmp/05b_action_sheet_5speeds.png screenshots/05b_action_sheet_5speeds.png
adb shell input keyevent 4
sleep 1

# 6. Queue drawer (SIGUIENTE) & Lyrics (LETRA with A+ & Compartir)
adb shell input tap 195 2215
sleep 1.5
adb shell screencap -p /data/local/tmp/06a_queue_tuner.png
adb pull /data/local/tmp/06a_queue_tuner.png screenshots/06a_queue_tuner.png

adb shell input tap 540 2215
sleep 3
adb shell screencap -p /data/local/tmp/06b_lyrics_karaoke.png
adb pull /data/local/tmp/06b_lyrics_karaoke.png screenshots/06b_lyrics_karaoke.png
adb shell input keyevent 4
sleep 1

# 7. In-place Video mode in Now Playing
adb shell input tap 645 152
sleep 5
adb shell screencap -p /data/local/tmp/07_video_inplace.png
adb pull /data/local/tmp/07_video_inplace.png screenshots/07_video_inplace.png
adb shell input keyevent 4
sleep 1

# 7b. Library tab (edge-to-edge filters, Sort pill, Liked & Offline downloads)
adb shell input tap 945 2180
sleep 2
adb shell screencap -p /data/local/tmp/07b_library.png
adb pull /data/local/tmp/07b_library.png screenshots/07b_library.png

# 8. Notification controls, screen-off survival & lockscreen controls
adb shell cmd statusbar expand-notifications
sleep 2
adb shell screencap -p /data/local/tmp/08a_notification_controls.png
adb pull /data/local/tmp/08a_notification_controls.png screenshots/08a_notification_controls.png
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
adb shell input keyevent 26
sleep 2
adb shell screencap -p /data/local/tmp/08b_lockscreen_controls.png
adb pull /data/local/tmp/08b_lockscreen_controls.png screenshots/08b_lockscreen_controls.png
adb shell input swipe 540 1900 540 700 300
sleep 1
adb shell input text 1234
sleep 1
adb shell input keyevent 66
sleep 1
adb shell locksettings clear --old 1234 || true

# 9. Monkey stability stress test
adb shell monkey -p com.yr.music --throttle 150 --pct-syskeys 0 400 || true
sleep 2
adb shell screencap -p /data/local/tmp/09_after_monkey.png
adb pull /data/local/tmp/09_after_monkey.png screenshots/09_after_monkey.png

PID_AFTER=$(adb shell pidof com.yr.music | tr -d '\r' || true)
adb logcat -d > emulator-logcat.log
if [ -z "$PID_AFTER" ]; then
    echo "ERROR: com.yr.music exited during UI navigation!"
    tail -n 80 emulator-logcat.log
    exit 1
fi
echo "YR Music survived navigation (PID: $PID_AFTER)"
