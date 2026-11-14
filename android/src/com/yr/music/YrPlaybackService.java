package com.yr.music;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.media.MediaMetadata;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.os.Build;
import android.os.IBinder;
import android.os.PowerManager;

public class YrPlaybackService extends Service {
    private static final String CHANNEL_ID = "yr_playback_channel";
    private static final int NOTIF_ID = 1001;

    private static final String ACTION_PLAY_PAUSE = "com.yr.music.ACTION_PLAY_PAUSE";
    private static final String ACTION_NEXT = "com.yr.music.ACTION_NEXT";
    private static final String ACTION_PREV = "com.yr.music.ACTION_PREV";
    private static final String ACTION_STOP = "com.yr.music.ACTION_STOP";
    private static final String ACTION_UPDATE = "com.yr.music.ACTION_UPDATE";

    public static final int NATIVE_PLAY = 0;
    public static final int NATIVE_PAUSE = 1;
    public static final int NATIVE_TOGGLE = 2;
    public static final int NATIVE_NEXT = 3;
    public static final int NATIVE_PREV = 4;
    public static final int NATIVE_SEEK = 5;
    public static final int NATIVE_STOP = 6;

    private static volatile String currentTitle = "YR Music";
    private static volatile String currentArtist = "";
    private static volatile boolean currentPlaying = false;
    private static volatile long currentPosMs = 0;
    private static volatile long currentDurMs = 0;

    static {
        try {
            System.loadLibrary("yr_app");
        } catch (Throwable ignored) {
        }
    }

    private static native void nativeOnMediaAction(int action, long arg);

    private MediaSession mediaSession;
    private PowerManager.WakeLock wakeLock;

    public static void updatePlayback(
            Context ctx,
            String title,
            String artist,
            boolean playing,
            long posMs,
            long durMs
    ) {
        if (ctx == null) return;
        currentTitle = title != null ? title : "YR Music";
        currentArtist = artist != null ? artist : "";
        currentPlaying = playing;
        currentPosMs = posMs;
        currentDurMs = durMs;

        Intent intent = new Intent(ctx, YrPlaybackService.class);
        intent.setAction(ACTION_UPDATE);
        try {
            if (Build.VERSION.SDK_INT >= 26) {
                ctx.startForegroundService(intent);
            } else {
                ctx.startService(intent);
            }
        } catch (Throwable ignored) {
        }
    }

    public static void stopService(Context ctx) {
        if (ctx == null) return;
        try {
            ctx.stopService(new Intent(ctx, YrPlaybackService.class));
        } catch (Throwable ignored) {
        }
    }

    @Override
    public void onCreate() {
        super.onCreate();
        createNotificationChannel();

        PowerManager pm = (PowerManager) getSystemService(Context.POWER_SERVICE);
        if (pm != null) {
            wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "yr:playback");
            wakeLock.setReferenceCounted(false);
        }

        mediaSession = new MediaSession(this, "YrMusicSession");
        mediaSession.setFlags(
                MediaSession.FLAG_HANDLES_MEDIA_BUTTONS
                        | MediaSession.FLAG_HANDLES_TRANSPORT_CONTROLS
        );
        mediaSession.setCallback(new MediaSession.Callback() {
            @Override
            public boolean onMediaButtonEvent(Intent mediaButtonIntent) {
                android.util.Log.i("YrPlaybackService", "onMediaButtonEvent: " + mediaButtonIntent);
                return super.onMediaButtonEvent(mediaButtonIntent);
            }

            @Override
            public void onPlay() {
                android.util.Log.i("YrPlaybackService", "MediaSession.onPlay");
                nativeOnMediaAction(NATIVE_PLAY, 0);
            }

            @Override
            public void onPause() {
                android.util.Log.i("YrPlaybackService", "MediaSession.onPause");
                nativeOnMediaAction(NATIVE_PAUSE, 0);
            }

            @Override
            public void onSkipToNext() {
                android.util.Log.i("YrPlaybackService", "MediaSession.onSkipToNext");
                nativeOnMediaAction(NATIVE_NEXT, 0);
            }

            @Override
            public void onSkipToPrevious() {
                android.util.Log.i("YrPlaybackService", "MediaSession.onSkipToPrevious");
                nativeOnMediaAction(NATIVE_PREV, 0);
            }

            @Override
            public void onStop() {
                android.util.Log.i("YrPlaybackService", "MediaSession.onStop");
                nativeOnMediaAction(NATIVE_STOP, 0);
                stopSelf();
            }

            @Override
            public void onSeekTo(long pos) {
                nativeOnMediaAction(NATIVE_SEEK, pos);
            }
        });
        mediaSession.setActive(true);
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent != null ? intent.getAction() : null;
        if (ACTION_PLAY_PAUSE.equals(action)) {
            nativeOnMediaAction(NATIVE_TOGGLE, 0);
        } else if (ACTION_NEXT.equals(action)) {
            nativeOnMediaAction(NATIVE_NEXT, 0);
        } else if (ACTION_PREV.equals(action)) {
            nativeOnMediaAction(NATIVE_PREV, 0);
        } else if (ACTION_STOP.equals(action)) {
            nativeOnMediaAction(NATIVE_STOP, 0);
            stopForeground(true);
            stopSelf();
            return START_NOT_STICKY;
        }

        refreshMediaSessionAndNotification();
        return START_STICKY;
    }

    private void refreshMediaSessionAndNotification() {
        if (wakeLock != null) {
            if (currentPlaying && !wakeLock.isHeld()) {
                wakeLock.acquire(4 * 60 * 60 * 1000L);
            } else if (!currentPlaying && wakeLock.isHeld()) {
                wakeLock.release();
            }
        }

        if (mediaSession != null) {
            MediaMetadata.Builder meta = new MediaMetadata.Builder()
                    .putString(MediaMetadata.METADATA_KEY_TITLE, currentTitle)
                    .putString(MediaMetadata.METADATA_KEY_ARTIST, currentArtist);
            if (currentDurMs > 0) {
                meta.putLong(MediaMetadata.METADATA_KEY_DURATION, currentDurMs);
            }
            mediaSession.setMetadata(meta.build());

            long actions = PlaybackState.ACTION_PLAY
                    | PlaybackState.ACTION_PAUSE
                    | PlaybackState.ACTION_PLAY_PAUSE
                    | PlaybackState.ACTION_SKIP_TO_NEXT
                    | PlaybackState.ACTION_SKIP_TO_PREVIOUS
                    | PlaybackState.ACTION_SEEK_TO
                    | PlaybackState.ACTION_STOP;
            int state = currentPlaying
                    ? PlaybackState.STATE_PLAYING
                    : PlaybackState.STATE_PAUSED;
            PlaybackState pbState = new PlaybackState.Builder()
                    .setActions(actions)
                    .setState(state, currentPosMs, currentPlaying ? 1.0f : 0.0f)
                    .build();
            mediaSession.setPlaybackState(pbState);
        }

        Notification notif = buildNotification();
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIF_ID, notif, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
        } else {
            startForeground(NOTIF_ID, notif);
        }
    }

    private Notification buildNotification() {
        int piFlags = PendingIntent.FLAG_UPDATE_CURRENT;
        if (Build.VERSION.SDK_INT >= 23) {
            piFlags |= PendingIntent.FLAG_IMMUTABLE;
        }

        Intent openIntent = new Intent(this, android.app.NativeActivity.class);
        openIntent.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent contentPi = PendingIntent.getActivity(this, 0, openIntent, piFlags);

        PendingIntent prevPi = serviceActionPi(ACTION_PREV, 1, piFlags);
        PendingIntent togglePi = serviceActionPi(ACTION_PLAY_PAUSE, 2, piFlags);
        PendingIntent nextPi = serviceActionPi(ACTION_NEXT, 3, piFlags);

        Notification.Builder builder = Build.VERSION.SDK_INT >= 26
                ? new Notification.Builder(this, CHANNEL_ID)
                : new Notification.Builder(this);

        int iconRes = getResources().getIdentifier("ic_music_note", "drawable", getPackageName());
        if (iconRes == 0) {
            iconRes = android.R.drawable.ic_media_play;
        }

        builder.setSmallIcon(iconRes)
                .setContentTitle(currentTitle)
                .setContentText(currentArtist)
                .setContentIntent(contentPi)
                .setOngoing(currentPlaying)
                .setVisibility(Notification.VISIBILITY_PUBLIC)
                .addAction(android.R.drawable.ic_media_previous, "Anterior", prevPi)
                .addAction(
                        currentPlaying ? android.R.drawable.ic_media_pause : android.R.drawable.ic_media_play,
                        currentPlaying ? "Pausar" : "Reproducir",
                        togglePi
                )
                .addAction(android.R.drawable.ic_media_next, "Siguiente", nextPi);

        if (mediaSession != null) {
            Notification.MediaStyle style = new Notification.MediaStyle()
                    .setMediaSession(mediaSession.getSessionToken())
                    .setShowActionsInCompactView(0, 1, 2);
            builder.setStyle(style);
        }

        return builder.build();
    }

    private PendingIntent serviceActionPi(String action, int reqCode, int piFlags) {
        Intent i = new Intent(this, YrPlaybackService.class);
        i.setAction(action);
        return PendingIntent.getService(this, reqCode, i, piFlags);
    }

    private void createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= 26) {
            NotificationChannel ch = new NotificationChannel(
                    CHANNEL_ID,
                    "Reproducción de YR Music",
                    NotificationManager.IMPORTANCE_LOW
            );
            ch.setDescription("Controles de reproducción en segundo plano");
            ch.setShowBadge(false);
            NotificationManager nm = getSystemService(NotificationManager.class);
            if (nm != null) {
                nm.createNotificationChannel(ch);
            }
        }
    }

    @Override
    public void onDestroy() {
        if (wakeLock != null && wakeLock.isHeld()) {
            wakeLock.release();
        }
        if (mediaSession != null) {
            mediaSession.setActive(false);
            mediaSession.release();
        }
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
