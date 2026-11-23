package com.yr.music;

import android.app.Activity;
import android.app.Dialog;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.content.pm.ServiceInfo;
import android.content.res.ColorStateList;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Canvas;
import android.graphics.Matrix;
import android.graphics.Outline;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.PixelFormat;
import android.graphics.SurfaceTexture;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.media.MediaMetadata;
import android.media.MediaPlayer;
import android.media.PlaybackParams;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.Uri;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.PowerManager;
import android.os.SystemClock;
import android.text.TextUtils;
import android.util.Log;
import android.view.GestureDetector;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.Surface;
import android.view.TextureView;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewOutlineProvider;
import android.view.WindowManager;
import android.webkit.CookieManager;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.TextView;
import java.io.InputStream;
import java.lang.ref.WeakReference;
import java.net.HttpURLConnection;
import java.net.URL;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

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
    private static volatile String currentArtworkUrl = "";
    private static volatile Bitmap currentArtworkBitmap = null;
    private static volatile YrPlaybackService activeInstance = null;
    private static volatile String lastMetaKey = "";
    private static volatile Boolean lastNotifPlaying = null;
    private static volatile FrameLayout sVideoOverlayRoot = null;
    private static volatile TextureView sVideoTextureView = null;
    private static volatile ProgressBar sVideoSpinner = null;
    private static volatile WindowManager.LayoutParams sVideoParams = null;
    private static volatile boolean sVideoAdded = false;
    private static volatile MediaPlayer sMediaPlayer = null;
    private static volatile Surface sVideoSurface = null;
    private static volatile String sVideoUrl = null;
    private static volatile String sVideoFallbackUrl = null;
    private static volatile String sVideoUserAgent = null;
    private static volatile boolean sVideoPrepared = false;
    private static volatile boolean sVideoRenderedFirstFrame = false;
    private static volatile boolean sSeekInFlight = false;
    private static volatile long sLastSeekStartedElapsedMs = 0L;
    private static volatile TextView sVideoSeekBadge = null;
    private static final Handler sUiHandler = new Handler(Looper.getMainLooper());
    private static Runnable sHideSeekBadgeRunnable = null;
    private static volatile View sVideoPauseIcon = null;
    private static volatile TextView sVideoFullscreenBtn = null;
    private static volatile TextView sVideoFullscreenTitle = null;
    private static volatile boolean sVideoFullscreen = false;
    private static volatile int sCompactX = 0;
    private static volatile int sCompactY = 0;
    private static volatile int sCompactW = 0;
    private static volatile int sCompactH = 0;
    private static volatile int sSeekSeconds = 10;
    private static volatile int sSavedSystemUiVisibility = -1;
    private static volatile WeakReference<Activity> sVideoActivityRef = null;
    private static volatile boolean sTriedFallback = false;
    private static volatile boolean sVideoWantedVisible = false;
    private static volatile boolean sPendingPlaying = false;
    private static volatile long sPendingPosMs = 0;
    private static volatile float sPendingSpeed = 1.0f;
    private static volatile float sLastAppliedSpeed = 1.0f;
    private static volatile int sVideoWidth = 0;
    private static volatile int sVideoHeight = 0;
    private static volatile boolean sActivityPaused = false;

    static {
        try {
            System.loadLibrary("yr_app");
        } catch (Throwable ignored) {
        }
    }

    private static native void nativeOnMediaAction(int action, long arg);
    private static native void nativeOnGoogleCookie(String cookie);

    private MediaSession mediaSession;
    private PowerManager.WakeLock wakeLock;

    private static String buildMetaKey() {
        Bitmap art = currentArtworkBitmap;
        int artId = (art != null && !art.isRecycled()) ? System.identityHashCode(art) : 0;
        return currentTitle + "\0" + currentArtist + "\0" + currentDurMs + "\0" + artId;
    }

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

        YrPlaybackService svc = activeInstance;
        String metaKey = buildMetaKey();
        if (svc != null
                && metaKey.equals(lastMetaKey)
                && lastNotifPlaying != null
                && lastNotifPlaying.booleanValue() == playing) {
            svc.publishPlaybackState();
            return;
        }

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

    public static void setArtworkUrl(final Context ctx, final String url) {
        if (ctx == null) return;
        final String normalized = url != null ? url.trim() : "";
        if (normalized.equals(currentArtworkUrl)
                && (normalized.isEmpty() || currentArtworkBitmap != null)) {
            return;
        }
        currentArtworkUrl = normalized;
        // Clear previous track's bitmap immediately so notification/lockscreen never pairs old art with new title.
        currentArtworkBitmap = null;
        if (normalized.isEmpty()) {
            return;
        }
        new Thread(() -> {
            HttpURLConnection conn = null;
            try {
                URL u = new URL(normalized);
                conn = (HttpURLConnection) u.openConnection();
                conn.setConnectTimeout(6000);
                conn.setReadTimeout(8000);
                conn.setInstanceFollowRedirects(true);
                try (InputStream is = conn.getInputStream()) {
                    Bitmap raw = BitmapFactory.decodeStream(is);
                    if (raw != null && normalized.equals(currentArtworkUrl)) {
                        int w = raw.getWidth();
                        int h = raw.getHeight();
                        int maxDim = Math.max(w, h);
                        Bitmap scaled = raw;
                        if (maxDim > 512) {
                            float ratio = 512.0f / maxDim;
                            scaled = Bitmap.createScaledBitmap(
                                    raw,
                                    Math.max(1, Math.round(w * ratio)),
                                    Math.max(1, Math.round(h * ratio)),
                                    true
                            );
                            if (scaled != raw) {
                                raw.recycle();
                            }
                        }
                        currentArtworkBitmap = scaled;
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
                }
            } catch (Throwable t) {
                Log.w("YrPlaybackService", "setArtworkUrl failed: " + t);
            } finally {
                if (conn != null) {
                    conn.disconnect();
                }
            }
        }, "yr-artwork-loader").start();
    }

    public static void stopService(Context ctx) {
        if (ctx == null) return;
        try {
            ctx.stopService(new Intent(ctx, YrPlaybackService.class));
        } catch (Throwable ignored) {
        }
    }

    public static String resolveAudioUrl(final Context ctx, final String videoId) {
        Log.i("YR_WEB", "resolveAudioUrl start vid=" + videoId);
        if (ctx == null || videoId == null || videoId.isEmpty()) return null;
        if (Looper.myLooper() == Looper.getMainLooper()) {
            Log.w("YR_WEB", "resolveAudioUrl called on main looper");
            return null;
        }

        final Handler mainHandler = new Handler(Looper.getMainLooper());
        final CountDownLatch latch = new CountDownLatch(1);
        final AtomicReference<String> result = new AtomicReference<>(null);
        final AtomicReference<String> fallbackMp4 = new AtomicReference<>(null);
        final AtomicReference<WebView> webRef = new AtomicReference<>(null);

        mainHandler.post(() -> {
            try {
                WebView wv = new WebView(ctx.getApplicationContext());
                webRef.set(wv);
                wv.layout(0, 0, 360, 200);
                wv.onResume();
                wv.resumeTimers();
                WebSettings ws = wv.getSettings();
                ws.setJavaScriptEnabled(true);
                ws.setDomStorageEnabled(true);
                ws.setMediaPlaybackRequiresUserGesture(false);
                ws.setUserAgentString(
                        "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 "
                                + "(KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36"
                );
                wv.setWebViewClient(new WebViewClient() {
                    private int gvCount = 0;

                    @Override
                    public void onPageStarted(WebView view, String url, android.graphics.Bitmap favicon) {
                        try {
                            view.evaluateJavascript(
                                    "try{if(window.MediaSource&&MediaSource.isTypeSupported){"
                                            + "var o=MediaSource.isTypeSupported.bind(MediaSource);"
                                            + "MediaSource.isTypeSupported=function(t){"
                                            + "if(t&&t.indexOf('webm')!==-1)return false;return o(t);};}}catch(e){}",
                                    null
                            );
                        } catch (Throwable ignored) {
                        }
                    }

                    @Override
                    public android.webkit.WebResourceResponse shouldInterceptRequest(
                            WebView view, WebResourceRequest request
                    ) {
                        try {
                            String u = request.getUrl().toString();
                            if (u.contains("googlevideo.com/videoplayback")) {
                                if (gvCount < 8) {
                                    gvCount++;
                                    Log.i("YR_WEB", "gv[" + gvCount + "] hasPot=" + u.contains("pot=")
                                            + " hasUmp=" + u.contains("ump=")
                                            + ": " + u.substring(0, Math.min(200, u.length())));
                                }
                                if (u.contains("mime=audio/mp4")
                                        || u.contains("mime%3Daudio%2Fmp4")
                                        || u.contains("itag=140")
                                        || u.contains("itag=139")) {
                                    String clean = u.replaceAll("([?&])(range|rn|rbuf|ump|srfvp)=[^&]*", "$1")
                                            .replace("?&", "?")
                                            .replaceAll("&&+", "&");
                                    Log.i("YR_WEB", "Intercepted audio/mp4 URL for " + videoId + " hasPot=" + clean.contains("pot="));
                                    if (result.compareAndSet(null, clean)) {
                                        latch.countDown();
                                    }
                                } else if (u.contains("mime=video/mp4")
                                        || u.contains("mime%3Dvideo%2Fmp4")
                                        || u.contains("itag=18")) {
                                    String clean = u.replaceAll("([?&])(range|rn|rbuf|ump|srfvp)=[^&]*", "$1")
                                            .replace("?&", "?")
                                            .replaceAll("&&+", "&");
                                    fallbackMp4.compareAndSet(null, clean);
                                }
                            }
                        } catch (Throwable ignored) {
                        }
                        return super.shouldInterceptRequest(view, request);
                    }

                    @Override
                    public void onPageFinished(WebView view, String url) {
                        try {
                            view.evaluateJavascript(
                                    "(function(){function poke(){try{var v=document.querySelector('video');if(v){v.muted=true;v.play();}var b=document.querySelector('.ytp-large-play-button,.ytp-play-button,button[aria-label*=\"Play\"],button[aria-label*=\"Reproducir\"]');if(b)b.click();}catch(e){}}poke();setTimeout(poke,400);setTimeout(poke,1200);})();",
                                    null
                            );
                        } catch (Throwable ignored) {
                        }
                    }
                });
                wv.loadUrl("https://m.youtube.com/watch?v=" + videoId);
            } catch (Throwable t) {
                Log.e("YR_WEB", "resolveAudioUrl failed", t);
                latch.countDown();
            }
        });

        try {
            latch.await(8, TimeUnit.SECONDS);
        } catch (InterruptedException ignored) {
        } finally {
            mainHandler.post(() -> {
                try {
                    WebView wv = webRef.get();
                    if (wv != null) {
                        wv.stopLoading();
                        wv.loadUrl("about:blank");
                        wv.destroy();
                    }
                } catch (Throwable ignored) {
                }
            });
        }
        String finalUrl = result.get() != null ? result.get() : fallbackMp4.get();
        Log.i("YR_WEB", "resolveAudioUrl end vid=" + videoId + " ok=" + (finalUrl != null));
        return finalUrl;
    }

    private static String mergeYoutubeCookies() {
        CookieManager cm = CookieManager.getInstance();
        cm.flush();
        String cMusic = cm.getCookie("https://music.youtube.com");
        String cMain = cm.getCookie("https://www.youtube.com");
        String cYt = cm.getCookie("https://youtube.com");
        String cAcc = cm.getCookie("https://accounts.google.com");
        String cGoogle = cm.getCookie("https://google.com");
        java.util.LinkedHashMap<String, String> map = new java.util.LinkedHashMap<>();
        for (String src : new String[]{cMain, cYt, cAcc, cGoogle, cMusic}) {
            if (src == null || src.isEmpty()) continue;
            for (String part : src.split(";")) {
                int eq = part.indexOf('=');
                if (eq > 0) {
                    String k = part.substring(0, eq).trim();
                    String v = part.substring(eq + 1).trim();
                    if (!k.isEmpty() && !v.isEmpty()) {
                        map.put(k, v);
                    }
                }
            }
        }
        StringBuilder sb = new StringBuilder();
        for (java.util.Map.Entry<String, String> e : map.entrySet()) {
            if (sb.length() > 0) sb.append("; ");
            sb.append(e.getKey()).append("=").append(e.getValue());
        }
        return sb.toString();
    }

    private static boolean hasGoogleAuthCookies(String cookies) {
        if (cookies == null || cookies.isEmpty()) return false;
        boolean hasSapi = cookies.contains("SAPISID=")
                || cookies.contains("__Secure-3PAPISID=")
                || cookies.contains("__Secure-1PAPISID=");
        boolean hasSid = cookies.contains("SID=")
                || cookies.contains("__Secure-3PSID=")
                || cookies.contains("LOGIN_INFO=");
        return hasSapi && hasSid;
    }

    public static void shareText(final Context ctx, final String title, final String text) {
        if (ctx == null || text == null) return;
        new Handler(Looper.getMainLooper()).post(() -> {
            try {
                Intent send = new Intent(Intent.ACTION_SEND);
                send.setType("text/plain");
                send.putExtra(Intent.EXTRA_SUBJECT, title != null ? title : "YR Music");
                send.putExtra(Intent.EXTRA_TEXT, text);
                Intent chooser = Intent.createChooser(send, "Compartir canción");
                chooser.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                ctx.startActivity(chooser);
            } catch (Throwable ignored) {
            }
        });
    }

    public static boolean isWifiConnected(final Context ctx) {
        if (ctx == null) return false;
        try {
            ConnectivityManager cm = (ConnectivityManager) ctx.getSystemService(Context.CONNECTIVITY_SERVICE);
            if (cm == null) return false;
            Network network = cm.getActiveNetwork();
            if (network == null) return false;
            NetworkCapabilities caps = cm.getNetworkCapabilities(network);
            if (caps == null) return false;
            return caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
                    || caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET);
        } catch (Throwable ignored) {
            return false;
        }
    }

    public static boolean isMeteredNetwork(final Context ctx) {
        if (ctx == null) return false;
        try {
            ConnectivityManager cm = (ConnectivityManager) ctx.getSystemService(Context.CONNECTIVITY_SERVICE);
            if (cm == null) return false;
            return cm.isActiveNetworkMetered();
        } catch (Throwable ignored) {
            return false;
        }
    }

    public static String extractVideoId(final String videoUrlOrId) {
        if (videoUrlOrId == null) return "";
        String videoId = videoUrlOrId.trim();
        if (videoId.contains("v=")) {
            videoId = videoId.substring(videoId.indexOf("v=") + 2);
            int amp = videoId.indexOf("&");
            if (amp != -1) videoId = videoId.substring(0, amp);
        } else if (videoId.contains("youtu.be/")) {
            videoId = videoId.substring(videoId.indexOf("youtu.be/") + 9);
            int q = videoId.indexOf("?");
            if (q != -1) videoId = videoId.substring(0, q);
        } else if (videoId.contains("embed/")) {
            videoId = videoId.substring(videoId.indexOf("embed/") + 6);
            int q = videoId.indexOf("?");
            if (q != -1) videoId = videoId.substring(0, q);
        }
        return videoId;
    }

    private static void applyVideoAspectTransform() {
        if (sVideoTextureView == null || sVideoWidth <= 0 || sVideoHeight <= 0) return;
        int viewWidth = sVideoTextureView.getWidth();
        int viewHeight = sVideoTextureView.getHeight();
        if (viewWidth <= 0 || viewHeight <= 0) return;
        float viewAspect = (float) viewWidth / (float) viewHeight;
        float videoAspect = (float) sVideoWidth / (float) sVideoHeight;
        float scaleX = 1.0f;
        float scaleY = 1.0f;
        if (videoAspect > viewAspect) {
            scaleY = viewAspect / videoAspect;
        } else {
            scaleX = videoAspect / viewAspect;
        }
        Matrix matrix = new Matrix();
        matrix.setScale(scaleX, scaleY, viewWidth * 0.5f, viewHeight * 0.5f);
        sVideoTextureView.setTransform(matrix);
    }

    private static void seekMediaPlayerTo(MediaPlayer mp, long posMs, boolean force) {
        if (mp == null) return;
        long now = SystemClock.elapsedRealtime();
        if (!force) {
            if (sSeekInFlight && (now - sLastSeekStartedElapsedMs) < 7000L) {
                return;
            }
            if ((now - sLastSeekStartedElapsedMs) < 3500L) {
                return;
            }
        }
        long clamped = Math.max(0L, posMs);
        sSeekInFlight = true;
        sLastSeekStartedElapsedMs = now;
        try {
            if (Build.VERSION.SDK_INT >= 26) {
                mp.seekTo(clamped, MediaPlayer.SEEK_CLOSEST_SYNC);
            } else {
                mp.seekTo((int) Math.min(Integer.MAX_VALUE, clamped));
            }
        } catch (Throwable ignored) {
            sSeekInFlight = false;
        }
    }

    private static void showVideoSeekBadge(final Activity act, final String label, final boolean rightSide) {
        if (act == null || sVideoSeekBadge == null) return;
        final float density = act.getResources().getDisplayMetrics().density;
        final int marginPx = (int) (22f * density);
        FrameLayout.LayoutParams lp = new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.WRAP_CONTENT,
                FrameLayout.LayoutParams.WRAP_CONTENT
        );
        lp.gravity = Gravity.CENTER_VERTICAL | (rightSide ? Gravity.END : Gravity.START);
        if (rightSide) {
            lp.setMargins(0, 0, marginPx, 0);
        } else {
            lp.setMargins(marginPx, 0, 0, 0);
        }
        sVideoSeekBadge.setLayoutParams(lp);
        sVideoSeekBadge.setText(label);
        sVideoSeekBadge.setVisibility(View.VISIBLE);
        if (sHideSeekBadgeRunnable != null) {
            sUiHandler.removeCallbacks(sHideSeekBadgeRunnable);
        }
        sHideSeekBadgeRunnable = () -> {
            if (sVideoSeekBadge != null) {
                sVideoSeekBadge.setVisibility(View.GONE);
            }
        };
        sUiHandler.postDelayed(sHideSeekBadgeRunnable, 850L);
    }

    public static void setSeekSeconds(final int seconds) {
        sSeekSeconds = Math.max(5, Math.min(60, seconds));
    }

    private static void updateVideoPauseIcon() {
        if (sVideoPauseIcon == null) return;
        boolean show = sVideoPrepared && !sPendingPlaying;
        sVideoPauseIcon.setVisibility(show ? View.VISIBLE : View.GONE);
    }

    public static boolean exitFullscreenIfActive(final Activity act) {
        if (!sVideoFullscreen) return false;
        sUiHandler.post(() -> setVideoFullscreen(act, false));
        return true;
    }

    private static void setVideoFullscreen(final Activity act, final boolean fullscreen) {
        Activity targetAct = (act != null)
                ? act
                : (sVideoActivityRef != null ? sVideoActivityRef.get() : null);
        if (targetAct == null || sVideoOverlayRoot == null || sVideoParams == null || !sVideoAdded) {
            sVideoFullscreen = false;
            return;
        }
        WindowManager wm = (WindowManager) targetAct.getSystemService(Context.WINDOW_SERVICE);
        if (wm == null) return;

        sVideoFullscreen = fullscreen;
        View decor = (targetAct.getWindow() != null) ? targetAct.getWindow().getDecorView() : null;
        if (fullscreen) {
            try {
                targetAct.setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE);
            } catch (Throwable ignored) {
            }
            if (decor != null) {
                if (sSavedSystemUiVisibility < 0) {
                    sSavedSystemUiVisibility = decor.getSystemUiVisibility();
                }
                decor.setSystemUiVisibility(
                        View.SYSTEM_UI_FLAG_FULLSCREEN
                                | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                                | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                                | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                                | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                                | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                );
            }
            sVideoOverlayRoot.setClipToOutline(false);
            sVideoOverlayRoot.setBackgroundColor(0xFF000000);
            sVideoParams.x = 0;
            sVideoParams.y = 0;
            sVideoParams.width = WindowManager.LayoutParams.MATCH_PARENT;
            sVideoParams.height = WindowManager.LayoutParams.MATCH_PARENT;
            if (sVideoFullscreenBtn != null) {
                sVideoFullscreenBtn.setText("\u2715");
            }
            if (sVideoFullscreenTitle != null) {
                String artist = (currentArtist != null && !currentArtist.isEmpty())
                        ? ("  \u2022  " + currentArtist)
                        : "";
                sVideoFullscreenTitle.setText(currentTitle + artist);
                sVideoFullscreenTitle.setVisibility(View.VISIBLE);
            }
        } else {
            try {
                targetAct.setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_PORTRAIT);
            } catch (Throwable ignored) {
            }
            if (decor != null) {
                decor.setSystemUiVisibility(
                        sSavedSystemUiVisibility >= 0 ? sSavedSystemUiVisibility : View.SYSTEM_UI_FLAG_VISIBLE
                );
            }
            sVideoOverlayRoot.setClipToOutline(true);
            sVideoOverlayRoot.setBackgroundColor(0xFF0A0A0C);
            sVideoParams.x = sCompactX;
            sVideoParams.y = sCompactY;
            sVideoParams.width = Math.max(1, sCompactW);
            sVideoParams.height = Math.max(1, sCompactH);
            if (sVideoFullscreenBtn != null) {
                sVideoFullscreenBtn.setText("\u26F6");
            }
            if (sVideoFullscreenTitle != null) {
                sVideoFullscreenTitle.setVisibility(View.GONE);
            }
        }
        try {
            wm.updateViewLayout(sVideoOverlayRoot, sVideoParams);
        } catch (Throwable updErr) {
            Log.w("YrPlaybackService", "setVideoFullscreen updateViewLayout failed: " + updErr);
        }
        sVideoOverlayRoot.post(YrPlaybackService::applyVideoAspectTransform);
        Log.i("YrPlaybackService", "setVideoFullscreen=" + fullscreen);
    }

    private static void applySpeedToMediaPlayer(MediaPlayer mp, float speed) {
        if (mp == null || Build.VERSION.SDK_INT < 23) return;
        float clamped = Math.max(0.5f, Math.min(2.0f, speed));
        try {
            PlaybackParams pp = mp.getPlaybackParams();
            pp.setSpeed(clamped);
            mp.setPlaybackParams(pp);
            sLastAppliedSpeed = clamped;
        } catch (Throwable ignored) {
        }
    }

    private static void releaseMediaPlayerLocked() {
        sVideoPrepared = false;
        sVideoRenderedFirstFrame = false;
        sSeekInFlight = false;
        sLastSeekStartedElapsedMs = 0L;
        sVideoWidth = 0;
        sVideoHeight = 0;
        sLastAppliedSpeed = 1.0f;
        updateVideoPauseIcon();
        MediaPlayer old = sMediaPlayer;
        sMediaPlayer = null;
        if (old != null) {
            try {
                old.setSurface(null);
            } catch (Throwable ignored) {
            }
            try {
                old.reset();
            } catch (Throwable ignored) {
            }
            try {
                old.release();
            } catch (Throwable ignored) {
            }
        }
    }

    private static void prepareNativeMediaPlayer(final Activity act, final String streamUrl, final String ua) {
        if (act == null || streamUrl == null || streamUrl.isEmpty()) return;
        releaseMediaPlayerLocked();
        if (sVideoSpinner != null) {
            sVideoSpinner.setVisibility(View.VISIBLE);
        }
        try {
            final MediaPlayer mp = new MediaPlayer();
            sMediaPlayer = mp;
            mp.setVolume(0f, 0f);
            mp.setLooping(false);
            if (sVideoSurface != null && sVideoSurface.isValid()) {
                mp.setSurface(sVideoSurface);
            }
            mp.setOnVideoSizeChangedListener((player, vw, vh) -> {
                if (player != sMediaPlayer) return;
                sVideoWidth = vw;
                sVideoHeight = vh;
                applyVideoAspectTransform();
            });
            mp.setOnInfoListener((player, what, extra) -> {
                if (player != sMediaPlayer) return false;
                if (what == MediaPlayer.MEDIA_INFO_VIDEO_RENDERING_START
                        || what == MediaPlayer.MEDIA_INFO_BUFFERING_END) {
                    sVideoRenderedFirstFrame = true;
                    if (sVideoSpinner != null) sVideoSpinner.setVisibility(View.GONE);
                } else if (what == MediaPlayer.MEDIA_INFO_BUFFERING_START) {
                    if (!sVideoRenderedFirstFrame && sVideoSpinner != null) {
                        sVideoSpinner.setVisibility(View.VISIBLE);
                    }
                }
                return false;
            });
            mp.setOnSeekCompleteListener(player -> {
                if (player != sMediaPlayer) return;
                sSeekInFlight = false;
                if (sVideoSpinner != null) sVideoSpinner.setVisibility(View.GONE);
            });
            mp.setOnPreparedListener(player -> {
                if (player != sMediaPlayer) return;
                sVideoPrepared = true;
                player.setVolume(0f, 0f);
                if (sVideoSpinner != null) {
                    sVideoSpinner.setVisibility(View.GONE);
                }
                if (sPendingPosMs > 1200) {
                    seekMediaPlayerTo(player, sPendingPosMs, true);
                }
                if (Math.abs(sPendingSpeed - 1.0f) > 0.03f) {
                    applySpeedToMediaPlayer(player, sPendingSpeed);
                }
                applyVideoAspectTransform();
                if (sPendingPlaying && !sActivityPaused && sVideoWantedVisible) {
                    try {
                        player.start();
                    } catch (Throwable ignored) {
                    }
                }
                updateVideoPauseIcon();
                Log.i("YrPlaybackService", "Native MediaPlayer prepared: " + sVideoWidth + "x" + sVideoHeight);
            });
            mp.setOnErrorListener((player, what, extra) -> {
                Log.w("YrPlaybackService", "Native MediaPlayer error what=" + what + " extra=" + extra);
                if (player != sMediaPlayer) return true;
                if (!sTriedFallback && sVideoFallbackUrl != null && !sVideoFallbackUrl.isEmpty()) {
                    sTriedFallback = true;
                    Log.i("YrPlaybackService", "Retrying fallback HLS video stream");
                    prepareNativeMediaPlayer(act, sVideoFallbackUrl, sVideoUserAgent);
                    return true;
                }
                if (sVideoSpinner != null) {
                    sVideoSpinner.setVisibility(View.GONE);
                }
                return true;
            });
            Map<String, String> headers = new HashMap<String, String>();
            if (ua != null && !ua.isEmpty()) {
                headers.put("User-Agent", ua);
            }
            mp.setDataSource(act, Uri.parse(streamUrl), headers);
            mp.prepareAsync();
        } catch (Throwable t) {
            Log.w("YrPlaybackService", "prepareNativeMediaPlayer failed: " + t);
            if (!sTriedFallback && sVideoFallbackUrl != null && !sVideoFallbackUrl.isEmpty()) {
                sTriedFallback = true;
                prepareNativeMediaPlayer(act, sVideoFallbackUrl, sVideoUserAgent);
            } else if (sVideoSpinner != null) {
                sVideoSpinner.setVisibility(View.GONE);
            }
        }
    }

    public static void attachNativeVideo(
            final Context ctx,
            final String videoUrl,
            final String fallbackUrl,
            final String userAgent,
            final int x,
            final int y,
            final int width,
            final int height,
            final long startMs,
            final boolean playing,
            final float speed
    ) {
        if (ctx == null || videoUrl == null || videoUrl.isEmpty()) return;
        final Activity act = (ctx instanceof Activity) ? (Activity) ctx : null;
        if (act == null || act.isFinishing()) return;
        if (Build.VERSION.SDK_INT >= 17 && act.isDestroyed()) return;

        new Handler(Looper.getMainLooper()).post(() -> {
            try {
                if (act.isFinishing()) return;
                if (Build.VERSION.SDK_INT >= 17 && act.isDestroyed()) return;
                View decor = act.getWindow() != null ? act.getWindow().getDecorView() : null;
                IBinder token = decor != null ? decor.getWindowToken() : null;
                if (token == null) {
                    Log.w("YrPlaybackService", "attachNativeVideo: decor windowToken is null");
                    return;
                }
                WindowManager wm = (WindowManager) act.getSystemService(Context.WINDOW_SERVICE);
                if (wm == null) return;

                final int wx = x;
                final int wy = y;
                sVideoActivityRef = new WeakReference<Activity>(act);
                if (!sVideoFullscreen) {
                    sCompactX = wx;
                    sCompactY = wy;
                    sCompactW = width;
                    sCompactH = height;
                }
                Log.i("YrPlaybackService", "attachNativeVideo rect=(" + wx + "," + wy + "," + width + "x" + height + ")");

                if (sVideoOverlayRoot == null) {
                    final float radiusPx = 12f * act.getResources().getDisplayMetrics().density;
                    sVideoOverlayRoot = new FrameLayout(act);
                    sVideoOverlayRoot.setBackgroundColor(0xFF0A0A0C);
                    sVideoOverlayRoot.setClipToOutline(true);
                    sVideoOverlayRoot.setOutlineProvider(new ViewOutlineProvider() {
                        @Override
                        public void getOutline(View view, Outline outline) {
                            outline.setRoundRect(0, 0, view.getWidth(), view.getHeight(), radiusPx);
                        }
                    });
                    final GestureDetector gd = new GestureDetector(act, new GestureDetector.SimpleOnGestureListener() {
                        @Override
                        public boolean onDown(MotionEvent e) {
                            return true;
                        }

                        @Override
                        public boolean onSingleTapConfirmed(MotionEvent e) {
                            sPendingPlaying = !sPendingPlaying;
                            updateVideoPauseIcon();
                            try {
                                nativeOnMediaAction(NATIVE_TOGGLE, 0);
                            } catch (Throwable ignored) {
                            }
                            return true;
                        }

                        @Override
                        public boolean onDoubleTap(MotionEvent e) {
                            int vw = sVideoOverlayRoot != null ? sVideoOverlayRoot.getWidth() : 0;
                            float tx = e.getX();
                            int secs = Math.max(5, Math.min(60, sSeekSeconds));
                            long deltaMs = secs * 1000L;
                            if (vw > 0 && tx < vw * 0.45f) {
                                long basePos = (sPendingPosMs > 0) ? sPendingPosMs : currentPosMs;
                                long target = Math.max(0L, basePos - deltaMs);
                                sPendingPosMs = target;
                                currentPosMs = target;
                                try {
                                    nativeOnMediaAction(NATIVE_SEEK, target);
                                } catch (Throwable ignored) {
                                }
                                if (sMediaPlayer != null && sVideoPrepared) {
                                    seekMediaPlayerTo(sMediaPlayer, target, true);
                                }
                                showVideoSeekBadge(act, "\u00AB\u00AB -" + secs + " s", false);
                            } else if (vw > 0 && tx > vw * 0.55f) {
                                long basePos = (sPendingPosMs > 0) ? sPendingPosMs : currentPosMs;
                                long target = (currentDurMs > 0)
                                        ? Math.min(currentDurMs, basePos + deltaMs)
                                        : (basePos + deltaMs);
                                sPendingPosMs = target;
                                currentPosMs = target;
                                try {
                                    nativeOnMediaAction(NATIVE_SEEK, target);
                                } catch (Throwable ignored) {
                                }
                                if (sMediaPlayer != null && sVideoPrepared) {
                                    seekMediaPlayerTo(sMediaPlayer, target, true);
                                }
                                showVideoSeekBadge(act, "+" + secs + " s \u00BB\u00BB", true);
                            } else {
                                sPendingPlaying = !sPendingPlaying;
                                updateVideoPauseIcon();
                                try {
                                    nativeOnMediaAction(NATIVE_TOGGLE, 0);
                                } catch (Throwable ignored) {
                                }
                            }
                            return true;
                        }
                    });
                    sVideoOverlayRoot.setOnTouchListener((v, ev) -> gd.onTouchEvent(ev));

                    sVideoTextureView = new TextureView(act);
                    sVideoTextureView.setSurfaceTextureListener(new TextureView.SurfaceTextureListener() {
                        @Override
                        public void onSurfaceTextureAvailable(SurfaceTexture st, int w, int h) {
                            if (sVideoSurface != null) {
                                try {
                                    sVideoSurface.release();
                                } catch (Throwable ignored) {
                                }
                            }
                            sVideoSurface = new Surface(st);
                            if (sMediaPlayer != null) {
                                try {
                                    sMediaPlayer.setSurface(sVideoSurface);
                                } catch (Throwable ignored) {
                                }
                            } else if (sVideoWantedVisible && sVideoUrl != null && !sVideoUrl.isEmpty()) {
                                prepareNativeMediaPlayer(act, sVideoUrl, sVideoUserAgent);
                            }
                            applyVideoAspectTransform();
                        }

                        @Override
                        public void onSurfaceTextureSizeChanged(SurfaceTexture st, int w, int h) {
                            applyVideoAspectTransform();
                        }

                        @Override
                        public boolean onSurfaceTextureDestroyed(SurfaceTexture st) {
                            if (sMediaPlayer != null) {
                                try {
                                    sMediaPlayer.setSurface(null);
                                } catch (Throwable ignored) {
                                }
                            }
                            if (sVideoSurface != null) {
                                try {
                                    sVideoSurface.release();
                                } catch (Throwable ignored) {
                                }
                                sVideoSurface = null;
                            }
                            return true;
                        }

                        @Override
                        public void onSurfaceTextureUpdated(SurfaceTexture st) {
                            if (sVideoPrepared) {
                                sVideoRenderedFirstFrame = true;
                                if (sVideoSpinner != null && sVideoSpinner.getVisibility() != View.GONE) {
                                    sVideoSpinner.setVisibility(View.GONE);
                                }
                            }
                        }
                    });
                    sVideoOverlayRoot.addView(sVideoTextureView, new FrameLayout.LayoutParams(
                            FrameLayout.LayoutParams.MATCH_PARENT,
                            FrameLayout.LayoutParams.MATCH_PARENT
                    ));

                    sVideoSpinner = new ProgressBar(act);
                    if (Build.VERSION.SDK_INT >= 21) {
                        sVideoSpinner.setIndeterminateTintList(ColorStateList.valueOf(0xFFFFFFFF));
                    }
                    FrameLayout.LayoutParams spLp = new FrameLayout.LayoutParams(
                            FrameLayout.LayoutParams.WRAP_CONTENT,
                            FrameLayout.LayoutParams.WRAP_CONTENT
                    );
                    spLp.gravity = Gravity.CENTER;
                    sVideoOverlayRoot.addView(sVideoSpinner, spLp);

                    sVideoSeekBadge = new TextView(act);
                    sVideoSeekBadge.setTextColor(0xFFFFFFFF);
                    sVideoSeekBadge.setTextSize(14f);
                    sVideoSeekBadge.setTypeface(Typeface.DEFAULT_BOLD);
                    final float badgeDensity = act.getResources().getDisplayMetrics().density;
                    GradientDrawable badgeBg = new GradientDrawable();
                    badgeBg.setColor(0xCC121216);
                    badgeBg.setCornerRadius(18f * badgeDensity);
                    sVideoSeekBadge.setBackground(badgeBg);
                    int padH = (int) (14f * badgeDensity);
                    int padV = (int) (7f * badgeDensity);
                    sVideoSeekBadge.setPadding(padH, padV, padH, padV);
                    sVideoSeekBadge.setVisibility(View.GONE);
                    sVideoOverlayRoot.addView(sVideoSeekBadge);

                    int pauseSz = (int) (56f * badgeDensity);
                    sVideoPauseIcon = new View(act) {
                        private final Paint triPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
                        private final Path triPath = new Path();
                        {
                            triPaint.setColor(0xFFFFFFFF);
                            triPaint.setStyle(Paint.Style.FILL);
                        }

                        @Override
                        protected void onDraw(Canvas canvas) {
                            super.onDraw(canvas);
                            float w = getWidth();
                            float h = getHeight();
                            triPath.reset();
                            triPath.moveTo(w * 0.39f, h * 0.28f);
                            triPath.lineTo(w * 0.39f, h * 0.72f);
                            triPath.lineTo(w * 0.72f, h * 0.50f);
                            triPath.close();
                            canvas.drawPath(triPath, triPaint);
                        }
                    };
                    GradientDrawable pauseBg = new GradientDrawable();
                    pauseBg.setShape(GradientDrawable.OVAL);
                    pauseBg.setColor(0xB80A0A0E);
                    sVideoPauseIcon.setBackground(pauseBg);
                    sVideoPauseIcon.setVisibility(View.GONE);
                    FrameLayout.LayoutParams pauseLp = new FrameLayout.LayoutParams(pauseSz, pauseSz);
                    pauseLp.gravity = Gravity.CENTER;
                    sVideoOverlayRoot.addView(sVideoPauseIcon, pauseLp);

                    sVideoFullscreenTitle = new TextView(act);
                    sVideoFullscreenTitle.setTextColor(0xFFFFFFFF);
                    sVideoFullscreenTitle.setTextSize(14f);
                    sVideoFullscreenTitle.setTypeface(Typeface.DEFAULT_BOLD);
                    sVideoFullscreenTitle.setSingleLine(true);
                    sVideoFullscreenTitle.setEllipsize(TextUtils.TruncateAt.END);
                    GradientDrawable titleBg = new GradientDrawable();
                    titleBg.setColor(0x88101014);
                    titleBg.setCornerRadius(10f * badgeDensity);
                    sVideoFullscreenTitle.setBackground(titleBg);
                    int tPadH = (int) (10f * badgeDensity);
                    int tPadV = (int) (5f * badgeDensity);
                    sVideoFullscreenTitle.setPadding(tPadH, tPadV, tPadH, tPadV);
                    sVideoFullscreenTitle.setVisibility(View.GONE);
                    FrameLayout.LayoutParams titleLp = new FrameLayout.LayoutParams(
                            FrameLayout.LayoutParams.WRAP_CONTENT,
                            FrameLayout.LayoutParams.WRAP_CONTENT
                    );
                    titleLp.gravity = Gravity.TOP | Gravity.START;
                    titleLp.setMargins(
                            (int) (18f * badgeDensity),
                            (int) (14f * badgeDensity),
                            (int) (68f * badgeDensity),
                            0
                    );
                    sVideoOverlayRoot.addView(sVideoFullscreenTitle, titleLp);

                    int fsSz = (int) (36f * badgeDensity);
                    sVideoFullscreenBtn = new TextView(act);
                    sVideoFullscreenBtn.setText("\u26F6");
                    sVideoFullscreenBtn.setTextColor(0xFFFFFFFF);
                    sVideoFullscreenBtn.setTextSize(18f);
                    sVideoFullscreenBtn.setTypeface(Typeface.DEFAULT_BOLD);
                    sVideoFullscreenBtn.setGravity(Gravity.CENTER);
                    GradientDrawable fsBg = new GradientDrawable();
                    fsBg.setColor(0x99121216);
                    fsBg.setCornerRadius(10f * badgeDensity);
                    sVideoFullscreenBtn.setBackground(fsBg);
                    sVideoFullscreenBtn.setClickable(true);
                    sVideoFullscreenBtn.setOnClickListener(v -> setVideoFullscreen(act, !sVideoFullscreen));
                    FrameLayout.LayoutParams fsLp = new FrameLayout.LayoutParams(fsSz, fsSz);
                    fsLp.gravity = Gravity.BOTTOM | Gravity.END;
                    int fsMargin = (int) (10f * badgeDensity);
                    fsLp.setMargins(0, 0, fsMargin, fsMargin);
                    sVideoOverlayRoot.addView(sVideoFullscreenBtn, fsLp);

                    sVideoParams = new WindowManager.LayoutParams(
                            width,
                            height,
                            wx,
                            wy,
                            WindowManager.LayoutParams.TYPE_APPLICATION_PANEL,
                            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE
                                    | WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL
                                    | WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN
                                    | WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS
                                    | WindowManager.LayoutParams.FLAG_HARDWARE_ACCELERATED,
                            PixelFormat.TRANSLUCENT
                    );
                    sVideoParams.token = token;
                    sVideoParams.gravity = Gravity.TOP | Gravity.START;
                    if (Build.VERSION.SDK_INT >= 28) {
                        sVideoParams.layoutInDisplayCutoutMode =
                                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
                    }
                    try {
                        wm.addView(sVideoOverlayRoot, sVideoParams);
                        sVideoAdded = true;
                    } catch (Throwable addErr) {
                        Log.e("YrPlaybackService", "WindowManager.addView failed", addErr);
                        sVideoOverlayRoot = null;
                        sVideoTextureView = null;
                        sVideoSpinner = null;
                        sVideoSeekBadge = null;
                        sVideoPauseIcon = null;
                        sVideoFullscreenBtn = null;
                        sVideoFullscreenTitle = null;
                        sVideoParams = null;
                        sVideoAdded = false;
                        return;
                    }
                } else if (sVideoAdded && sVideoParams != null) {
                    if (!sVideoFullscreen) {
                        sVideoParams.token = token;
                        sVideoParams.x = wx;
                        sVideoParams.y = wy;
                        sVideoParams.width = width;
                        sVideoParams.height = height;
                        try {
                            wm.updateViewLayout(sVideoOverlayRoot, sVideoParams);
                        } catch (Throwable updErr) {
                            Log.w("YrPlaybackService", "WindowManager.updateViewLayout failed: " + updErr);
                        }
                    }
                }

                sVideoWantedVisible = true;
                sPendingPlaying = playing;
                sPendingPosMs = startMs;
                sPendingSpeed = speed;
                updateVideoPauseIcon();
                if (!sActivityPaused && sVideoOverlayRoot != null) {
                    sVideoOverlayRoot.setVisibility(View.VISIBLE);
                }

                boolean urlChanged = !videoUrl.equals(sVideoUrl);
                if (urlChanged || sMediaPlayer == null) {
                    sVideoUrl = videoUrl;
                    sVideoFallbackUrl = fallbackUrl;
                    sVideoUserAgent = userAgent;
                    sTriedFallback = false;
                    prepareNativeMediaPlayer(act, videoUrl, userAgent);
                } else if (sVideoPrepared && sMediaPlayer != null) {
                    try {
                        if (playing && !sActivityPaused && !sMediaPlayer.isPlaying()) {
                            sMediaPlayer.start();
                        } else if (!playing && sMediaPlayer.isPlaying()) {
                            sMediaPlayer.pause();
                        }
                        if (Math.abs(sMediaPlayer.getCurrentPosition() - startMs) > 2200) {
                            seekMediaPlayerTo(sMediaPlayer, startMs, false);
                        }
                    } catch (Throwable ignored) {
                    }
                }
            } catch (Throwable t) {
                Log.w("YrPlaybackService", "attachNativeVideo failed: " + t);
            }
        });
    }

    public static void syncNativeVideo(final Context ctx, final boolean playing, final long posMs, final float speed) {
        new Handler(Looper.getMainLooper()).post(() -> {
            try {
                sPendingPlaying = playing;
                sPendingPosMs = posMs;
                sPendingSpeed = speed;
                updateVideoPauseIcon();
                if (sVideoFullscreen && sVideoFullscreenTitle != null) {
                    String artist = (currentArtist != null && !currentArtist.isEmpty())
                            ? ("  \u2022  " + currentArtist)
                            : "";
                    sVideoFullscreenTitle.setText(currentTitle + artist);
                }
                MediaPlayer mp = sMediaPlayer;
                if (mp == null || !sVideoPrepared || sActivityPaused || !sVideoWantedVisible) {
                    return;
                }
                if (playing && !mp.isPlaying()) {
                    mp.start();
                } else if (!playing && mp.isPlaying()) {
                    mp.pause();
                }
                long cur = mp.getCurrentPosition();
                long now = SystemClock.elapsedRealtime();
                boolean waitingFirstFrame = !sVideoRenderedFirstFrame && (now - sLastSeekStartedElapsedMs) < 5000L;
                if (!waitingFirstFrame && Math.abs(cur - posMs) > 2200) {
                    seekMediaPlayerTo(mp, posMs, false);
                }
                if (Math.abs(sLastAppliedSpeed - speed) > 0.03f) {
                    applySpeedToMediaPlayer(mp, speed);
                }
            } catch (Throwable ignored) {
            }
        });
    }

    public static void hideEmbeddedVideo(final Context ctx) {
        new Handler(Looper.getMainLooper()).post(() -> {
            try {
                sVideoWantedVisible = false;
                if (sVideoFullscreen) {
                    setVideoFullscreen(null, false);
                }
                if (sMediaPlayer != null && sVideoPrepared) {
                    try {
                        if (sMediaPlayer.isPlaying()) {
                            sMediaPlayer.pause();
                        }
                    } catch (Throwable ignored) {
                    }
                }
                if (sVideoOverlayRoot != null) {
                    sVideoOverlayRoot.setVisibility(View.GONE);
                }
            } catch (Throwable ignored) {
            }
        });
    }

    public static void onActivityPaused(final Context ctx) {
        new Handler(Looper.getMainLooper()).post(() -> {
            try {
                sActivityPaused = true;
                if (sMediaPlayer != null && sVideoPrepared) {
                    try {
                        if (sMediaPlayer.isPlaying()) {
                            sMediaPlayer.pause();
                        }
                    } catch (Throwable ignored) {
                    }
                }
                if (sVideoOverlayRoot != null) {
                    sVideoOverlayRoot.setVisibility(View.GONE);
                }
            } catch (Throwable ignored) {
            }
        });
    }

    public static void onActivityResumed(final Context ctx) {
        new Handler(Looper.getMainLooper()).post(() -> {
            try {
                sActivityPaused = false;
                if (sVideoWantedVisible && sVideoOverlayRoot != null) {
                    sVideoOverlayRoot.setVisibility(View.VISIBLE);
                    if (sMediaPlayer != null && sVideoPrepared && sPendingPlaying && !sMediaPlayer.isPlaying()) {
                        try {
                            sMediaPlayer.start();
                        } catch (Throwable ignored) {
                        }
                    }
                }
            } catch (Throwable ignored) {
            }
        });
    }

    public static void destroyEmbeddedVideo(final Context ctx) {
        new Handler(Looper.getMainLooper()).post(() -> {
            try {
                sVideoWantedVisible = false;
                if (sVideoFullscreen) {
                    setVideoFullscreen(null, false);
                }
                sVideoUrl = null;
                sVideoFallbackUrl = null;
                sVideoUserAgent = null;
                releaseMediaPlayerLocked();
                if (sVideoSurface != null) {
                    try {
                        sVideoSurface.release();
                    } catch (Throwable ignored) {
                    }
                    sVideoSurface = null;
                }
                if (sVideoAdded && sVideoOverlayRoot != null && ctx != null) {
                    try {
                        WindowManager wm = (WindowManager) ctx.getSystemService(Context.WINDOW_SERVICE);
                        if (wm != null) {
                            wm.removeViewImmediate(sVideoOverlayRoot);
                        }
                    } catch (Throwable ignored) {
                    }
                }
                sVideoAdded = false;
                sVideoOverlayRoot = null;
                sVideoTextureView = null;
                sVideoSpinner = null;
                sVideoSeekBadge = null;
                sVideoPauseIcon = null;
                sVideoFullscreenBtn = null;
                sVideoFullscreenTitle = null;
                sVideoActivityRef = null;
                sVideoFullscreen = false;
                if (sHideSeekBadgeRunnable != null) {
                    sUiHandler.removeCallbacks(sHideSeekBadgeRunnable);
                    sHideSeekBadgeRunnable = null;
                }
                sVideoParams = null;
            } catch (Throwable ignored) {
            }
        });
    }

    public static void openGoogleLogin(final Context ctx) {
        Log.i("YR_LOGIN", "openGoogleLogin called");
        if (ctx == null) return;
        new Handler(Looper.getMainLooper()).post(() -> {
            try {
                final Dialog dialog = new Dialog(ctx, android.R.style.Theme_Black_NoTitleBar_Fullscreen);
                dialog.setCancelable(true);
                LinearLayout root = new LinearLayout(ctx);
                root.setOrientation(LinearLayout.VERTICAL);
                root.setBackgroundColor(0xFF0A0A0A);

                LinearLayout bar = new LinearLayout(ctx);
                bar.setOrientation(LinearLayout.HORIZONTAL);
                bar.setBackgroundColor(0xFF181818);
                bar.setPadding(24, 18, 24, 18);

                TextView title = new TextView(ctx);
                title.setText("Conectar Google / YT Music");
                title.setTextColor(0xFFFFFFFF);
                title.setTextSize(16);
                LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f);
                bar.addView(title, tp);

                Button doneBtn = new Button(ctx);
                doneBtn.setText("Listo");
                bar.addView(doneBtn);

                Button closeBtn = new Button(ctx);
                closeBtn.setText("Cerrar");
                bar.addView(closeBtn);
                root.addView(bar, new LinearLayout.LayoutParams(
                        LinearLayout.LayoutParams.MATCH_PARENT,
                        LinearLayout.LayoutParams.WRAP_CONTENT
                ));

                final WebView wv = new WebView(ctx);
                WebSettings ws = wv.getSettings();
                ws.setJavaScriptEnabled(true);
                ws.setDomStorageEnabled(true);
                ws.setDatabaseEnabled(true);
                ws.setSupportMultipleWindows(false);
                String defaultUa = WebSettings.getDefaultUserAgent(ctx);
                String cleanUa = defaultUa != null
                        ? defaultUa.replace("; wv)", ")").replace(" wv)", ")")
                        : "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36";
                ws.setUserAgentString(cleanUa);

                CookieManager cm = CookieManager.getInstance();
                cm.setAcceptCookie(true);
                cm.setAcceptThirdPartyCookies(wv, true);

                final java.util.concurrent.atomic.AtomicBoolean sent = new java.util.concurrent.atomic.AtomicBoolean(false);
                final Runnable tryCapture = () -> {
                    if (sent.get()) return;
                    String merged = mergeYoutubeCookies();
                    if (hasGoogleAuthCookies(merged)) {
                        if (sent.compareAndSet(false, true)) {
                            Log.i("YR_LOGIN", "Captured Google cookies len=" + merged.length());
                            nativeOnGoogleCookie(merged);
                            try {
                                wv.stopLoading();
                                wv.destroy();
                            } catch (Throwable ignored) {
                            }
                            dialog.dismiss();
                        }
                    }
                };

                doneBtn.setOnClickListener(v -> {
                    String merged = mergeYoutubeCookies();
                    if (hasGoogleAuthCookies(merged)) {
                        tryCapture.run();
                    } else {
                        title.setText("Inicia sesión en Google primero");
                    }
                });

                closeBtn.setOnClickListener(v -> {
                    try {
                        wv.stopLoading();
                        wv.destroy();
                    } catch (Throwable ignored) {
                    }
                    dialog.dismiss();
                });

                dialog.setOnKeyListener((d, keyCode, event) -> {
                    if (keyCode == android.view.KeyEvent.KEYCODE_BACK
                            && event.getAction() == android.view.KeyEvent.ACTION_UP) {
                        if (wv.canGoBack()) {
                            wv.goBack();
                        } else {
                            try {
                                wv.stopLoading();
                                wv.destroy();
                            } catch (Throwable ignored) {
                            }
                            d.dismiss();
                        }
                        return true;
                    }
                    return false;
                });

                wv.setWebViewClient(new WebViewClient() {
                    @Override
                    public void doUpdateVisitedHistory(WebView view, String url, boolean isReload) {
                        super.doUpdateVisitedHistory(view, url, isReload);
                        Log.i("YR_LOGIN", "Visited: " + url);
                        if (url != null && url.startsWith("https://music.youtube.com")) {
                            tryCapture.run();
                        }
                    }

                    @Override
                    public void onPageFinished(WebView view, String url) {
                        Log.i("YR_LOGIN", "Finished: " + url);
                        if (url != null && url.startsWith("https://music.youtube.com")) {
                            tryCapture.run();
                        }
                    }
                });

                root.addView(wv, new LinearLayout.LayoutParams(
                        LinearLayout.LayoutParams.MATCH_PARENT,
                        LinearLayout.LayoutParams.MATCH_PARENT
                ));
                dialog.setContentView(root);
                dialog.show();
                wv.loadUrl("https://accounts.google.com/ServiceLogin?service=youtube&uilel=3&passive=true&continue=https%3A%2F%2Fmusic.youtube.com%2F");
            } catch (Throwable t) {
                Log.e("YR_LOGIN", "openGoogleLogin failed", t);
            }
        });
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
        activeInstance = this;
        lastMetaKey = "";
        lastNotifPlaying = null;
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

    private void publishPlaybackState() {
        MediaSession session = mediaSession;
        if (session == null) return;
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
        session.setPlaybackState(pbState);
    }

    private void refreshMediaSessionAndNotification() {
        if (wakeLock != null) {
            if (currentPlaying && !wakeLock.isHeld()) {
                wakeLock.acquire(4 * 60 * 60 * 1000L);
            } else if (!currentPlaying && wakeLock.isHeld()) {
                wakeLock.release();
            }
        }

        String metaKey = buildMetaKey();
        if (mediaSession != null && !metaKey.equals(lastMetaKey)) {
            MediaMetadata.Builder meta = new MediaMetadata.Builder()
                    .putString(MediaMetadata.METADATA_KEY_TITLE, currentTitle)
                    .putString(MediaMetadata.METADATA_KEY_ARTIST, currentArtist);
            if (currentDurMs > 0) {
                meta.putLong(MediaMetadata.METADATA_KEY_DURATION, currentDurMs);
            }
            Bitmap art = currentArtworkBitmap;
            if (art != null && !art.isRecycled()) {
                meta.putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, art);
                meta.putBitmap(MediaMetadata.METADATA_KEY_ART, art);
                meta.putBitmap(MediaMetadata.METADATA_KEY_DISPLAY_ICON, art);
            }
            mediaSession.setMetadata(meta.build());
            lastMetaKey = metaKey;
        }
        publishPlaybackState();
        lastNotifPlaying = currentPlaying;

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

        Intent openIntent = new Intent(this, YrMainActivity.class);
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
                .setVisibility(Notification.VISIBILITY_PUBLIC);

        Bitmap art = currentArtworkBitmap;
        if (art != null && !art.isRecycled()) {
            builder.setLargeIcon(art);
        }

        builder.addAction(android.R.drawable.ic_media_previous, "Anterior", prevPi)
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
        if (Build.VERSION.SDK_INT >= 26) {
            return PendingIntent.getForegroundService(this, reqCode, i, piFlags);
        }
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
        activeInstance = null;
        lastMetaKey = "";
        lastNotifPlaying = null;
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
