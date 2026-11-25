package com.yr.music;

import android.app.NativeActivity;

public class YrMainActivity extends NativeActivity {
    static {
        try {
            System.loadLibrary("yr_app");
        } catch (UnsatisfiedLinkError ignored) {
        }
    }

    private static native boolean nativeOnBackPressed();

    @Override
    protected void onPause() {
        super.onPause();
        try {
            YrPlaybackService.onActivityPaused(this);
        } catch (Throwable ignored) {
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        try {
            YrPlaybackService.onActivityResumed(this);
        } catch (Throwable ignored) {
        }
        try {
            YrUpdateChecker.onActivityResumed(this);
        } catch (Throwable ignored) {
        }
    }

    @Override
    protected void onDestroy() {
        try {
            YrPlaybackService.destroyEmbeddedVideo(this);
        } catch (Throwable ignored) {
        }
        super.onDestroy();
    }

    @Override
    public void onBackPressed() {
        try {
            if (YrPlaybackService.exitFullscreenIfActive(this)) {
                return;
            }
        } catch (Throwable ignored) {
        }
        try {
            if (nativeOnBackPressed()) {
                return;
            }
        } catch (UnsatisfiedLinkError ignored) {
        }
        moveTaskToBack(true);
    }
}
