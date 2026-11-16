package com.yr.music;

import android.app.NativeActivity;

public class YrMainActivity extends NativeActivity {
    @Override
    public void onBackPressed() {
        moveTaskToBack(true);
    }
}
