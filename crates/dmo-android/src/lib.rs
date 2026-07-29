#![cfg(target_os = "android")]
//! Android NativeActivity entry point for DMO.

use std::{error::Error, fs};

use jni::{
    JavaVM,
    objects::{JObject, JValue},
};
use winit::platform::android::activity::AndroidApp;

const MICROPHONE_PERMISSION: &str = "android.permission.RECORD_AUDIO";
const MICROPHONE_PERMISSION_REQUEST: i32 = 1_001;

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    configure_storage(&app);
    request_microphone_permission(&app);

    let options = eframe::NativeOptions {
        android_app: Some(app),
        ..Default::default()
    };
    if let Err(error) = eframe::run_native(
        "DMO — Open Modern DAW",
        options,
        Box::new(|creation_context| Ok(Box::new(dmo_gui::DmoApp::new(creation_context)))),
    ) {
        log::error!("DMO Android stopped: {error}");
    }
}

fn configure_storage(app: &AndroidApp) {
    let Some(root) = app
        .external_data_path()
        .or_else(|| app.internal_data_path())
    else {
        log::error!("Android did not expose an application data directory");
        return;
    };
    if let Err(error) = fs::create_dir_all(&root) {
        log::error!("Could not create Android data directory: {error}");
        return;
    }
    for directory in ["Projects", "Imports", "Exports", "Recordings"] {
        if let Err(error) = fs::create_dir_all(root.join(directory)) {
            log::error!("Could not create {directory} directory: {error}");
        }
    }
    if let Err(error) = std::env::set_current_dir(&root) {
        log::error!("Could not select Android data directory: {error}");
    } else {
        log::info!("DMO Android data directory: {}", root.display());
    }
}

fn request_microphone_permission(app: &AndroidApp) {
    let app = app.clone();
    app.clone().run_on_java_main_thread(Box::new(move || {
        if let Err(error) = request_microphone_permission_on_java_thread(&app) {
            log::error!("Could not request microphone permission: {error}");
        }
    }));
}

fn request_microphone_permission_on_java_thread(app: &AndroidApp) -> Result<(), Box<dyn Error>> {
    // SAFETY: AndroidApp owns both raw references for the lifetime of this
    // callback, and the callback runs on the attached Java main thread.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast())? };
    let mut environment = vm.attach_current_thread()?;
    // SAFETY: activity_as_ptr is a valid, unowned global Activity reference
    // while `app` remains alive. JObject does not delete local/global refs on
    // drop; AndroidApp retains ownership.
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let permission = environment.new_string(MICROPHONE_PERMISSION)?;
    let permission_object = JObject::from(permission);
    let status = environment
        .call_method(
            &activity,
            "checkSelfPermission",
            "(Ljava/lang/String;)I",
            &[JValue::Object(&permission_object)],
        )?
        .i()?;
    if status == 0 {
        return Ok(());
    }

    let string_class = environment.find_class("java/lang/String")?;
    let permissions = environment.new_object_array(1, string_class, JObject::null())?;
    environment.set_object_array_element(&permissions, 0, &permission_object)?;
    let permissions_object = JObject::from(permissions);
    environment.call_method(
        &activity,
        "requestPermissions",
        "([Ljava/lang/String;I)V",
        &[
            JValue::Object(&permissions_object),
            JValue::Int(MICROPHONE_PERMISSION_REQUEST),
        ],
    )?;
    Ok(())
}
