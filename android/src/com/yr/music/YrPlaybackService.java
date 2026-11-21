package com.yr.music;

import android.app.Dialog;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Typeface;
import android.media.MediaMetadata;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.net.Uri;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.PowerManager;
import android.text.TextUtils;
import android.util.Log;
import android.view.Gravity;
import android.view.KeyEvent;
import android.webkit.CookieManager;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
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

    public static void openVideo(final Context ctx, final String videoUrl) {
        openVideo(ctx, videoUrl, "", "");
    }

    public static void openVideo(final Context ctx, final String videoUrlOrId, final String title, final String artist) {
        if (ctx == null || videoUrlOrId == null) return;
        new Handler(Looper.getMainLooper()).post(() -> {
            try {
                // Pause background audio so it doesn't double-play with the music video
                try {
                    nativeOnMediaAction(NATIVE_PAUSE, 0);
                } catch (Throwable ignored) {
                }

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

                final Dialog dialog = new Dialog(ctx, android.R.style.Theme_Black_NoTitleBar_Fullscreen);
                dialog.setCancelable(true);

                LinearLayout root = new LinearLayout(ctx);
                root.setOrientation(LinearLayout.VERTICAL);
                root.setBackgroundColor(0xFF000000);

                // Top Bar
                LinearLayout topBar = new LinearLayout(ctx);
                topBar.setOrientation(LinearLayout.HORIZONTAL);
                topBar.setBackgroundColor(0xFF101014);
                topBar.setPadding(24, 20, 24, 20);
                topBar.setGravity(Gravity.CENTER_VERTICAL);

                // Close button "✕"
                TextView closeBtn = new TextView(ctx);
                closeBtn.setText("✕");
                closeBtn.setTextColor(0xFFFFFFFF);
                closeBtn.setTextSize(20);
                closeBtn.setPadding(12, 8, 20, 8);
                topBar.addView(closeBtn);

                // Title & Artist container
                LinearLayout infoCol = new LinearLayout(ctx);
                infoCol.setOrientation(LinearLayout.VERTICAL);
                infoCol.setPadding(8, 0, 8, 0);

                TextView tvTitle = new TextView(ctx);
                String displayTitle = (title != null && !title.trim().isEmpty()) ? title : "Video Musical";
                tvTitle.setText(displayTitle);
                tvTitle.setTextColor(0xFFFFFFFF);
                tvTitle.setTextSize(15);
                tvTitle.setTypeface(Typeface.DEFAULT_BOLD);
                tvTitle.setSingleLine(true);
                tvTitle.setEllipsize(TextUtils.TruncateAt.END);
                infoCol.addView(tvTitle);

                TextView tvArtist = new TextView(ctx);
                String displayArtist = (artist != null && !artist.trim().isEmpty()) ? artist : "YR Music • Reproductor Integrado";
                tvArtist.setText(displayArtist);
                tvArtist.setTextColor(0xFFAAAAAA);
                tvArtist.setTextSize(12);
                tvArtist.setSingleLine(true);
                tvArtist.setEllipsize(TextUtils.TruncateAt.END);
                infoCol.addView(tvArtist);

                LinearLayout.LayoutParams infoParams = new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1.0f);
                topBar.addView(infoCol, infoParams);

                // Button to close and return to audio
                Button returnBtn = new Button(ctx);
                returnBtn.setText("Música");
                returnBtn.setTextColor(0xFFFFFFFF);
                returnBtn.setTextSize(12);
                returnBtn.setBackgroundColor(0xFF24242A);
                topBar.addView(returnBtn, new LinearLayout.LayoutParams(
                        LinearLayout.LayoutParams.WRAP_CONTENT,
                        LinearLayout.LayoutParams.WRAP_CONTENT
                ));

                root.addView(topBar, new LinearLayout.LayoutParams(
                        LinearLayout.LayoutParams.MATCH_PARENT,
                        LinearLayout.LayoutParams.WRAP_CONTENT
                ));

                // WebView Video Frame
                FrameLayout container = new FrameLayout(ctx);
                container.setBackgroundColor(0xFF000000);

                final WebView wv = new WebView(ctx);
                wv.setBackgroundColor(0xFF000000);
                WebSettings ws = wv.getSettings();
                ws.setJavaScriptEnabled(true);
                ws.setDomStorageEnabled(true);
                ws.setMediaPlaybackRequiresUserGesture(false);
                ws.setSupportMultipleWindows(false);
                ws.setAllowFileAccess(false);
                ws.setAllowContentAccess(false);

                String defaultUa = WebSettings.getDefaultUserAgent(ctx);
                String cleanUa = defaultUa != null
                        ? defaultUa.replace("; wv)", ")").replace(" wv)", ")")
                        : "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36";
                ws.setUserAgentString(cleanUa);

                wv.setWebChromeClient(new WebChromeClient());
                wv.setWebViewClient(new WebViewClient() {
                    @Override
                    public boolean shouldOverrideUrlLoading(WebView view, String url) {
                        return false;
                    }

                    @Override
                    public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                        return false;
                    }

                    @Override
                    public void onPageFinished(WebView view, String url) {
                        super.onPageFinished(view, url);
                        // Hide banners, recommendations and comments to present a pure video player
                        String js = "(function() {"
                                + "  var style = document.createElement('style');"
                                + "  style.type = 'text/css';"
                                + "  style.innerHTML = 'header, ytm-mobile-topbar-renderer, #masthead, ytm-pivot-bar-renderer, .slim-video-metadata-actions, ytm-item-section-renderer, #related, #comments { display: none !important; } html, body { background: #000000 !important; color: #ffffff !important; }';"
                                + "  document.head.appendChild(style);"
                                + "  var v = document.querySelector('video');"
                                + "  if (v) { v.play(); }"
                                + "})();";
                        view.evaluateJavascript(js, null);
                    }
                });

                wv.loadUrl("https://m.youtube.com/watch?v=" + videoId);

                container.addView(wv, new FrameLayout.LayoutParams(
                        FrameLayout.LayoutParams.MATCH_PARENT,
                        FrameLayout.LayoutParams.MATCH_PARENT
                ));

                root.addView(container, new LinearLayout.LayoutParams(
                        LinearLayout.LayoutParams.MATCH_PARENT,
                        0,
                        1.0f
                ));

                final Runnable cleanup = () -> {
                    try {
                        wv.stopLoading();
                        wv.loadUrl("about:blank");
                        wv.onPause();
                        wv.pauseTimers();
                        wv.destroy();
                    } catch (Throwable ignored) {
                    }
                    try {
                        dialog.dismiss();
                    } catch (Throwable ignored) {
                    }
                };

                closeBtn.setOnClickListener(v -> cleanup.run());
                returnBtn.setOnClickListener(v -> cleanup.run());
                dialog.setOnCancelListener(d -> cleanup.run());

                dialog.setOnKeyListener((d, keyCode, event) -> {
                    if (keyCode == KeyEvent.KEYCODE_BACK && event.getAction() == KeyEvent.ACTION_UP) {
                        cleanup.run();
                        return true;
                    }
                    return false;
                });

                dialog.setContentView(root);
                dialog.show();

            } catch (Throwable t) {
                Log.w("YrPlaybackService", "openVideo in-app failed: " + t);
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
