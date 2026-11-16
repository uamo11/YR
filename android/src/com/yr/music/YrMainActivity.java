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
    public void onBackPressed() {
        try {
            if (nativeOnBackPressed()) {
                return;
            }
        } catch (UnsatisfiedLinkError ignored) {
        }
        moveTaskToBack(true);
    }
}
