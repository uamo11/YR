#[derive(Debug, Clone, Copy)]
pub enum MediaAction {
    Play,
    Pause,
    Toggle,
    Next,
    Prev,
    SeekMs(u64),
    Stop,
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
    open_google_login, resolve_audio_url, set_action_sender, set_artwork_url, set_cookie_sender,
    set_egui_ctx, set_ui_at_root, share_text, stop_service, take_back_pressed, update_playback,
};
