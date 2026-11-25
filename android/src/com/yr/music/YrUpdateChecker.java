package com.yr.music;

import android.app.Activity;
import android.app.AlertDialog;
import android.app.Dialog;
import android.content.Context;
import android.content.DialogInterface;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.content.res.ColorStateList;
import android.graphics.Typeface;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.GradientDrawable;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.Uri;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.provider.Settings;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.CompoundButton;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.lang.ref.WeakReference;
import java.net.HttpURLConnection;
import java.net.URL;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.json.JSONArray;
import org.json.JSONObject;

/**
 * In-app GitHub Releases updater modeled after OpenManager's UpdateChecker,
 * implemented with pure Android platform APIs and Java 8 compatibility.
 */
public final class YrUpdateChecker {
    public static final String GITHUB_REPO = "uamo11/YR";
    public static final String GITHUB_LATEST_URL =
            "https://api.github.com/repos/" + GITHUB_REPO + "/releases/latest";
    public static final String GITHUB_RELEASES_LIST_URL =
            "https://api.github.com/repos/" + GITHUB_REPO + "/releases?per_page=20";
    public static final String GITHUB_RELEASES_ATOM_URL =
            "https://github.com/" + GITHUB_REPO + "/releases.atom";

    private static final String PREFS_NAME = "yr_updater_prefs";
    private static final String PREF_AUTO_CHECK = "auto_check_updates";
    private static final String PREF_LAST_CHECK_MS = "last_check_ms";
    private static final String PREF_SKIPPED_VERSION = "skipped_version";
    private static final String PREF_CACHED_ETAG = "cached_etag";
    private static final String PREF_CACHED_RELEASE_JSON = "cached_release_json";
    private static final String PREF_PENDING_APK_PATH = "pending_update_apk_path";
    private static final String PREF_PENDING_VERSION = "pending_update_version";

    private static final long AUTO_CHECK_COOLDOWN_MS = 2L * 60L * 60L * 1000L;
    private static final long AUTO_STARTUP_DELAY_MS = 2500L;
    private static final long MIN_CLEANUP_AGE_MS = 15000L;
    private static final long STALE_TMP_AGE_MS = 3600000L;
    private static final int DOWNLOAD_BUFFER_SIZE = 65536;

    private static final int COLOR_DIALOG_BG = 0xFF18181C;
    private static final int COLOR_CARD_BG = 0xFF222228;
    private static final int COLOR_BORDER = 0xFF34343C;
    private static final int COLOR_TEXT_PRIMARY = 0xFFF5F5F5;
    private static final int COLOR_TEXT_SECONDARY = 0xFFA5A5AD;
    private static final int COLOR_ACCENT_RED = 0xFFFF0033;
    private static final int COLOR_GREEN = 0xFF2ECC71;

    private static final AtomicBoolean sIsChecking = new AtomicBoolean(false);
    private static volatile boolean sIsDownloading = false;
    private static volatile boolean sStartupChecked = false;
    private static volatile boolean sCleanedUpThisRun = false;
    private static volatile boolean sWaitingUnknownSources = false;
    private static volatile File sPendingInstallFile = null;
    private static volatile WeakReference<Activity> sActivityRef = null;
    private static volatile WeakReference<Dialog> sActiveDialogRef = null;
    private static final Handler sUiHandler = new Handler(Looper.getMainLooper());

    private YrUpdateChecker() {}

    public static final class ReleaseAsset {
        public final String name;
        public final long size;
        public final String downloadUrl;

        public ReleaseAsset(String name, long size, String downloadUrl) {
            this.name = name;
            this.size = size;
            this.downloadUrl = downloadUrl;
        }
    }

    public static final class ReleaseInfo {
        public final String tagName;
        public final String version;
        public final String name;
        public final String body;
        public final String htmlUrl;
        public final ReleaseAsset targetAsset;

        public ReleaseInfo(
                String tagName,
                String version,
                String name,
                String body,
                String htmlUrl,
                ReleaseAsset targetAsset
        ) {
            this.tagName = tagName;
            this.version = version;
            this.name = name;
            this.body = body;
            this.htmlUrl = htmlUrl;
            this.targetAsset = targetAsset;
        }
    }

    private static final class HttpResult {
        final int code;
        final String body;
        final String etag;

        HttpResult(int code, String body, String etag) {
            this.code = code;
            this.body = body;
            this.etag = etag;
        }
    }

    public static void onActivityResumed(final Activity activity) {
        if (activity == null) return;
        sActivityRef = new WeakReference<Activity>(activity);
        if (!sCleanedUpThisRun) {
            sCleanedUpThisRun = true;
            final Context appCtx = activity.getApplicationContext();
            new Thread(new Runnable() {
                @Override
                public void run() {
                    cleanupInstalledUpdateApks(appCtx);
                }
            }, "yr-update-startup-clean").start();
        }
        if (sWaitingUnknownSources && sPendingInstallFile != null) {
            boolean canInstall = Build.VERSION.SDK_INT < 26
                    || activity.getPackageManager().canRequestPackageInstalls();
            if (canInstall) {
                final File pending = sPendingInstallFile;
                sWaitingUnknownSources = false;
                sPendingInstallFile = null;
                sUiHandler.post(new Runnable() {
                    @Override
                    public void run() {
                        if (isActivityAlive(activity) && pending.exists()) {
                            launchApkInstallation(activity, pending);
                        }
                    }
                });
            }
        }
        if (!sStartupChecked) {
            sStartupChecked = true;
            sUiHandler.postDelayed(new Runnable() {
                @Override
                public void run() {
                    Activity act = sActivityRef != null ? sActivityRef.get() : null;
                    if (isActivityAlive(act) && isAutoCheckEnabled(act)) {
                        checkForUpdates(act, false);
                    }
                }
            }, AUTO_STARTUP_DELAY_MS);
        }
    }

    public static boolean isAutoCheckEnabled(Context ctx) {
        if (ctx == null) return true;
        return prefs(ctx).getBoolean(PREF_AUTO_CHECK, true);
    }

    public static void setAutoCheckEnabled(Context ctx, boolean enabled) {
        if (ctx == null) return;
        prefs(ctx).edit().putBoolean(PREF_AUTO_CHECK, enabled).apply();
    }

    public static void checkForUpdates(final Context ctx, final boolean isManual) {
        final Activity act = (ctx instanceof Activity)
                ? (Activity) ctx
                : (sActivityRef != null ? sActivityRef.get() : null);
        if (ctx == null) return;

        final SharedPreferences sp = prefs(ctx);
        if (!isManual) {
            if (!sp.getBoolean(PREF_AUTO_CHECK, true)) return;
            long last = sp.getLong(PREF_LAST_CHECK_MS, 0L);
            if (System.currentTimeMillis() - last < AUTO_CHECK_COOLDOWN_MS) {
                return;
            }
            if (!isActivityAlive(act) || !isNetworkAvailable(ctx)) {
                return;
            }
        }

        if (!sIsChecking.compareAndSet(false, true)) {
            if (isManual && isActivityAlive(act)) {
                sUiHandler.post(new Runnable() {
                    @Override
                    public void run() {
                        Toast.makeText(act, "Ya buscando actualizaciones...", Toast.LENGTH_SHORT).show();
                    }
                });
            }
            return;
        }

        final AtomicBoolean cancelled = new AtomicBoolean(false);
        final Dialog[] loadingHolder = new Dialog[1];
        if (isManual && isActivityAlive(act)) {
            sUiHandler.post(new Runnable() {
                @Override
                public void run() {
                    if (!isActivityAlive(act)) return;
                    Dialog d = showLoadingDialog(act, new DialogInterface.OnCancelListener() {
                        @Override
                        public void onCancel(DialogInterface dialog) {
                            cancelled.set(true);
                        }
                    });
                    loadingHolder[0] = d;
                }
            });
        }

        final Context appCtx = ctx.getApplicationContext();
        new Thread(new Runnable() {
            @Override
            public void run() {
                ReleaseInfo release = null;
                Exception error = null;
                try {
                    if (!isNetworkAvailable(appCtx)) {
                        throw new IOException("Sin conexión a internet");
                    }
                    release = fetchLatestRelease(appCtx);
                    sp.edit().putLong(PREF_LAST_CHECK_MS, System.currentTimeMillis()).apply();
                } catch (Exception e) {
                    error = e;
                } finally {
                    sIsChecking.set(false);
                }

                if (cancelled.get()) return;
                final ReleaseInfo finalRelease = release;
                final Exception finalError = error;
                sUiHandler.post(new Runnable() {
                    @Override
                    public void run() {
                        dismissSafely(loadingHolder[0]);
                        Activity targetAct = isActivityAlive(act)
                                ? act
                                : (sActivityRef != null ? sActivityRef.get() : null);
                        if (!isActivityAlive(targetAct)) return;

                        if (finalError != null || finalRelease == null) {
                            if (isManual) {
                                String msg = finalError != null && finalError.getMessage() != null
                                        ? finalError.getMessage()
                                        : "No se pudo consultar GitHub Releases.";
                                showErrorDialog(targetAct, "Error al buscar actualizaciones", msg, new Runnable() {
                                    @Override
                                    public void run() {
                                        checkForUpdates(targetAct, true);
                                    }
                                });
                            }
                            return;
                        }

                        String curVer = getCurrentVersionName(appCtx);
                        String skipped = sp.getString(PREF_SKIPPED_VERSION, null);
                        if (shouldNotifyUpdate(finalRelease.version, curVer, skipped, isManual)) {
                            showUpdateDialog(targetAct, finalRelease, curVer);
                        } else if (isManual && !isVersionNewer(finalRelease.version, curVer)) {
                            showUpToDateDialog(targetAct, curVer);
                        }
                    }
                });
            }
        }, "yr-update-check").start();
    }

    private static SharedPreferences prefs(Context ctx) {
        return ctx.getApplicationContext().getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE);
    }

    private static boolean isActivityAlive(Activity act) {
        if (act == null || act.isFinishing()) return false;
        return !act.isDestroyed();
    }

    private static String getCurrentVersionName(Context ctx) {
        try {
            PackageInfo pi = ctx.getPackageManager().getPackageInfo(ctx.getPackageName(), 0);
            if (pi != null && pi.versionName != null) {
                return pi.versionName;
            }
        } catch (Throwable ignored) {
        }
        return "0.9.22";
    }

    @SuppressWarnings("deprecation")
    private static long getCurrentVersionCode(Context ctx) {
        try {
            PackageInfo pi = ctx.getPackageManager().getPackageInfo(ctx.getPackageName(), 0);
            if (pi != null) {
                if (Build.VERSION.SDK_INT >= 28) {
                    return pi.getLongVersionCode();
                }
                return (long) pi.versionCode;
            }
        } catch (Throwable ignored) {
        }
        return 0L;
    }

    public static List<Integer> parseVersionParts(String versionStr) {
        List<Integer> out = new ArrayList<Integer>();
        if (versionStr == null) return out;
        String clean = versionStr.trim();
        if (clean.startsWith("v") || clean.startsWith("V")) {
            clean = clean.substring(1);
        }
        String[] parts = clean.split("[.\\-_]");
        for (int i = 0; i < parts.length; i++) {
            StringBuilder digits = new StringBuilder();
            String p = parts[i];
            for (int j = 0; j < p.length(); j++) {
                char c = p.charAt(j);
                if (c >= '0' && c <= '9') {
                    digits.append(c);
                }
            }
            if (digits.length() > 0) {
                try {
                    out.add(Integer.parseInt(digits.toString()));
                } catch (NumberFormatException ignored) {
                }
            }
        }
        return out;
    }

    public static boolean isVersionNewer(String latestVersion, String currentVersion) {
        List<Integer> latest = parseVersionParts(latestVersion);
        List<Integer> current = parseVersionParts(currentVersion);
        int maxLen = Math.max(latest.size(), current.size());
        for (int i = 0; i < maxLen; i++) {
            int l = i < latest.size() ? latest.get(i).intValue() : 0;
            int c = i < current.size() ? current.get(i).intValue() : 0;
            if (l > c) return true;
            if (l < c) return false;
        }
        return false;
    }

    private static boolean shouldNotifyUpdate(
            String latestVersion,
            String currentVersion,
            String skippedVersion,
            boolean isManual
    ) {
        if (!isVersionNewer(latestVersion, currentVersion)) return false;
        if (!isManual && skippedVersion != null && !skippedVersion.trim().isEmpty()) {
            if (!isVersionNewer(latestVersion, skippedVersion)) {
                return false;
            }
        }
        return true;
    }

    private static ReleaseAsset selectTargetAsset(List<ReleaseAsset> assets, String tagName) {
        for (int i = 0; i < assets.size(); i++) {
            ReleaseAsset a = assets.get(i);
            String lower = a.name.toLowerCase(Locale.US);
            if (lower.startsWith("yr-music") && lower.endsWith("-android.apk")) {
                return a;
            }
        }
        for (int i = 0; i < assets.size(); i++) {
            ReleaseAsset a = assets.get(i);
            if (a.name.toLowerCase(Locale.US).endsWith(".apk")) {
                return a;
            }
        }
        if (!assets.isEmpty()) return assets.get(0);
        String fallbackName = "yr-music-" + tagName + "-android.apk";
        String fallbackUrl = "https://github.com/" + GITHUB_REPO + "/releases/download/" + tagName + "/" + fallbackName;
        return new ReleaseAsset(fallbackName, 0L, fallbackUrl);
    }

    private static ReleaseInfo parseReleaseObject(JSONObject obj) {
        if (obj == null || obj.optBoolean("draft", false)) return null;
        String tagName = obj.optString("tag_name", "").trim();
        if (tagName.isEmpty()) return null;
        String version = (tagName.startsWith("v") || tagName.startsWith("V"))
                ? tagName.substring(1).trim()
                : tagName;
        String name = obj.optString("name", "YR Music " + tagName);
        String rawBody = obj.optString("body", "");
        String body = cleanReleaseNotes(rawBody);
        String htmlUrl = obj.optString(
                "html_url",
                "https://github.com/" + GITHUB_REPO + "/releases/tag/" + tagName
        );
        List<ReleaseAsset> assets = new ArrayList<ReleaseAsset>();
        JSONArray arr = obj.optJSONArray("assets");
        if (arr != null) {
            for (int i = 0; i < arr.length(); i++) {
                JSONObject aObj = arr.optJSONObject(i);
                if (aObj == null) continue;
                String aName = aObj.optString("name", "");
                long aSize = aObj.optLong("size", 0L);
                String dlUrl = aObj.optString("browser_download_url", "");
                if (!aName.isEmpty() && !dlUrl.isEmpty()) {
                    assets.add(new ReleaseAsset(aName, aSize, dlUrl));
                }
            }
        }
        ReleaseAsset target = selectTargetAsset(assets, tagName);
        return new ReleaseInfo(tagName, version, name, body, htmlUrl, target);
    }

    private static ReleaseInfo parseReleaseAtomFeed(String xml) {
        if (xml == null || xml.trim().isEmpty()) return null;
        Pattern entryPat = Pattern.compile("<entry>([\\s\\S]*?)</entry>");
        Pattern idPat = Pattern.compile("<id>[^<]*/([^/<]+)</id>");
        Pattern linkPat = Pattern.compile("<link[^>]*href=\"([^\"]+)\"");
        Pattern titlePat = Pattern.compile("<title>([\\s\\S]*?)</title>");
        Pattern contentPat = Pattern.compile("<content[^>]*>([\\s\\S]*?)</content>");
        Matcher m = entryPat.matcher(xml);
        ReleaseInfo best = null;
        while (m.find()) {
            String block = m.group(1);
            Matcher idM = idPat.matcher(block);
            if (!idM.find()) continue;
            String tagName = idM.group(1).trim();
            if (tagName.isEmpty()) continue;
            String version = (tagName.startsWith("v") || tagName.startsWith("V"))
                    ? tagName.substring(1).trim()
                    : tagName;
            Matcher tM = titlePat.matcher(block);
            String title = tM.find() ? tM.group(1).trim() : ("YR Music " + tagName);
            Matcher lM = linkPat.matcher(block);
            String htmlUrl = lM.find()
                    ? lM.group(1).trim()
                    : ("https://github.com/" + GITHUB_REPO + "/releases/tag/" + tagName);
            Matcher cM = contentPat.matcher(block);
            String rawHtml = cM.find() ? cM.group(1) : "";
            String body = cleanReleaseNotes(stripHtml(rawHtml));
            ReleaseAsset target = selectTargetAsset(new ArrayList<ReleaseAsset>(), tagName);
            ReleaseInfo candidate = new ReleaseInfo(tagName, version, title, body, htmlUrl, target);
            if (best == null || isVersionNewer(candidate.version, best.version)) {
                best = candidate;
            }
        }
        return best;
    }

    private static String stripHtml(String html) {
        if (html == null || html.trim().isEmpty()) return "";
        String t = html
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&")
                .replace("&quot;", "\"")
                .replace("&#39;", "'")
                .replace("&apos;", "'")
                .replace("&nbsp;", " ");
        t = t.replaceAll("(?i)<li[^>]*>", "\n- ")
                .replaceAll("(?i)<br\\s*/?>", "\n")
                .replaceAll("(?i)</p>", "\n")
                .replaceAll("</?[a-zA-Z][^>]*>", "")
                .replaceAll("\n{3,}", "\n\n")
                .trim();
        return t;
    }

    public static String cleanReleaseNotes(String raw) {
        if (raw == null || raw.trim().isEmpty()) return "";
        String section = raw.replace("\r\n", "\n").trim();
        String[] lines = section.split("\n");
        List<String> kept = new ArrayList<String>();
        boolean foundTop = false;
        for (int i = 0; i < lines.length; i++) {
            String line = lines[i];
            String tr = line.trim();
            if (!foundTop && tr.matches("(?i)^#+\\s*(YR\\s+Music\\s+)?v[0-9.]+.*")) {
                foundTop = true;
                continue;
            }
            if ((foundTop || !kept.isEmpty()) && tr.matches("^(?:---+|___+|\\*\\*\\*+|<hr\\s*/?>)$")) {
                break;
            }
            kept.add(line);
        }
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < kept.size(); i++) {
            if (i > 0) sb.append('\n');
            sb.append(kept.get(i));
        }
        String body = sb.toString().trim();
        if (body.isEmpty()) body = section;
        body = body.replaceAll("(?m)^#+\\s*(.+?):?\\s*$", "$1:")
                .replaceAll("\\*\\*([^*]+)\\*\\*", "$1")
                .replaceAll("__([^_]+)__", "$1")
                .replaceAll("`([^`]+)`", "$1")
                .replaceAll("(?m)^[ \t]{2,}[-*][ \t]+", "   \u25AB ")
                .replaceAll("(?m)^[ \t]*[-*][ \t]+", "\u2022 ")
                .replaceAll("</?[a-zA-Z][^>]*>", "")
                .replaceAll("\n{3,}", "\n\n")
                .trim();
        return body;
    }

    private static ReleaseInfo fetchLatestRelease(Context ctx) throws IOException {
        SharedPreferences sp = prefs(ctx);
        String cachedEtag = sp.getString(PREF_CACHED_ETAG, null);
        String cachedJson = sp.getString(PREF_CACHED_RELEASE_JSON, null);

        // 1. Primary: GitHub REST /releases/latest with conditional ETag caching
        try {
            String effectiveEtag = (cachedJson != null && !cachedJson.trim().isEmpty()) ? cachedEtag : null;
            HttpResult res = executeHttpGet(ctx, GITHUB_LATEST_URL, effectiveEtag);
            if (res.code == HttpURLConnection.HTTP_NOT_MODIFIED && cachedJson != null) {
                ReleaseInfo info = parseReleaseObject(new JSONObject(cachedJson));
                if (info != null) return info;
            } else if (res.code >= 200 && res.code <= 299 && !res.body.trim().isEmpty()) {
                ReleaseInfo info = parseReleaseObject(new JSONObject(res.body));
                if (info != null) {
                    SharedPreferences.Editor ed = sp.edit().putString(PREF_CACHED_RELEASE_JSON, res.body);
                    if (res.etag != null && !res.etag.trim().isEmpty()) {
                        ed.putString(PREF_CACHED_ETAG, res.etag);
                    }
                    ed.apply();
                    return info;
                }
            }
        } catch (Throwable ignored) {
        }

        // 2. Secondary: Zero-rate-limit Atom feed fallback
        try {
            HttpResult atom = executeHttpGet(ctx, GITHUB_RELEASES_ATOM_URL, null);
            if (atom.code >= 200 && atom.code <= 299 && !atom.body.trim().isEmpty()) {
                ReleaseInfo info = parseReleaseAtomFeed(atom.body);
                if (info != null) return info;
            }
        } catch (Throwable ignored) {
        }

        // 3. Tertiary: Releases list API
        HttpResult listRes = executeHttpGet(ctx, GITHUB_RELEASES_LIST_URL, null);
        if (listRes.code >= 200 && listRes.code <= 299 && !listRes.body.trim().isEmpty()) {
            try {
                JSONArray arr = new JSONArray(listRes.body);
                ReleaseInfo best = null;
                for (int i = 0; i < arr.length(); i++) {
                    ReleaseInfo cand = parseReleaseObject(arr.optJSONObject(i));
                    if (cand != null && (best == null || isVersionNewer(cand.version, best.version))) {
                        best = cand;
                    }
                }
                if (best != null) return best;
            } catch (Throwable ignored) {
            }
            return null;
        }
        throw new IOException("GitHub HTTP " + listRes.code);
    }

    private static HttpResult executeHttpGet(Context ctx, String urlStr, String etag) throws IOException {
        HttpURLConnection conn = openConnectionWithRedirects(ctx, urlStr, 0L);
        try {
            if (etag != null && !etag.trim().isEmpty()) {
                conn.disconnect();
                URL url = new URL(urlStr);
                conn = (HttpURLConnection) url.openConnection();
                conn.setConnectTimeout(12000);
                conn.setReadTimeout(18000);
                conn.setInstanceFollowRedirects(true);
                conn.setRequestProperty("Accept", "application/vnd.github.v3+json, application/atom+xml, */*");
                conn.setRequestProperty("User-Agent", "YR-Music-Android/" + getCurrentVersionName(ctx));
                conn.setRequestProperty("If-None-Match", etag);
                conn.connect();
            }
            int code = conn.getResponseCode();
            String respEtag = conn.getHeaderField("ETag");
            String body = "";
            if (code != HttpURLConnection.HTTP_NOT_MODIFIED) {
                InputStream is = (code >= 200 && code <= 299) ? conn.getInputStream() : conn.getErrorStream();
                if (is != null) {
                    body = readAllText(is);
                }
            }
            return new HttpResult(code, body, respEtag != null ? respEtag : etag);
        } finally {
            conn.disconnect();
        }
    }

    private static HttpURLConnection openConnectionWithRedirects(
            Context ctx,
            String urlStr,
            long rangeStartBytes
    ) throws IOException {
        URL url = new URL(urlStr);
        String ua = "YR-Music-Android/" + getCurrentVersionName(ctx);
        for (int redirects = 0; redirects < 6; redirects++) {
            HttpURLConnection conn = (HttpURLConnection) url.openConnection();
            conn.setInstanceFollowRedirects(true);
            conn.setConnectTimeout(15000);
            conn.setReadTimeout(30000);
            conn.setRequestProperty("User-Agent", ua);
            conn.setRequestProperty("Accept", "*/*");
            if (rangeStartBytes > 0L) {
                conn.setRequestProperty("Range", "bytes=" + rangeStartBytes + "-");
            }
            conn.connect();
            int code = conn.getResponseCode();
            if (code == HttpURLConnection.HTTP_MOVED_PERM
                    || code == HttpURLConnection.HTTP_MOVED_TEMP
                    || code == HttpURLConnection.HTTP_SEE_OTHER
                    || code == 307
                    || code == 308) {
                String loc = conn.getHeaderField("Location");
                conn.disconnect();
                if (loc == null || loc.trim().isEmpty()) {
                    throw new IOException("Redirección HTTP sin cabecera Location");
                }
                url = new URL(url, loc);
                continue;
            }
            return conn;
        }
        throw new IOException("Demasiadas redirecciones HTTP");
    }

    private static String readAllText(InputStream is) throws IOException {
        BufferedReader br = new BufferedReader(new InputStreamReader(is, "UTF-8"));
        try {
            StringBuilder sb = new StringBuilder();
            String line;
            while ((line = br.readLine()) != null) {
                sb.append(line).append('\n');
            }
            return sb.toString();
        } finally {
            try {
                br.close();
            } catch (IOException ignored) {
            }
        }
    }

    private static boolean isNetworkAvailable(Context ctx) {
        try {
            ConnectivityManager cm = (ConnectivityManager) ctx.getSystemService(Context.CONNECTIVITY_SERVICE);
            if (cm == null) return true;
            Network net = cm.getActiveNetwork();
            if (net == null) return false;
            NetworkCapabilities caps = cm.getNetworkCapabilities(net);
            return caps != null && caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET);
        } catch (Throwable ignored) {
            return true;
        }
    }

    private static File resolveUpdatesDir(Context ctx) {
        File ext = ctx.getExternalFilesDir(null);
        if (ext != null) {
            File dir = new File(ext, "updates");
            if (dir.exists() || dir.mkdirs()) {
                return dir;
            }
        }
        File cacheDir = new File(ctx.getCacheDir(), "updates");
        cacheDir.mkdirs();
        return cacheDir;
    }

    private static boolean hasZipMagic(File file) {
        FileInputStream fis = null;
        try {
            fis = new FileInputStream(file);
            byte[] h = new byte[4];
            int n = fis.read(h);
            return n == 4 && h[0] == 0x50 && h[1] == 0x4B && h[2] == 0x03 && h[3] == 0x04;
        } catch (Throwable ignored) {
            return false;
        } finally {
            if (fis != null) {
                try {
                    fis.close();
                } catch (IOException ignored) {
                }
            }
        }
    }

    private static boolean isValidDownloadedApk(Context ctx, File file, long expectedSize) {
        if (file == null || !file.exists() || !file.isFile() || file.length() <= 65536L) return false;
        if (expectedSize > 0L && file.length() != expectedSize) return false;
        if (!hasZipMagic(file)) return false;
        try {
            PackageInfo pi = ctx.getPackageManager().getPackageArchiveInfo(file.getAbsolutePath(), 0);
            return pi != null && ctx.getPackageName().equals(pi.packageName);
        } catch (Throwable ignored) {
            return false;
        }
    }

    private static File findExistingValidUpdateApk(Context ctx, ReleaseInfo info) {
        String name = info.targetAsset.name;
        long expected = info.targetAsset.size;
        File ext = ctx.getExternalFilesDir(null);
        if (ext != null) {
            File f = new File(new File(ext, "updates"), name);
            if (isValidDownloadedApk(ctx, f, expected)) return f;
        }
        File f2 = new File(new File(ctx.getCacheDir(), "updates"), name);
        if (isValidDownloadedApk(ctx, f2, expected)) return f2;
        return null;
    }

    @SuppressWarnings("deprecation")
    public static void cleanupInstalledUpdateApks(Context ctx) {
        if (ctx == null || sIsDownloading) return;
        try {
            SharedPreferences sp = prefs(ctx);
            String pendingPath = sp.getString(PREF_PENDING_APK_PATH, null);
            String pendingVer = sp.getString(PREF_PENDING_VERSION, null);
            String curVer = getCurrentVersionName(ctx);
            long curCode = getCurrentVersionCode(ctx);
            long now = System.currentTimeMillis();

            List<File> dirs = new ArrayList<File>();
            File ext = ctx.getExternalFilesDir(null);
            if (ext != null) dirs.add(new File(ext, "updates"));
            dirs.add(new File(ctx.getCacheDir(), "updates"));

            Pattern namePat = Pattern.compile("(?i)^yr-music-v?([0-9][0-9.]*).*\\.apk$");
            boolean clearedPending = false;
            for (int i = 0; i < dirs.size(); i++) {
                File d = dirs.get(i);
                if (!d.exists() || !d.isDirectory()) continue;
                File[] children = d.listFiles();
                if (children == null) continue;
                for (int j = 0; j < children.length; j++) {
                    File f = children[j];
                    if (!f.isFile()) continue;
                    long ageMs = Math.max(0L, now - f.lastModified());
                    if (ageMs < MIN_CLEANUP_AGE_MS) continue;
                    String fname = f.getName();
                    if (fname.toLowerCase(Locale.US).endsWith(".apk.tmp")) {
                        if (ageMs > STALE_TMP_AGE_MS) f.delete();
                        continue;
                    }
                    if (!fname.toLowerCase(Locale.US).endsWith(".apk")) continue;
                    boolean delete = false;
                    if (pendingPath != null && pendingPath.equals(f.getAbsolutePath()) && pendingVer != null) {
                        delete = !isVersionNewer(pendingVer, curVer);
                    } else {
                        Matcher m = namePat.matcher(fname);
                        if (m.find()) {
                            delete = !isVersionNewer(m.group(1), curVer);
                        }
                    }
                    if (!delete) continue;
                    try {
                        PackageInfo pi = ctx.getPackageManager().getPackageArchiveInfo(f.getAbsolutePath(), 0);
                        if (pi != null) {
                            long code = (Build.VERSION.SDK_INT >= 28) ? pi.getLongVersionCode() : (long) pi.versionCode;
                            if (!ctx.getPackageName().equals(pi.packageName) || code > curCode) {
                                continue;
                            }
                        }
                    } catch (Throwable ignored) {
                    }
                    if (f.delete() && pendingPath != null && pendingPath.equals(f.getAbsolutePath())) {
                        clearedPending = true;
                    }
                }
            }
            if (clearedPending) {
                sp.edit().remove(PREF_PENDING_APK_PATH).remove(PREF_PENDING_VERSION).apply();
            }
        } catch (Throwable ignored) {
        }
    }

    private static int dp(Context ctx, int value) {
        return Math.round(value * ctx.getResources().getDisplayMetrics().density);
    }

    private static GradientDrawable roundedDrawable(int fill, int stroke, int radiusDp, Context ctx) {
        GradientDrawable gd = new GradientDrawable();
        gd.setShape(GradientDrawable.RECTANGLE);
        gd.setColor(fill);
        gd.setCornerRadius(dp(ctx, radiusDp));
        gd.setStroke(Math.max(1, dp(ctx, 1)), stroke);
        return gd;
    }

    private static Dialog createDarkDialog(Activity act, View content, boolean cancelable) {
        Dialog dialog = new Dialog(act);
        dialog.requestWindowFeature(Window.FEATURE_NO_TITLE);
        dialog.setContentView(content);
        dialog.setCancelable(cancelable);
        dialog.setCanceledOnTouchOutside(cancelable);
        Window win = dialog.getWindow();
        if (win != null) {
            win.setBackgroundDrawable(new ColorDrawable(0x00000000));
            int width = (int) (act.getResources().getDisplayMetrics().widthPixels * 0.90f);
            win.setLayout(width, ViewGroup.LayoutParams.WRAP_CONTENT);
        }
        return dialog;
    }

    private static Button makeDialogButton(Context ctx, String text, int fillColor, int textColor) {
        Button b = new Button(ctx);
        b.setText(text);
        b.setAllCaps(false);
        b.setTextSize(TypedValue.COMPLEX_UNIT_SP, 13.5f);
        b.setTypeface(null, Typeface.BOLD);
        b.setTextColor(textColor);
        b.setBackground(roundedDrawable(fillColor, fillColor == 0x00000000 ? COLOR_BORDER : fillColor, 10, ctx));
        int h = dp(ctx, 10);
        int v = dp(ctx, 8);
        b.setPadding(h, v, h, v);
        b.setMinimumHeight(dp(ctx, 38));
        b.setMinimumWidth(0);
        return b;
    }

    private static Dialog showLoadingDialog(Activity act, DialogInterface.OnCancelListener onCancel) {
        LinearLayout card = new LinearLayout(act);
        card.setOrientation(LinearLayout.HORIZONTAL);
        card.setGravity(Gravity.CENTER_VERTICAL);
        card.setBackground(roundedDrawable(COLOR_DIALOG_BG, COLOR_BORDER, 16, act));
        int p = dp(act, 20);
        card.setPadding(p, p, p, p);

        ProgressBar pb = new ProgressBar(act);
        pb.setIndeterminateTintList(ColorStateList.valueOf(COLOR_ACCENT_RED));
        LinearLayout.LayoutParams pbLp = new LinearLayout.LayoutParams(dp(act, 28), dp(act, 28));
        pbLp.setMargins(0, 0, dp(act, 16), 0);
        card.addView(pb, pbLp);

        TextView tv = new TextView(act);
        tv.setText("Buscando actualizaciones de YR Music...");
        tv.setTextColor(COLOR_TEXT_PRIMARY);
        tv.setTextSize(TypedValue.COMPLEX_UNIT_SP, 15f);
        card.addView(tv);

        Dialog d = createDarkDialog(act, card, true);
        d.setOnCancelListener(onCancel);
        d.show();
        return d;
    }

    private static void showUpToDateDialog(final Activity act, String curVer) {
        LinearLayout card = new LinearLayout(act);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setBackground(roundedDrawable(COLOR_DIALOG_BG, COLOR_BORDER, 16, act));
        int p = dp(act, 20);
        card.setPadding(p, p, p, p);

        TextView title = new TextView(act);
        title.setText("YR Music está actualizado");
        title.setTextColor(COLOR_TEXT_PRIMARY);
        title.setTextSize(TypedValue.COMPLEX_UNIT_SP, 18f);
        title.setTypeface(null, Typeface.BOLD);
        card.addView(title);

        TextView msg = new TextView(act);
        msg.setText("Ya tienes instalada la última versión oficial: v" + curVer);
        msg.setTextColor(COLOR_TEXT_SECONDARY);
        msg.setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f);
        LinearLayout.LayoutParams msgLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        msgLp.setMargins(0, dp(act, 10), 0, dp(act, 16));
        card.addView(msg, msgLp);

        final Dialog d = createDarkDialog(act, card, true);
        Button ok = makeDialogButton(act, "Aceptar", COLOR_ACCENT_RED, 0xFFFFFFFF);
        ok.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                dismissSafely(d);
            }
        });
        card.addView(ok);
        d.show();
    }

    private static void showErrorDialog(
            final Activity act,
            String titleStr,
            String messageStr,
            final Runnable onRetry
    ) {
        LinearLayout card = new LinearLayout(act);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setBackground(roundedDrawable(COLOR_DIALOG_BG, COLOR_BORDER, 16, act));
        int p = dp(act, 20);
        card.setPadding(p, p, p, p);

        TextView title = new TextView(act);
        title.setText(titleStr);
        title.setTextColor(COLOR_ACCENT_RED);
        title.setTextSize(TypedValue.COMPLEX_UNIT_SP, 17f);
        title.setTypeface(null, Typeface.BOLD);
        card.addView(title);

        TextView msg = new TextView(act);
        msg.setText(messageStr);
        msg.setTextColor(COLOR_TEXT_SECONDARY);
        msg.setTextSize(TypedValue.COMPLEX_UNIT_SP, 13.5f);
        LinearLayout.LayoutParams msgLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        msgLp.setMargins(0, dp(act, 10), 0, dp(act, 16));
        card.addView(msg, msgLp);

        final Dialog d = createDarkDialog(act, card, true);
        LinearLayout row = new LinearLayout(act);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.END);

        Button cancel = makeDialogButton(act, "Cancelar", COLOR_CARD_BG, COLOR_TEXT_SECONDARY);
        cancel.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                dismissSafely(d);
            }
        });
        LinearLayout.LayoutParams cLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        cLp.setMargins(0, 0, dp(act, 8), 0);
        row.addView(cancel, cLp);

        Button retry = makeDialogButton(act, "Reintentar", COLOR_ACCENT_RED, 0xFFFFFFFF);
        retry.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                dismissSafely(d);
                if (onRetry != null) onRetry.run();
            }
        });
        row.addView(retry);
        card.addView(row);
        d.show();
    }

    private static void showUpdateDialog(
            final Activity act,
            final ReleaseInfo info,
            final String curVer
    ) {
        if (!isActivityAlive(act)) return;
        Dialog prev = sActiveDialogRef != null ? sActiveDialogRef.get() : null;
        dismissSafely(prev);

        LinearLayout card = new LinearLayout(act);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setBackground(roundedDrawable(COLOR_DIALOG_BG, COLOR_BORDER, 16, act));
        int p = dp(act, 18);
        card.setPadding(p, p, p, p);

        TextView title = new TextView(act);
        title.setText("Actualización disponible");
        title.setTextColor(COLOR_TEXT_PRIMARY);
        title.setTextSize(TypedValue.COMPLEX_UNIT_SP, 19f);
        title.setTypeface(null, Typeface.BOLD);
        card.addView(title);

        TextView channel = new TextView(act);
        channel.setText("Canal Release Oficial • GitHub uamo11/YR");
        channel.setTextColor(COLOR_GREEN);
        channel.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12.5f);
        channel.setTypeface(null, Typeface.BOLD);
        LinearLayout.LayoutParams chLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        chLp.setMargins(0, dp(act, 3), 0, dp(act, 12));
        card.addView(channel, chLp);

        LinearLayout verBox = new LinearLayout(act);
        verBox.setOrientation(LinearLayout.VERTICAL);
        verBox.setBackground(roundedDrawable(COLOR_CARD_BG, COLOR_BORDER, 10, act));
        int vp = dp(act, 12);
        verBox.setPadding(vp, vp, vp, vp);

        TextView verText = new TextView(act);
        verText.setText("Nueva versión: v" + info.version + "\nVersión actual: v" + curVer);
        verText.setTextColor(COLOR_TEXT_PRIMARY);
        verText.setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f);
        verText.setTypeface(null, Typeface.BOLD);
        verBox.addView(verText);

        String sizeStr = info.targetAsset.size > 0L ? formatSize(info.targetAsset.size) : "APK";
        TextView fileText = new TextView(act);
        fileText.setText("Archivo: " + info.targetAsset.name + " (" + sizeStr + ")");
        fileText.setTextColor(COLOR_TEXT_SECONDARY);
        fileText.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f);
        LinearLayout.LayoutParams fLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        fLp.setMargins(0, dp(act, 4), 0, 0);
        verBox.addView(fileText, fLp);
        card.addView(verBox);

        TextView notesHdr = new TextView(act);
        notesHdr.setText("Novedades:");
        notesHdr.setTextColor(COLOR_ACCENT_RED);
        notesHdr.setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f);
        notesHdr.setTypeface(null, Typeface.BOLD);
        LinearLayout.LayoutParams nhLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        nhLp.setMargins(0, dp(act, 12), 0, dp(act, 6));
        card.addView(notesHdr, nhLp);

        ScrollView scroll = new ScrollView(act);
        scroll.setBackground(roundedDrawable(COLOR_CARD_BG, COLOR_BORDER, 10, act));
        scroll.setPadding(vp, vp, vp, vp);
        TextView changelog = new TextView(act);
        changelog.setText(!info.body.isEmpty() ? info.body : info.name);
        changelog.setTextColor(COLOR_TEXT_PRIMARY);
        changelog.setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f);
        scroll.addView(changelog);
        LinearLayout.LayoutParams scLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, dp(act, 160));
        card.addView(scroll, scLp);

        CheckBox cbAuto = new CheckBox(act);
        cbAuto.setText("Buscar actualizaciones automáticamente");
        cbAuto.setTextColor(COLOR_TEXT_SECONDARY);
        cbAuto.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12.5f);
        cbAuto.setButtonTintList(ColorStateList.valueOf(COLOR_ACCENT_RED));
        cbAuto.setChecked(isAutoCheckEnabled(act));
        cbAuto.setOnCheckedChangeListener(new CompoundButton.OnCheckedChangeListener() {
            @Override
            public void onCheckedChanged(CompoundButton buttonView, boolean isChecked) {
                setAutoCheckEnabled(act, isChecked);
            }
        });
        LinearLayout.LayoutParams cbLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        cbLp.setMargins(0, dp(act, 8), 0, dp(act, 8));
        card.addView(cbAuto, cbLp);

        final Dialog dialog = createDarkDialog(act, card, true);
        sActiveDialogRef = new WeakReference<Dialog>(dialog);

        LinearLayout btnRow1 = new LinearLayout(act);
        btnRow1.setOrientation(LinearLayout.HORIZONTAL);
        btnRow1.setGravity(Gravity.END);

        Button btnSkip = makeDialogButton(act, "Omitir", 0x00000000, COLOR_TEXT_SECONDARY);
        btnSkip.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                prefs(act).edit().putString(PREF_SKIPPED_VERSION, info.version).apply();
                Toast.makeText(act, "Versión v" + info.version + " omitida", Toast.LENGTH_SHORT).show();
                dismissSafely(dialog);
            }
        });

        Button btnLater = makeDialogButton(act, "Más tarde", COLOR_CARD_BG, COLOR_TEXT_PRIMARY);
        btnLater.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                dismissSafely(dialog);
            }
        });

        final File preDownloaded = findExistingValidUpdateApk(act, info);
        String primaryText = preDownloaded != null ? "Instalar ahora" : "Actualizar";
        Button btnUpdate = makeDialogButton(act, primaryText, COLOR_ACCENT_RED, 0xFFFFFFFF);
        btnUpdate.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                dismissSafely(dialog);
                if (preDownloaded != null && isValidDownloadedApk(act, preDownloaded, info.targetAsset.size)) {
                    prefs(act).edit()
                            .putString(PREF_PENDING_APK_PATH, preDownloaded.getAbsolutePath())
                            .putString(PREF_PENDING_VERSION, info.version)
                            .apply();
                    launchApkInstallation(act, preDownloaded);
                } else {
                    downloadAndInstall(act, info);
                }
            }
        });

        LinearLayout.LayoutParams bLp1 = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        bLp1.setMargins(0, 0, dp(act, 6), 0);
        btnRow1.addView(btnSkip, bLp1);
        btnRow1.addView(btnLater, bLp1);
        btnRow1.addView(btnUpdate);
        card.addView(btnRow1);

        dialog.show();
    }

    private static void downloadAndInstall(final Activity act, final ReleaseInfo info) {
        if (!isActivityAlive(act)) return;

        LinearLayout card = new LinearLayout(act);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setBackground(roundedDrawable(COLOR_DIALOG_BG, COLOR_BORDER, 16, act));
        int p = dp(act, 20);
        card.setPadding(p, p, p, p);

        TextView title = new TextView(act);
        title.setText("Descargando actualización...");
        title.setTextColor(COLOR_TEXT_PRIMARY);
        title.setTextSize(TypedValue.COMPLEX_UNIT_SP, 17f);
        title.setTypeface(null, Typeface.BOLD);
        card.addView(title);

        TextView fname = new TextView(act);
        fname.setText(info.targetAsset.name);
        fname.setTextColor(COLOR_TEXT_SECONDARY);
        fname.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12.5f);
        LinearLayout.LayoutParams fnLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        fnLp.setMargins(0, dp(act, 4), 0, dp(act, 12));
        card.addView(fname, fnLp);

        final ProgressBar pb = new ProgressBar(act, null, android.R.attr.progressBarStyleHorizontal);
        pb.setMax(100);
        pb.setProgress(0);
        pb.setProgressTintList(ColorStateList.valueOf(COLOR_ACCENT_RED));
        card.addView(pb, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, dp(act, 10)));

        final TextView statsTv = new TextView(act);
        String expectedStr = info.targetAsset.size > 0L ? formatSize(info.targetAsset.size) : "-- MB";
        statsTv.setText("0 MB / " + expectedStr + " (0%)");
        statsTv.setTextColor(COLOR_TEXT_PRIMARY);
        statsTv.setTextSize(TypedValue.COMPLEX_UNIT_SP, 13.5f);
        LinearLayout.LayoutParams stLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        stLp.setMargins(0, dp(act, 10), 0, dp(act, 2));
        card.addView(statsTv, stLp);

        final TextView speedTv = new TextView(act);
        speedTv.setText("Conectando con GitHub...");
        speedTv.setTextColor(COLOR_GREEN);
        speedTv.setTextSize(TypedValue.COMPLEX_UNIT_SP, 12.5f);
        LinearLayout.LayoutParams spLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        spLp.setMargins(0, 0, 0, dp(act, 14));
        card.addView(speedTv, spLp);

        final Dialog dialog = createDarkDialog(act, card, false);
        final AtomicBoolean cancelled = new AtomicBoolean(false);
        Button cancelBtn = makeDialogButton(act, "Cancelar", COLOR_CARD_BG, COLOR_TEXT_PRIMARY);
        cancelBtn.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                cancelled.set(true);
                dismissSafely(dialog);
            }
        });
        card.addView(cancelBtn);
        dialog.show();

        sIsDownloading = true;
        final Context appCtx = act.getApplicationContext();
        new Thread(new Runnable() {
            @Override
            public void run() {
                File tempFile = null;
                try {
                    File existing = findExistingValidUpdateApk(appCtx, info);
                    File finalFile;
                    if (existing != null) {
                        finalFile = existing;
                    } else {
                        File dir = resolveUpdatesDir(appCtx);
                        File destFile = new File(dir, info.targetAsset.name);
                        tempFile = new File(dir, info.targetAsset.name + ".tmp");
                        long expectedSize = info.targetAsset.size;
                        long existingBytes = tempFile.exists() && tempFile.isFile() ? tempFile.length() : 0L;
                        if (expectedSize > 0L && existingBytes >= expectedSize) {
                            tempFile.delete();
                            existingBytes = 0L;
                        }

                        HttpURLConnection conn = openConnectionWithRedirects(
                                appCtx, info.targetAsset.downloadUrl, existingBytes);
                        try {
                            int code = conn.getResponseCode();
                            if (code == 416) {
                                conn.disconnect();
                                tempFile.delete();
                                existingBytes = 0L;
                                conn = openConnectionWithRedirects(appCtx, info.targetAsset.downloadUrl, 0L);
                                code = conn.getResponseCode();
                            }
                            if (code < 200 || code > 299) {
                                throw new IOException("HTTP " + code);
                            }
                            final boolean append = (code == HttpURLConnection.HTTP_PARTIAL && existingBytes > 0L);
                            long startOffset = append ? existingBytes : 0L;
                            long respLen = conn.getContentLengthLong();
                            final long totalBytes = (append && respLen > 0L)
                                    ? (startOffset + respLen)
                                    : (expectedSize > 0L ? expectedSize : respLen);

                            long downloaded = startOffset;
                            long lastUpdateMs = SystemClock.elapsedRealtime();
                            long sinceLast = 0L;
                            long smoothedBps = 0L;

                            InputStream input = conn.getInputStream();
                            FileOutputStream output = new FileOutputStream(tempFile, append);
                            try {
                                byte[] buf = new byte[DOWNLOAD_BUFFER_SIZE];
                                int read;
                                while ((read = input.read(buf)) != -1) {
                                    if (cancelled.get()) {
                                        throw new IOException("Descarga cancelada");
                                    }
                                    output.write(buf, 0, read);
                                    downloaded += read;
                                    sinceLast += read;
                                    long nowMs = SystemClock.elapsedRealtime();
                                    long diffMs = nowMs - lastUpdateMs;
                                    if (diffMs >= 140L) {
                                        final int pct = totalBytes > 0L
                                                ? (int) Math.min(100L, (downloaded * 100L) / totalBytes)
                                                : 0;
                                        long instBps = (sinceLast * 1000L) / diffMs;
                                        smoothedBps = smoothedBps <= 0L ? instBps : (smoothedBps * 3L + instBps) / 4L;
                                        final long curDl = downloaded;
                                        final long bpsNow = smoothedBps;
                                        final String eta = formatEta(totalBytes - curDl, bpsNow);
                                        sUiHandler.post(new Runnable() {
                                            @Override
                                            public void run() {
                                                if (!isActivityAlive(act)) return;
                                                pb.setProgress(pct);
                                                statsTv.setText(formatSize(curDl) + " / " + formatSize(totalBytes) + " (" + pct + "%)");
                                                speedTv.setText(totalBytes > 0L && bpsNow > 0L
                                                        ? (formatSize(bpsNow) + "/s • ETA " + eta)
                                                        : (formatSize(bpsNow) + "/s"));
                                            }
                                        });
                                        lastUpdateMs = nowMs;
                                        sinceLast = 0L;
                                    }
                                }
                                output.flush();
                            } finally {
                                try {
                                    output.close();
                                } catch (IOException ignored) {
                                }
                                try {
                                    input.close();
                                } catch (IOException ignored) {
                                }
                            }

                            long valSize = totalBytes > 0L ? totalBytes : expectedSize;
                            if (!isValidDownloadedApk(appCtx, tempFile, valSize)) {
                                tempFile.delete();
                                throw new IOException("El APK descargado está corrupto o incompleto");
                            }
                            if (destFile.exists()) destFile.delete();
                            if (!tempFile.renameTo(destFile)) {
                                throw new IOException("No se pudo mover el APK final");
                            }
                        } finally {
                            conn.disconnect();
                        }
                        finalFile = destFile;
                    }

                    final File installTarget = finalFile;
                    sUiHandler.post(new Runnable() {
                        @Override
                        public void run() {
                            dismissSafely(dialog);
                            if (!isActivityAlive(act)) return;
                            prefs(appCtx).edit()
                                    .putString(PREF_PENDING_APK_PATH, installTarget.getAbsolutePath())
                                    .putString(PREF_PENDING_VERSION, info.version)
                                    .apply();
                            Toast.makeText(act, "Actualización lista para instalar", Toast.LENGTH_SHORT).show();
                            launchApkInstallation(act, installTarget);
                        }
                    });
                } catch (final Exception e) {
                    if (cancelled.get() && tempFile != null) {
                        tempFile.delete();
                    }
                    sUiHandler.post(new Runnable() {
                        @Override
                        public void run() {
                            dismissSafely(dialog);
                            if (cancelled.get() || !isActivityAlive(act)) return;
                            String msg = e.getMessage() != null ? e.getMessage() : "Error de red";
                            showErrorDialog(act, "Error al descargar", msg, new Runnable() {
                                @Override
                                public void run() {
                                    downloadAndInstall(act, info);
                                }
                            });
                        }
                    });
                } finally {
                    sIsDownloading = false;
                }
            }
        }, "yr-update-download").start();
    }

    private static void launchApkInstallation(Activity act, File apkFile) {
        if (!isActivityAlive(act) || apkFile == null || !apkFile.exists()) return;
        try {
            if (Build.VERSION.SDK_INT >= 26 && !act.getPackageManager().canRequestPackageInstalls()) {
                sWaitingUnknownSources = true;
                sPendingInstallFile = apkFile;
                Toast.makeText(act, "Autoriza instalar aplicaciones de esta fuente", Toast.LENGTH_LONG).show();
                Intent permIntent = new Intent(
                        Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                        Uri.parse("package:" + act.getPackageName())
                );
                permIntent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                act.startActivity(permIntent);
                return;
            }
            Uri apkUri = (Build.VERSION.SDK_INT >= 24)
                    ? YrFileProvider.getUriForFile(act, apkFile)
                    : Uri.fromFile(apkFile);
            Intent install = new Intent(Intent.ACTION_VIEW);
            install.setDataAndType(apkUri, "application/vnd.android.package-archive");
            install.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_GRANT_READ_URI_PERMISSION);
            act.startActivity(install);
        } catch (Throwable e) {
            Toast.makeText(act, "No se pudo abrir el instalador: " + e.getMessage(), Toast.LENGTH_LONG).show();
        }
    }

    private static void dismissSafely(Dialog d) {
        if (d == null) return;
        try {
            if (d.isShowing()) d.dismiss();
        } catch (Throwable ignored) {
        }
    }

    private static String formatSize(long bytes) {
        if (bytes <= 0L) return "0 B";
        final String[] units = new String[] { "B", "KB", "MB", "GB" };
        int grp = (int) (Math.log10((double) bytes) / Math.log10(1024.0));
        if (grp >= units.length) grp = units.length - 1;
        double val = (double) bytes / Math.pow(1024.0, (double) grp);
        return String.format(Locale.US, "%.1f %s", Double.valueOf(val), units[grp]);
    }

    private static String formatEta(long remBytes, long bps) {
        if (remBytes <= 0L || bps <= 0L) return "--s";
        long sec = Math.max(1L, remBytes / bps);
        if (sec < 60L) return sec + "s";
        return (sec / 60L) + "m " + (sec % 60L) + "s";
    }
}
