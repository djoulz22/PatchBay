use serde::Serialize;

const LEGACY_UID_PREFIX: &str = "fr.patchbay.audio.bus.";
const NATIVE_UID_PREFIX: &str = "fr.patchbay.audio.native.";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualBus {
    id: String,
    name: String,
    input_channels: usize,
    output_channels: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualBuses {
    driver_available: bool,
    driver_installed: bool,
    buses: Vec<VirtualBus>,
}

// Old patch files can still refer to the aggregate or to its underlying device.
// Treat them as the same endpoint when checking a saved graph for feedback.
pub(crate) fn backing_id(id: &str) -> Option<&str> {
    id.strip_prefix(LEGACY_UID_PREFIX).filter(|id| {
        let id = id.to_ascii_lowercase();
        id.starts_with("blackhole") && id.ends_with("_uid")
    })
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use crate::audio;
    use objc2::{
        rc::{Retained, autoreleasepool},
        runtime::AnyObject,
    };
    use objc2_core_audio::*;
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};
    use std::{
        mem::MaybeUninit,
        path::Path,
        process::Command,
        ptr::{self, NonNull},
        thread,
        time::{Duration, Instant},
    };

    const DRIVER_ID: &str = "fr.patchbay.audio.driver";
    const INSTALLED_DRIVER: &str =
        "/Library/Audio/Plug-Ins/HAL/Patchbay.driver/Contents/MacOS/Patchbay";
    const CONFIG_PROPERTY: u32 = u32::from_be_bytes(*b"pbcf");
    const DRIVER_MISSING: &str = "Le pilote audio Patchbay n'est pas chargé par macOS.";

    pub fn list() -> Result<VirtualBuses, String> {
        autoreleasepool(|_| {
            let driver = driver_object()?;
            let buses = if let Some(driver) = driver {
                read_devices(driver)?
            } else {
                Vec::new()
            };
            Ok(VirtualBuses {
                driver_available: driver.is_some(),
                driver_installed: driver.is_some() || Path::new(INSTALLED_DRIVER).is_file(),
                buses,
            })
        })
    }

    pub fn restart_core_audio() -> Result<Option<VirtualBuses>, String> {
        let current = list()?;
        if current.driver_available {
            return Ok(Some(current));
        }
        if !current.driver_installed {
            return Err("Installe le pilote Patchbay avant de relancer Core Audio.".into());
        }

        // macOS 14.4 and later restrict launchctl kickstart -k for system services.
        // Target only Apple's daemon account and let macOS handle authentication.
        let script = r#"
try
    do shell script "/usr/bin/killall -TERM -u _coreaudiod coreaudiod" with prompt "Patchbay demande de relancer Core Audio pour charger son pilote." with administrator privileges
    return "signalled"
on error number -128
    return "cancelled"
end try
"#;
        let output = Command::new("/usr/bin/osascript")
            .args(["-e", script])
            .output()
            .map_err(|error| {
                format!("Impossible de demander la relance de Core Audio ({error}).")
            })?;
        if !output.status.success() {
            let details = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "Impossible de relancer Core Audio. {}",
                details.trim()
            ));
        }
        if String::from_utf8_lossy(&output.stdout).trim() == "cancelled" {
            return Ok(None);
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(catalog) = list()
                && catalog.driver_available
            {
                return Ok(Some(catalog));
            }
            if Instant::now() >= deadline {
                return Err("Le pilote Patchbay reste indisponible après la demande de relance. Clique sur Vérifier le pilote ou redémarre le Mac si nécessaire.".into());
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    pub fn create(
        name: &str,
        input_channels: usize,
        output_channels: usize,
    ) -> Result<VirtualBus, String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
            return Err("Choisis un nom d'interface de 1 à 64 caractères.".into());
        }
        if !(1..=64).contains(&input_channels) || !(1..=64).contains(&output_channels) {
            return Err("Choisis un nombre d'entrées et un nombre de sorties de 1 à 64.".into());
        }
        let bus = autoreleasepool(|_| {
            let driver = driver_object()?.ok_or_else(|| DRIVER_MISSING.to_owned())?;
            let existing = read_devices(driver)?;
            if existing
                .iter()
                .any(|bus| bus.name.eq_ignore_ascii_case(name))
            {
                return Err("Une interface Patchbay porte déjà ce nom.".into());
            }
            let identifier = NSUUID::new().UUIDString().to_string().to_ascii_lowercase();
            let id = format!("{NATIVE_UID_PREFIX}{identifier}");
            let action = NSString::from_str("create");
            let native_id = NSString::from_str(&id);
            let native_name = NSString::from_str(name);
            let native_inputs = NSNumber::new_u32(input_channels as u32);
            let native_outputs = NSNumber::new_u32(output_channels as u32);
            let request = NSDictionary::<NSString, AnyObject>::from_slices::<NSString>(
                &[
                    &NSString::from_str("action"),
                    &NSString::from_str("id"),
                    &NSString::from_str("name"),
                    &NSString::from_str("inputChannels"),
                    &NSString::from_str("outputChannels"),
                ],
                &[
                    action.as_ref(),
                    native_id.as_ref(),
                    native_name.as_ref(),
                    native_inputs.as_ref(),
                    native_outputs.as_ref(),
                ],
            );
            submit_command(driver, &request, "La création de l'interface a échoué")?;
            Ok::<_, String>(VirtualBus {
                id,
                name: name.into(),
                input_channels,
                output_channels,
            })
        })?;
        wait_for_visibility(&bus.id, true)?;
        Ok(bus)
    }

    pub fn delete(id: &str) -> Result<(), String> {
        if !id.starts_with(NATIVE_UID_PREFIX) {
            return Err("Cette interface n'est pas gérée par le pilote Patchbay.".into());
        }
        let removed = autoreleasepool(|_| {
            let driver = driver_object()?.ok_or_else(|| DRIVER_MISSING.to_owned())?;
            if !read_devices(driver)?.iter().any(|bus| bus.id == id) {
                return Ok(false);
            }
            let action = NSString::from_str("delete");
            let native_id = NSString::from_str(id);
            let request = NSDictionary::<NSString, AnyObject>::from_slices::<NSString>(
                &[&NSString::from_str("action"), &NSString::from_str("id")],
                &[action.as_ref(), native_id.as_ref()],
            );
            submit_command(driver, &request, "La suppression de l'interface a échoué")?;
            Ok::<_, String>(true)
        })?;
        if removed {
            wait_for_visibility(id, false)?;
        }
        Ok(())
    }

    fn address(selector: u32) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        }
    }

    fn driver_object() -> Result<Option<AudioObjectID>, String> {
        let identifier = NSString::from_str(DRIVER_ID);
        let qualifier = ptr::from_ref(&*identifier);
        let object = read_property::<AudioObjectID>(
            kAudioObjectSystemObject as AudioObjectID,
            kAudioHardwarePropertyTranslateBundleIDToPlugIn,
            Some(&qualifier),
        )?;
        Ok((object != kAudioObjectUnknown as AudioObjectID).then_some(object))
    }

    fn read_devices(driver: AudioObjectID) -> Result<Vec<VirtualBus>, String> {
        let raw = read_property::<*mut AnyObject>(driver, CONFIG_PROPERTY, None)?;
        // SAFETY: The driver and Core Audio return a retained CF property list. Its
        // Foundation representation is toll-free bridged. Check its type before use.
        let array = unsafe { Retained::from_raw(raw) }
            .and_then(|value| value.downcast::<NSArray<AnyObject>>().ok())
            .ok_or_else(invalid_catalog)?;
        let mut devices = Vec::with_capacity(array.count());
        for value in array.iter() {
            let entry = value
                .downcast::<NSDictionary>()
                .map_err(|_| invalid_catalog())?;
            let id = string_value(&entry, "id")?;
            let name = string_value(&entry, "name")?;
            let input_channels = channel_value(&entry, "inputChannels")?;
            let output_channels = channel_value(&entry, "outputChannels")?;
            if !id.starts_with(NATIVE_UID_PREFIX) || name.is_empty() {
                return Err(invalid_catalog());
            }
            devices.push(VirtualBus {
                id,
                name,
                input_channels,
                output_channels,
            });
        }
        Ok(devices)
    }

    fn channel_value(entry: &NSDictionary, key: &str) -> Result<usize, String> {
        entry
            .objectForKey(NSString::from_str(key).as_ref())
            .and_then(|value| value.downcast::<NSNumber>().ok())
            .map(|number| number.unsignedLongLongValue())
            .filter(|channels| (1..=64).contains(channels))
            .map(|channels| channels as usize)
            .ok_or_else(invalid_catalog)
    }

    fn string_value(entry: &NSDictionary, key: &str) -> Result<String, String> {
        entry
            .objectForKey(NSString::from_str(key).as_ref())
            .and_then(|value| value.downcast::<NSString>().ok())
            .map(|value| value.to_string())
            .ok_or_else(invalid_catalog)
    }

    fn invalid_catalog() -> String {
        "Le pilote Patchbay a retourné un catalogue d'interfaces invalide.".into()
    }

    fn read_property<T>(
        object: AudioObjectID,
        selector: u32,
        qualifier: Option<&*const NSString>,
    ) -> Result<T, String> {
        let address = address(selector);
        let mut data = MaybeUninit::<T>::uninit();
        let mut size = size_of::<T>() as u32;
        let (qualifier_size, qualifier_data) = qualifier.map_or((0, ptr::null()), |value| {
            (
                size_of::<*const NSString>() as u32,
                ptr::from_ref(value).cast(),
            )
        });
        // SAFETY: Both read sites use the SDK-defined output type for the requested
        // property. The qualifier, when present, points to a live CFString reference.
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&address),
                qualifier_size,
                qualifier_data,
                NonNull::from(&mut size),
                NonNull::from(&mut data).cast(),
            )
        };
        check(status, "La lecture du pilote Patchbay a échoué")?;
        if size != size_of::<T>() as u32 {
            return Err(invalid_catalog());
        }
        Ok(unsafe { data.assume_init() })
    }

    fn submit_command(
        driver: AudioObjectID,
        request: &NSDictionary<NSString, AnyObject>,
        operation: &str,
    ) -> Result<(), String> {
        let address = address(CONFIG_PROPERTY);
        let reference = ptr::from_ref(request);
        // SAFETY: The custom property expects a pointer to a CFPropertyListRef.
        // The toll-free bridged dictionary remains live for the duration of the call.
        let status = unsafe {
            AudioObjectSetPropertyData(
                driver,
                NonNull::from(&address),
                0,
                ptr::null(),
                size_of_val(&reference) as u32,
                NonNull::from(&reference).cast(),
            )
        };
        check(status, operation)
    }

    fn check(status: i32, operation: &str) -> Result<(), String> {
        if status == 0 {
            return Ok(());
        }
        if status == kAudioHardwareIllegalOperationError {
            return Err(format!(
                "{operation}. Vérifie que l'interface est disponible et qu'aucune application ne l'utilise."
            ));
        }
        Err(format!("{operation} (Core Audio {status})."))
    }

    fn wait_for_visibility(id: &str, visible: bool) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while audio::device_object(id).is_some() != visible {
            if Instant::now() >= deadline {
                return Err("Le pilote a accepté la modification. Actualise les interfaces pour vérifier sa publication dans macOS.".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub use macos::{create, delete, list, restart_core_audio};
