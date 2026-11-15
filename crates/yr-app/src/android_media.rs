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
    use jni::objects::{GlobalRef, JClass, JObject, JValue};
    use jni::sys::{jint, jlong};
    use jni::{JNIEnv, JavaVM};
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use tokio::sync::mpsc;

    struct AndroidBridge {
        vm: JavaVM,
        app_ctx: GlobalRef,
        service_class: GlobalRef,
    }

    static BRIDGE: OnceLock<AndroidBridge> = OnceLock::new();
    static ACTION_TX: OnceLock<mpsc::UnboundedSender<MediaAction>> = OnceLock::new();

    pub fn set_action_sender(tx: mpsc::UnboundedSender<MediaAction>) {
        let _ = ACTION_TX.set(tx);
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

        let _ = BRIDGE.set(AndroidBridge {
            vm,
            app_ctx,
            service_class,
        });
        // Prevent dropping `activity` local ref as owned if borrowed from NativeActivity:
        std::mem::forget(activity);
    }

    pub fn update_playback(title: &str, artist: &str, playing: bool, pos_ms: u64, dur_ms: u64) {
        let Some(bridge) = BRIDGE.get() else {
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

    pub fn stop_service() {
        let Some(bridge) = BRIDGE.get() else {
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
        let Some(bridge) = BRIDGE.get() else {
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
        if let Some(tx) = ACTION_TX.get() {
            let _ = tx.send(mapped);
        }
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    use super::MediaAction;
    use tokio::sync::mpsc;

    pub fn set_action_sender(_tx: mpsc::UnboundedSender<MediaAction>) {}
    pub fn update_playback(_title: &str, _artist: &str, _playing: bool, _pos_ms: u64, _dur_ms: u64) {
    }
    pub fn stop_service() {}
    pub fn resolve_audio_url(_video_id: &str) -> Option<String> {
        None
    }
}

#[cfg(target_os = "android")]
pub use imp::init;
pub use imp::{resolve_audio_url, set_action_sender, stop_service, update_playback};
