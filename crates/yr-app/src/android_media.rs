#[derive(Debug, Clone, Copy)]
pub enum MediaAction {
    Play,
    Pause,
    Toggle,
    Next,
    Prev,
    SeekMs(u64),
    Stop,
    CycleVideoQuality,
}

#[cfg(target_os = "android")]
mod imp {
    use super::MediaAction;
    use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
    use jni::sys::{jboolean, jint, jlong, JNI_FALSE, JNI_TRUE};
    use jni::{JNIEnv, JavaVM};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    struct AndroidBridge {
        vm: JavaVM,
        app_ctx: GlobalRef,
        service_class: GlobalRef,
    }

    static BRIDGE: Mutex<Option<Arc<AndroidBridge>>> = Mutex::new(None);
    static ACTION_TX: Mutex<Option<mpsc::UnboundedSender<MediaAction>>> = Mutex::new(None);
    static COOKIE_TX: Mutex<Option<mpsc::UnboundedSender<String>>> = Mutex::new(None);
    static EGUI_CTX: Mutex<Option<egui::Context>> = Mutex::new(None);
    static UI_AT_ROOT: AtomicBool = AtomicBool::new(true);
    static BACK_PRESSED_PENDING: AtomicBool = AtomicBool::new(false);

    pub fn set_egui_ctx(ctx: egui::Context) {
        if let Ok(mut g) = EGUI_CTX.lock() {
            *g = Some(ctx);
        }
    }

    pub fn set_ui_at_root(at_root: bool) {
        UI_AT_ROOT.store(at_root, Ordering::Release);
    }

    pub fn take_back_pressed() -> bool {
        BACK_PRESSED_PENDING.swap(false, Ordering::AcqRel)
    }

    fn bridge() -> Option<Arc<AndroidBridge>> {
        BRIDGE.lock().ok().and_then(|g| g.clone())
    }

    pub fn set_action_sender(tx: mpsc::UnboundedSender<MediaAction>) {
        if let Ok(mut g) = ACTION_TX.lock() {
            *g = Some(tx);
        }
    }

    pub fn set_cookie_sender(tx: mpsc::UnboundedSender<String>) {
        if let Ok(mut g) = COOKIE_TX.lock() {
            *g = Some(tx);
        }
    }

    pub unsafe fn init(vm_ptr: *mut c_void, activity_ptr: *mut c_void) {
        if vm_ptr.is_null() || activity_ptr.is_null() {
            return;
        }
        let Ok(vm) = JavaVM::from_raw(vm_ptr as *mut jni::sys::JavaVM) else {
            return;
        };
        let Ok(mut env) = vm.get_env() else {
            return;
        };
        let activity = JObject::from_raw(activity_ptr as jni::sys::jobject);
        let Ok(app_ctx) = env.new_global_ref(&activity) else {
            return;
        };

        let loader_val = env.call_method(&activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[]);
        let Ok(loader_obj) = loader_val.and_then(|v| v.l()) else {
            log::warn!("android_media: failed to get ClassLoader");
            return;
        };
        let Ok(cls_name) = env.new_string("com.yr.music.YrPlaybackService") else {
            return;
        };
        let cls_val = env.call_method(
            &loader_obj,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&cls_name)],
        );
        let Ok(cls_obj) = cls_val.and_then(|v| v.l()) else {
            log::warn!("android_media: failed to load YrPlaybackService");
            return;
        };
        let Ok(service_class) = env.new_global_ref(&cls_obj) else {
            return;
        };

        if let Ok(mut g) = BRIDGE.lock() {
            *g = Some(Arc::new(AndroidBridge {
                vm,
                app_ctx,
                service_class,
            }));
        }
        // Prevent dropping `activity` local ref as owned if borrowed from NativeActivity:
        std::mem::forget(activity);
    }

    pub fn update_playback(title: &str, artist: &str, playing: bool, pos_ms: u64, dur_ms: u64) {
        let Some(bridge) = bridge() else {
            return;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return;
        };
        let Ok(j_title) = env.new_string(title) else {
            return;
        };
        let Ok(j_artist) = env.new_string(artist) else {
            return;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        let res = env.call_static_method(
            service_cls,
            "updatePlayback",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;ZJJ)V",
            &[
                JValue::Object(bridge.app_ctx.as_obj()),
                JValue::Object(&j_title),
                JValue::Object(&j_artist),
                JValue::Bool(if playing { 1 } else { 0 }),
                JValue::Long(pos_ms as i64),
                JValue::Long(dur_ms as i64),
            ],
        );
        if let Err(e) = res {
            log::warn!("android_media: updatePlayback failed: {e:?}");
            let _ = env.exception_clear();
        }
    }

    pub fn set_artwork_url(url: &str) {
        let Some(bridge) = bridge() else {
            return;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return;
        };
        let Ok(j_url) = env.new_string(url) else {
            return;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        let res = env.call_static_method(
            service_cls,
            "setArtworkUrl",
            "(Landroid/content/Context;Ljava/lang/String;)V",
            &[
                JValue::Object(bridge.app_ctx.as_obj()),
                JValue::Object(&j_url),
            ],
        );
        if let Err(e) = res {
            log::warn!("android_media: setArtworkUrl failed: {e:?}");
            let _ = env.exception_clear();
        }
    }

    pub fn stop_service() {
        let Some(bridge) = bridge() else {
            return;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        let _ = env.call_static_method(
            service_cls,
            "stopService",
            "(Landroid/content/Context;)V",
            &[JValue::Object(bridge.app_ctx.as_obj())],
        );
    }

    pub fn resolve_audio_url(video_id: &str) -> Option<String> {
        let Some(bridge) = bridge() else {
            log::warn!("android_media: resolve_audio_url BRIDGE not initialized");
            return None;
        };
        let mut env = match bridge.vm.attach_current_thread() {
            Ok(e) => e,
            Err(err) => {
                log::warn!("android_media: attach_current_thread failed: {err:?}");
                return None;
            }
        };
        let j_vid = env.new_string(video_id).ok()?;
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        log::info!("android_media: calling YrPlaybackService.resolveAudioUrl({video_id})");
        let res = env.call_static_method(
            service_cls,
            "resolveAudioUrl",
            "(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;",
            &[
                JValue::Object(bridge.app_ctx.as_obj()),
                JValue::Object(&j_vid),
            ],
        );
        match res {
            Ok(val) => {
                let obj = val.l().ok()?;
                if obj.is_null() {
                    return None;
                }
                let jstr: jni::objects::JString = obj.into();
                env.get_string(&jstr).ok().map(|s| s.into())
            }
            Err(e) => {
                log::warn!("android_media: resolveAudioUrl failed: {e:?}");
                let _ = env.exception_clear();
                None
            }
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_yr_music_YrPlaybackService_nativeOnMediaAction(
        _env: JNIEnv,
        _class: JClass,
        action: jint,
        arg: jlong,
    ) {
        let mapped = match action {
            0 => MediaAction::Play,
            1 => MediaAction::Pause,
            2 => MediaAction::Toggle,
            3 => MediaAction::Next,
            4 => MediaAction::Prev,
            5 => MediaAction::SeekMs(arg.max(0) as u64),
            6 => MediaAction::Stop,
            7 => MediaAction::CycleVideoQuality,
            _ => return,
        };
        if let Some(tx) = ACTION_TX.lock().ok().and_then(|g| g.clone()) {
            let _ = tx.send(mapped);
        }
    }

    pub fn open_google_login() {
        let Some(bridge) = bridge() else {
            return;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        if let Err(e) = env.call_static_method(
            service_cls,
            "openGoogleLogin",
            "(Landroid/content/Context;)V",
            &[JValue::Object(bridge.app_ctx.as_obj())],
        ) {
            log::warn!("android_media: openGoogleLogin failed: {e:?}");
            let _ = env.exception_clear();
        }
    }

    pub fn share_text(title: &str, text: &str) {
        let Some(bridge) = bridge() else {
            return;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return;
        };
        let Ok(j_title) = env.new_string(title) else {
            return;
        };
        let Ok(j_text) = env.new_string(text) else {
            return;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        if let Err(e) = env.call_static_method(
            service_cls,
            "shareText",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V",
            &[
                JValue::Object(bridge.app_ctx.as_obj()),
                JValue::Object(&j_title),
                JValue::Object(&j_text),
            ],
        ) {
            log::warn!("android_media: shareText failed: {e:?}");
            let _ = env.exception_clear();
        }
    }

    pub fn is_wifi_connected() -> bool {
        let Some(bridge) = bridge() else {
            return true;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return true;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        let Ok(val) = env.call_static_method(
            service_cls,
            "isWifiConnected",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(bridge.app_ctx.as_obj())],
        ) else {
            return true;
        };
        val.z().unwrap_or(true)
    }

    pub fn is_metered_network() -> bool {
        let Some(bridge) = bridge() else {
            return false;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return false;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        let Ok(val) = env.call_static_method(
            service_cls,
            "isMeteredNetwork",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(bridge.app_ctx.as_obj())],
        ) else {
            return false;
        };
        val.z().unwrap_or(false)
    }

    pub fn attach_native_video(
        video_url: &str,
        fallback_url: &str,
        user_agent: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        start_ms: u64,
        playing: bool,
        speed: f32,
    ) {
        let Some(bridge) = bridge() else {
            return;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return;
        };
        let Ok(j_url) = env.new_string(video_url) else {
            return;
        };
        let Ok(j_fallback) = env.new_string(fallback_url) else {
            return;
        };
        let Ok(j_ua) = env.new_string(user_agent) else {
            return;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        if let Err(e) = env.call_static_method(
            service_cls,
            "attachNativeVideo",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;IIIIJZF)V",
            &[
                JValue::Object(bridge.app_ctx.as_obj()),
                JValue::Object(&j_url),
                JValue::Object(&j_fallback),
                JValue::Object(&j_ua),
                JValue::Int(x),
                JValue::Int(y),
                JValue::Int(width),
                JValue::Int(height),
                JValue::Long(start_ms as i64),
                JValue::Bool(if playing { 1 } else { 0 }),
                JValue::Float(speed),
            ],
        ) {
            log::warn!("android_media: attachNativeVideo failed: {e:?}");
            let _ = env.exception_clear();
        }
    }

    pub fn hide_embedded_video() {
        let Some(bridge) = bridge() else {
            return;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        if let Err(e) = env.call_static_method(
            service_cls,
            "hideEmbeddedVideo",
            "(Landroid/content/Context;)V",
            &[JValue::Object(bridge.app_ctx.as_obj())],
        ) {
            log::warn!("android_media: hideEmbeddedVideo failed: {e:?}");
            let _ = env.exception_clear();
        }
    }

    pub fn prepare_next_embedded_video() {
        let Some(bridge) = bridge() else {
            return;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        if let Err(e) = env.call_static_method(
            service_cls,
            "prepareNextEmbeddedVideo",
            "(Landroid/content/Context;)V",
            &[JValue::Object(bridge.app_ctx.as_obj())],
        ) {
            log::warn!("android_media: prepareNextEmbeddedVideo failed: {e:?}");
            let _ = env.exception_clear();
        }
    }

    pub fn sync_native_video(playing: bool, pos_ms: u64, speed: f32) {
        let Some(bridge) = bridge() else {
            return;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        if let Err(e) = env.call_static_method(
            service_cls,
            "syncNativeVideo",
            "(Landroid/content/Context;ZJF)V",
            &[
                JValue::Object(bridge.app_ctx.as_obj()),
                JValue::Bool(if playing { 1 } else { 0 }),
                JValue::Long(pos_ms as i64),
                JValue::Float(speed),
            ],
        ) {
            log::warn!("android_media: syncNativeVideo failed: {e:?}");
            let _ = env.exception_clear();
        }
    }

    pub fn set_video_seek_seconds(secs: u32) {
        let Some(bridge) = bridge() else {
            return;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        if let Err(e) = env.call_static_method(
            service_cls,
            "setSeekSeconds",
            "(I)V",
            &[JValue::Int(secs as jint)],
        ) {
            log::warn!("android_media: setSeekSeconds failed: {e:?}");
            let _ = env.exception_clear();
        }
    }

    pub fn set_video_quality_label(label: &str) {
        let Some(bridge) = bridge() else {
            return;
        };
        let Ok(mut env) = bridge.vm.attach_current_thread() else {
            return;
        };
        let Ok(j_label) = env.new_string(label) else {
            return;
        };
        let service_cls: &JClass = bridge.service_class.as_obj().into();
        if let Err(e) = env.call_static_method(
            service_cls,
            "setVideoQualityLabel",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&j_label)],
        ) {
            log::warn!("android_media: setVideoQualityLabel failed: {e:?}");
            let _ = env.exception_clear();
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_yr_music_YrPlaybackService_nativeOnGoogleCookie(
        mut env: JNIEnv,
        _class: JClass,
        cookie: JString,
    ) {
        if let Ok(s) = env.get_string(&cookie) {
            let val: String = s.into();
            if let Some(tx) = COOKIE_TX.lock().ok().and_then(|g| g.clone()) {
                let _ = tx.send(val);
            }
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_yr_music_YrMainActivity_nativeOnBackPressed(
        _env: JNIEnv,
        _class: JClass,
    ) -> jboolean {
        if UI_AT_ROOT.load(Ordering::Acquire) {
            return JNI_FALSE;
        }
        BACK_PRESSED_PENDING.store(true, Ordering::Release);
        if let Some(ctx) = EGUI_CTX.lock().ok().and_then(|g| g.clone()) {
            ctx.request_repaint();
        }
        JNI_TRUE
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    use super::MediaAction;
    use tokio::sync::mpsc;

    pub fn set_action_sender(_tx: mpsc::UnboundedSender<MediaAction>) {}
    pub fn set_cookie_sender(_tx: mpsc::UnboundedSender<String>) {}
    pub fn set_egui_ctx(_ctx: egui::Context) {}
    pub fn set_ui_at_root(_at_root: bool) {}
    pub fn take_back_pressed() -> bool {
        false
    }
    pub fn open_google_login() {}
    pub fn share_text(_title: &str, _text: &str) {}
    pub fn attach_native_video(
        _video_url: &str,
        _fallback_url: &str,
        _user_agent: &str,
        _x: i32,
        _y: i32,
        _width: i32,
        _height: i32,
        _start_ms: u64,
        _playing: bool,
        _speed: f32,
    ) {
    }
    pub fn hide_embedded_video() {}
    pub fn prepare_next_embedded_video() {}
    pub fn sync_native_video(_playing: bool, _pos_ms: u64, _speed: f32) {}
    pub fn set_video_seek_seconds(_secs: u32) {}
    pub fn set_video_quality_label(_label: &str) {}
    pub fn is_wifi_connected() -> bool {
        true
    }
    pub fn is_metered_network() -> bool {
        false
    }
    pub fn update_playback(_title: &str, _artist: &str, _playing: bool, _pos_ms: u64, _dur_ms: u64) {
    }
    pub fn set_artwork_url(_url: &str) {}
    pub fn stop_service() {}
    pub fn resolve_audio_url(_video_id: &str) -> Option<String> {
        None
    }
}

#[cfg(target_os = "android")]
pub use imp::init;
pub use imp::{
    attach_native_video, hide_embedded_video, is_metered_network, is_wifi_connected,
    open_google_login, prepare_next_embedded_video, resolve_audio_url, set_action_sender,
    set_artwork_url, set_cookie_sender, set_egui_ctx, set_ui_at_root, set_video_quality_label,
    set_video_seek_seconds, share_text, stop_service, sync_native_video, take_back_pressed,
    update_playback,
};
