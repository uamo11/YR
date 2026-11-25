package com.yr.music;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.content.Context;
import android.content.pm.ProviderInfo;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.util.ArrayList;
import java.util.List;

/**
 * Lightweight ContentProvider serving downloaded APK updates from YrFileProvider without AndroidX.
 */
public class YrFileProvider extends ContentProvider {
    public static final String AUTHORITY = "com.yr.music.fileprovider";

    public static Uri getUriForFile(Context ctx, File file) {
        return new Uri.Builder()
                .scheme("content")
                .authority(AUTHORITY)
                .appendPath(file.getName())
                .build();
    }

    private static List<File> candidateDirs(Context ctx) {
        List<File> dirs = new ArrayList<File>();
        if (ctx == null) return dirs;
        File ext = ctx.getExternalFilesDir(null);
        if (ext != null) {
            dirs.add(new File(ext, "updates"));
        }
        File cache = ctx.getCacheDir();
        if (cache != null) {
            dirs.add(new File(cache, "updates"));
        }
        return dirs;
    }

    private File resolveAllowedFile(Uri uri) throws FileNotFoundException {
        Context ctx = getContext();
        if (ctx == null || uri == null) {
            throw new FileNotFoundException("Missing provider context or URI");
        }
        String name = uri.getLastPathSegment();
        if (name == null || name.trim().isEmpty() || name.contains("..") || name.contains("/")) {
            throw new FileNotFoundException("Invalid update file name");
        }
        for (File root : candidateDirs(ctx)) {
            if (!root.exists()) continue;
            File target = new File(root, name);
            try {
                String canonicalRoot = root.getCanonicalPath();
                String canonicalTarget = target.getCanonicalPath();
                if (!canonicalTarget.startsWith(canonicalRoot + File.separator)) {
                    continue;
                }
            } catch (IOException ignored) {
                continue;
            }
            if (target.exists() && target.isFile()) {
                return target;
            }
        }
        throw new FileNotFoundException("Update file not found: " + name);
    }

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public void attachInfo(Context context, ProviderInfo info) {
        super.attachInfo(context, info);
        if (info.exported) {
            throw new SecurityException("YrFileProvider must not be exported");
        }
        if (!info.grantUriPermissions) {
            throw new SecurityException("YrFileProvider must grant uri permissions");
        }
    }

    @Override
    public Cursor query(
            Uri uri,
            String[] projection,
            String selection,
            String[] selectionArgs,
            String sortOrder
    ) {
        File file;
        try {
            file = resolveAllowedFile(uri);
        } catch (FileNotFoundException e) {
            return null;
        }
        String[] cols = projection != null
                ? projection
                : new String[] { OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE };
        String[] outCols = new String[cols.length];
        Object[] values = new Object[cols.length];
        int count = 0;
        for (int i = 0; i < cols.length; i++) {
            String col = cols[i];
            if (OpenableColumns.DISPLAY_NAME.equals(col)) {
                outCols[count] = OpenableColumns.DISPLAY_NAME;
                values[count++] = file.getName();
            } else if (OpenableColumns.SIZE.equals(col)) {
                outCols[count] = OpenableColumns.SIZE;
                values[count++] = Long.valueOf(file.length());
            }
        }
        String[] finalCols = new String[count];
        Object[] finalVals = new Object[count];
        System.arraycopy(outCols, 0, finalCols, 0, count);
        System.arraycopy(values, 0, finalVals, 0, count);
        MatrixCursor cursor = new MatrixCursor(finalCols, 1);
        cursor.addRow(finalVals);
        return cursor;
    }

    @Override
    public String getType(Uri uri) {
        String seg = uri != null ? uri.getLastPathSegment() : null;
        if (seg != null && seg.toLowerCase(java.util.Locale.US).endsWith(".apk")) {
            return "application/vnd.android.package-archive";
        }
        return "application/octet-stream";
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (mode != null && mode.contains("w")) {
            throw new SecurityException("Read-only update provider");
        }
        File file = resolveAllowedFile(uri);
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        throw new UnsupportedOperationException("No external inserts");
    }

    @Override
    public int delete(Uri uri, String selection, String[] selectionArgs) {
        throw new UnsupportedOperationException("No external deletes");
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] selectionArgs) {
        throw new UnsupportedOperationException("No external updates");
    }
}
