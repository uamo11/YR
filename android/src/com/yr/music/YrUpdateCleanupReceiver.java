package com.yr.music;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/**
 * Cleans up downloaded YR Music update APKs once Android finishes replacing the app package.
 */
public class YrUpdateCleanupReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        if (intent == null || !Intent.ACTION_MY_PACKAGE_REPLACED.equals(intent.getAction())) {
            return;
        }
        final Context appContext = context.getApplicationContext();
        final PendingResult pendingResult = goAsync();
        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    YrUpdateChecker.cleanupInstalledUpdateApks(appContext);
                } catch (Throwable ignored) {
                } finally {
                    try {
                        pendingResult.finish();
                    } catch (Throwable ignored) {
                    }
                }
            }
        }, "yr-update-cleanup").start();
    }
}
