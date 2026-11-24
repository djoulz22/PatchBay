//! Private macOS process taps and Core Audio HAL metadata.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, OsStr, c_void},
    fs,
    mem::{offset_of, size_of},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::{self, NonNull},
    slice,
    sync::{Arc, mpsc},
    time::Duration,
};

use block2::RcBlock;
use objc2::{
    AnyThread, class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::{AnyObject, Bool, NSObjectProtocol},
    sel,
};
use objc2_core_audio::*;
use objc2_core_audio_types::*;
use objc2_foundation::{
    NSArray, NSBundle, NSDictionary, NSHomeDirectory, NSNumber, NSString, NSURL, NSUUID,
};

use super::{AudioApplication, AudioDevice, Direction, FrameQueue, MAX_CHANNELS};

type Result<T> = std::result::Result<T, String>;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVMediaTypeAudio: &'static NSString;
}

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidpath(pid: i32, buffer: *mut c_void, buffer_size: u32) -> i32;
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

const MICROPHONE_NOT_DETERMINED: isize = 0;
const MICROPHONE_AUTHORIZED: isize = 3;
const MICROPHONE_PERMISSION_ERROR: &str =
    "Autorise le microphone dans Réglages Système > Confidentialité et sécurité.";

fn microphone_authorization_status() -> isize {
    let device = class!(AVCaptureDevice);
    // SAFETY: AVFoundation is linked above, and this selector takes its exported audio media type.
    unsafe { msg_send![device, authorizationStatusForMediaType: AVMediaTypeAudio] }
}

pub(super) fn microphone_authorized() -> Result<()> {
    autoreleasepool(|_| {
        if microphone_authorization_status() == MICROPHONE_AUTHORIZED {
            Ok(())
        } else {
            Err(MICROPHONE_PERMISSION_ERROR.into())
        }
    })
}

pub(super) fn request_microphone() -> Result<()> {
    autoreleasepool(|_| {
        match microphone_authorization_status() {
            MICROPHONE_AUTHORIZED => return Ok(()),
            MICROPHONE_NOT_DETERMINED => {}
            _ => return Err(MICROPHONE_PERMISSION_ERROR.into()),
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let completed = RcBlock::new(move |granted: Bool| {
            let _ = sender.try_send(granted.as_bool());
        });
        let device = class!(AVCaptureDevice);
        // SAFETY: AVFoundation copies this block for async delivery. Its captured sender is Send.
        let _: () = unsafe {
            msg_send![device, requestAccessForMediaType: AVMediaTypeAudio, completionHandler: &*completed]
        };
        match receiver.recv_timeout(Duration::from_secs(120)) {
            Ok(true) => Ok(()),
            Ok(false) => Err(MICROPHONE_PERMISSION_ERROR.into()),
            Err(_) => Err("La demande d'accès au microphone n'a pas abouti.".into()),
        }
    })
}

fn status(result: i32, operation: &str) -> Result<()> {
    if result == 0 {
        return Ok(());
    }
    let permission = if result == -54 || result == kAudioDevicePermissionsError {
        " Vérifie les autorisations audio dans Confidentialité et sécurité."
    } else {
        ""
    };
    Err(format!("{operation} (Core Audio {result}).{permission}"))
}

fn property_address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn property<T>(object: AudioObjectID, selector: u32, qualifier: &[u8]) -> Result<T> {
    property_at(object, property_address(selector), qualifier)
}

fn property_at<T>(
    object: AudioObjectID,
    address: AudioObjectPropertyAddress,
    qualifier: &[u8],
) -> Result<T> {
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    // SAFETY: The address and qualifier are valid and the output buffer has exactly size bytes.
    // Callers use the SDK-defined plain-data type for each selector.
    let result = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            qualifier.len() as u32,
            if qualifier.is_empty() {
                ptr::null()
            } else {
                qualifier.as_ptr().cast()
            },
            NonNull::from(&mut size),
            NonNull::new(value.as_mut_ptr().cast()).unwrap(),
        )
    };
    status(result, "Lecture d'une propriété Core Audio")?;
    if size != size_of::<T>() as u32 {
        return Err("Taille de propriété Core Audio inattendue.".into());
    }
    // SAFETY: The successful call initialized the exact SDK type and byte count.
    Ok(unsafe { value.assume_init() })
}

fn audio_process_ids() -> Result<Vec<AudioObjectID>> {
    const ID_BYTES: u32 = size_of::<AudioObjectID>() as u32;
    let system = kAudioObjectSystemObject as AudioObjectID;
    let address = property_address(kAudioHardwarePropertyProcessObjectList);
    for _ in 0..3 {
        let mut size = 0;
        // SAFETY: This unqualified property returns an array of AudioObjectIDs.
        status(
            unsafe {
                AudioObjectGetPropertyDataSize(
                    system,
                    NonNull::from(&address),
                    0,
                    ptr::null(),
                    NonNull::from(&mut size),
                )
            },
            "Lecture de la taille de la liste des processus Core Audio",
        )?;
        if size == 0 {
            return Ok(Vec::new());
        }
        if !size.is_multiple_of(ID_BYTES) {
            return Err("Taille de liste des processus Core Audio inattendue.".into());
        }
        let mut ids = vec![0; (size / ID_BYTES) as usize];
        let mut written = size;
        // SAFETY: The ID buffer is aligned and has exactly `size` writable bytes.
        let result = unsafe {
            AudioObjectGetPropertyData(
                system,
                NonNull::from(&address),
                0,
                ptr::null(),
                NonNull::from(&mut written),
                NonNull::new(ids.as_mut_ptr().cast()).unwrap(),
            )
        };
        if result == kAudioHardwareBadPropertySizeError {
            // A HAL client can connect between reading the size and reading the list.
            continue;
        }
        status(result, "Lecture de la liste des processus Core Audio")?;
        if written > size || !written.is_multiple_of(ID_BYTES) {
            return Err("Taille de liste des processus Core Audio inattendue.".into());
        }
        ids.truncate((written / ID_BYTES) as usize);
        return Ok(ids);
    }
    Err("La liste des processus Core Audio a changé. Actualise le catalogue.".into())
}

fn is_application_bundle(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

fn outer_application_bundle(executable: &Path) -> Option<&Path> {
    // Chromium and Electron place their audio helpers in nested .app bundles. Match the
    // enclosing application on disk instead of assuming a bundle-ID prefix identifies it.
    executable
        .ancestors()
        .skip(1)
        .filter(|candidate| {
            is_application_bundle(candidate)
                && executable
                    .strip_prefix(candidate)
                    .is_ok_and(|inside| inside.starts_with("Contents"))
        })
        .last()
}

fn process_application(pid: i32) -> Option<(String, String)> {
    // PROC_PIDPATHINFO_MAXSIZE in the macOS SDK.
    let mut buffer = [0u8; 4096];
    // SAFETY: The byte array is writable and its length fits the API's u32 size.
    if unsafe { proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) } <= 0 {
        return None;
    }
    let executable = Path::new(OsStr::from_bytes(
        CStr::from_bytes_until_nul(&buffer).ok()?.to_bytes(),
    ));
    bundle_application(outer_application_bundle(executable)?)
}

fn bundle_application(app: &Path) -> Option<(String, String)> {
    let bundle = NSBundle::bundleWithPath(&NSString::from_str(app.to_str()?))?;
    let id = bundle.bundleIdentifier()?.to_string();
    if id.is_empty() {
        return None;
    }
    let name = ["CFBundleDisplayName", "CFBundleName"]
        .into_iter()
        .find_map(|key| {
            let value = bundle
                .objectForInfoDictionaryKey(&NSString::from_str(key))?
                .downcast::<NSString>()
                .ok()?;
            let name = value.to_string();
            (!name.trim().is_empty()).then_some(name)
        })
        .unwrap_or_else(|| {
            app.file_stem()
                .and_then(OsStr::to_str)
                .unwrap_or(&id)
                .to_owned()
        });
    Some((id, name))
}

fn installed_application_paths() -> BTreeSet<PathBuf> {
    let home_applications = PathBuf::from(NSHomeDirectory().to_string()).join("Applications");
    let mut directories = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        home_applications,
    ];
    let mut applications = BTreeSet::new();
    while let Some(directory) = directories.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_application_bundle(&path) {
                if path.is_dir() {
                    applications.insert(path);
                }
                // The helpers bundled within an app belong to that app.
            } else if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                // Do not recurse through symlinks.
                directories.push(path);
            }
        }
    }
    applications
}

fn running_application_paths() -> Vec<PathBuf> {
    // SAFETY: AppKit's NSWorkspace returns NSRunningApplication objects and documents
    // runningApplications as safe to read from a background thread.
    let applications: Retained<NSArray<AnyObject>> = unsafe {
        let workspace: Retained<AnyObject> = msg_send![class!(NSWorkspace), sharedWorkspace];
        msg_send![&*workspace, runningApplications]
    };
    applications
        .iter()
        .filter_map(|application| {
            // SAFETY: Each array member is an NSRunningApplication and bundleURL is nullable.
            let url: Option<Retained<NSURL>> = unsafe { msg_send![&*application, bundleURL] };
            let path = PathBuf::from(url?.path()?.to_string());
            if !is_application_bundle(&path) {
                return None;
            }
            Some(outer_application_bundle(&path).unwrap_or(&path).to_owned())
        })
        .collect()
}

struct ApplicationGroup {
    name: String,
    processes: Vec<AudioObjectID>,
}

fn audio_applications() -> Result<BTreeMap<String, ApplicationGroup>> {
    let mut applications = BTreeMap::new();
    let this_pid = std::process::id();
    for object in audio_process_ids()? {
        let Ok(pid) = property::<i32>(object, kAudioProcessPropertyPID, &[]) else {
            // The HAL client can disconnect after the list was read.
            continue;
        };
        if pid <= 0 || pid as u32 == this_pid {
            continue;
        }
        let Some((id, name)) = process_application(pid) else {
            continue;
        };
        applications
            .entry(id)
            .or_insert_with(|| ApplicationGroup {
                name,
                processes: Vec::new(),
            })
            .processes
            .push(object);
    }
    Ok(applications)
}

pub(super) fn list_applications() -> Vec<AudioApplication> {
    autoreleasepool(|_| {
        // A HAL restart must not hide installed apps. The tap's separate lookup still
        // propagates HAL errors when opening the user's selection.
        let connected = audio_applications().unwrap_or_default();
        let mut paths = installed_application_paths();
        paths.extend(running_application_paths());
        let own_id = NSBundle::mainBundle()
            .bundleIdentifier()
            .map(|id| id.to_string());
        let mut applications = application_catalog(
            paths.iter().filter_map(|path| bundle_application(path)),
            connected,
        );
        applications.retain(|app| own_id.as_deref() != Some(app.bundle_id.as_str()));
        applications
    })
}

fn application_catalog(
    bundles: impl IntoIterator<Item = (String, String)>,
    connected: BTreeMap<String, ApplicationGroup>,
) -> Vec<AudioApplication> {
    let mut groups: BTreeMap<_, _> = bundles
        .into_iter()
        .map(|(id, name)| {
            (
                id,
                ApplicationGroup {
                    name,
                    processes: Vec::new(),
                },
            )
        })
        .collect();
    groups.extend(connected);
    let mut applications: Vec<_> = groups
        .into_iter()
        .map(|(id, group)| AudioApplication {
            bundle_id: id,
            name: group.name,
            active: !group.processes.is_empty(),
        })
        .collect();
    applications.sort_by_cached_key(|app| app.name.to_lowercase());
    applications
}

pub(crate) fn device_object(id: &str) -> Option<AudioObjectID> {
    autoreleasepool(|_| {
        let uid = NSString::from_str(id);
        let qualifier = (&*uid as *const NSString).addr().to_ne_bytes();
        property::<AudioObjectID>(
            kAudioObjectSystemObject as AudioObjectID,
            kAudioHardwarePropertyTranslateUIDToDevice,
            &qualifier,
        )
        .ok()
        .filter(|object| *object != kAudioObjectUnknown as AudioObjectID)
    })
}

pub(super) fn device_sample_rate(id: &str) -> Option<u32> {
    nominal_sample_rate(device_object(id)?)
}

fn nominal_sample_rate(device: AudioObjectID) -> Option<u32> {
    let rate = property::<f64>(device, kAudioDevicePropertyNominalSampleRate, &[]).ok()?;
    (8_000.0..=384_000.0).contains(&rate).then_some(rate as u32)
}

pub(super) fn is_virtual_device(id: &str, name: &str) -> bool {
    is_virtual_object(device_object(id), id, name)
}

fn is_virtual_object(device: Option<AudioObjectID>, id: &str, name: &str) -> bool {
    if crate::virtual_bus::backing_id(id).is_some() {
        return true;
    }
    let transport = device
        .and_then(|device| property::<u32>(device, kAudioDevicePropertyTransportType, &[]).ok());
    let known_loopback_driver = id.to_ascii_lowercase().contains("blackhole")
        || id.to_ascii_lowercase().contains("vbcable")
        || name.starts_with("BlackHole")
        || name == "VB-Cable";
    transport == Some(kAudioDeviceTransportTypeVirtual) || known_loopback_driver
}

pub(super) fn device_catalog_entry(
    id: String,
    direction: Direction,
    default_id: Option<&str>,
) -> Option<AudioDevice> {
    let device = device_object(&id)?;
    let channels = device_channel_count(device, direction)?.min(MAX_CHANNELS);
    if channels == 0 {
        return None;
    }
    let name = device_name(device)?;
    Some(AudioDevice {
        is_default: default_id == Some(id.as_str()),
        is_virtual: is_virtual_object(Some(device), &id, &name),
        sample_rate: nominal_sample_rate(device)?,
        channels,
        name,
        id,
    })
}

fn device_name(device: AudioObjectID) -> Option<String> {
    autoreleasepool(|_| {
        // Match the property CPAL uses for display names, without its format probes.
        let address = AudioObjectPropertyAddress {
            mScope: kAudioObjectPropertyScopeOutput,
            ..property_address(kAudioDevicePropertyDeviceNameCFString)
        };
        let raw = property_at::<*mut NSString>(device, address, &[])
            .or_else(|_| property(device, kAudioObjectPropertyName, &[]))
            .ok()?;
        // SAFETY: Both properties return an owned CFString, toll-free bridged to NSString.
        let name = unsafe { Retained::from_raw(raw) }?;
        Some(name.to_string())
    })
}

fn device_channel_count(device: AudioObjectID, direction: Direction) -> Option<usize> {
    let address = AudioObjectPropertyAddress {
        mScope: match direction {
            Direction::Input => kAudioObjectPropertyScopeInput,
            Direction::Output => kAudioObjectPropertyScopeOutput,
        },
        ..property_address(kAudioDevicePropertyStreamConfiguration)
    };
    let mut size = 0_u32;
    // SAFETY: The address and output-size pointer are valid for this unqualified property.
    let query = unsafe {
        AudioObjectGetPropertyDataSize(
            device,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut size),
        )
    };
    if query != 0 || size < size_of::<u32>() as u32 {
        return None;
    }
    let capacity = size as usize;
    // Allocate aligned storage for the flexible AudioBufferList returned by HAL.
    let mut storage = vec![
        AudioBuffer {
            mNumberChannels: 0,
            mDataByteSize: 0,
            mData: ptr::null_mut()
        };
        capacity.div_ceil(size_of::<AudioBuffer>())
    ];
    let raw = storage.as_mut_ptr().cast::<u8>();
    // SAFETY: Storage is aligned for AudioBufferList and can hold all `size` bytes.
    let read = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&address),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(raw.cast()).unwrap(),
        )
    };
    if read != 0 || (size as usize) < size_of::<u32>() || size as usize > capacity {
        return None;
    }
    // SAFETY: A successful read returned at least the AudioBufferList count.
    let count = unsafe { raw.cast::<u32>().read() } as usize;
    if count == 0 {
        return Some(0);
    }
    let first_buffer = offset_of!(AudioBufferList, mBuffers);
    let available = (size as usize).saturating_sub(first_buffer) / size_of::<AudioBuffer>();
    if count > available {
        return None;
    }
    // SAFETY: `count` entries fit within the written aligned buffer. No audio data is accessed.
    let buffers =
        unsafe { slice::from_raw_parts(raw.add(first_buffer).cast::<AudioBuffer>(), count) };
    Some(buffers.iter().fold(0_usize, |channels, buffer| {
        channels.saturating_add(buffer.mNumberChannels as usize)
    }))
}

pub(super) struct SystemTap {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    io_proc: AudioDeviceIOProcID,
    context: Option<Arc<FrameQueue>>,
    started: bool,
    rate: u32,
    channels: usize,
}

fn tap_description(
    application: Option<(&str, bool)>,
    processes: &NSArray<NSNumber>,
) -> Result<Retained<CATapDescription>> {
    // SAFETY: The description and its argument arrays use owned Foundation objects.
    unsafe {
        let (description, mute_behavior) = if let Some((bundle_id, mute_original)) = application {
            let app = CATapDescription::initStereoMixdownOfProcesses(
                CATapDescription::alloc(),
                processes,
            );
            if app.respondsToSelector(sel!(setBundleIDs:))
                && app.respondsToSelector(sel!(setProcessRestoreEnabled:))
            {
                // macOS 26 adds lookup by bundle ID and process restoration. The known
                // process IDs also include audio helpers inside the outer app bundle.
                let bundle_ids = NSArray::from_retained_slice(&[NSString::from_str(bundle_id)]);
                app.setBundleIDs(&bundle_ids);
                app.setProcessRestoreEnabled(true);
            } else if processes.is_empty() {
                return Err(format!(
                    "{bundle_id} n'est pas connecté à Core Audio. Lance une lecture dans l'application puis réessaie."
                ));
            }
            let behavior = if mute_original {
                CATapMuteBehavior::MutedWhenTapped
            } else {
                CATapMuteBehavior::Unmuted
            };
            (app, behavior)
        } else {
            let system = CATapDescription::initStereoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                processes,
            );
            (system, CATapMuteBehavior::Unmuted)
        };
        description.setUUID(&NSUUID::new());
        description.setPrivate(true);
        description.setMuteBehavior(mute_behavior);
        Ok(description)
    }
}

impl SystemTap {
    pub(super) fn open(application: Option<(&str, bool)>) -> Result<(Self, Arc<FrameQueue>)> {
        autoreleasepool(|_| {
            let mut stream = Self {
                tap: 0,
                aggregate: 0,
                io_proc: None,
                context: None,
                started: false,
                rate: 0,
                channels: 0,
            };
            let process_ids = if let Some((bundle_id, _)) = application {
                let own_id = NSBundle::mainBundle().bundleIdentifier();
                if bundle_id.is_empty() || own_id.is_some_and(|id| id.to_string() == bundle_id) {
                    return Err("Choisis une autre application à capturer.".into());
                }
                audio_applications()?
                    .remove(bundle_id)
                    .map_or_else(Vec::new, |app| app.processes)
            } else {
                let pid = std::process::id().to_ne_bytes();
                let this_process: AudioObjectID = property(
                    kAudioObjectSystemObject as AudioObjectID,
                    kAudioHardwarePropertyTranslatePIDToProcessObject,
                    &pid,
                )?;
                if this_process == kAudioObjectUnknown as AudioObjectID {
                    return Err("Core Audio n'a pas pu exclure les sorties de Patchbay de la capture système.".into());
                }
                vec![this_process]
            };
            let processes: Vec<_> = process_ids.into_iter().map(NSNumber::new_u32).collect();
            let processes = NSArray::from_retained_slice(&processes);
            let description = tap_description(application, &processes)?;
            // SAFETY: The tap description is owned and the output ID is writable.
            status(
                unsafe { AudioHardwareCreateProcessTap(Some(&description), &mut stream.tap) },
                "Création de la capture audio",
            )?;
            if stream.tap == kAudioObjectUnknown as AudioObjectID {
                return Err("Core Audio n'a pas créé la capture. Vérifie l'autorisation d'enregistrement de l'audio système.".into());
            }

            let format: AudioStreamBasicDescription =
                property(stream.tap, kAudioTapPropertyFormat, &[])?;
            let (rate, channels) = pcm_format(&format).ok_or_else(|| {
                "La capture Core Audio n'a pas fourni de PCM Float32 mono ou stéréo.".to_owned()
            })?;
            stream.rate = rate;
            stream.channels = channels;

            let uid_ptr: *mut NSString = property(stream.tap, kAudioTapPropertyUID, &[])?;
            // SAFETY: The tap property returns an owned CFString, toll-free bridged to NSString.
            let uid = unsafe { Retained::from_raw(uid_ptr) }
                .ok_or_else(|| "Identifiant de capture Core Audio manquant.".to_owned())?;
            let yes = NSNumber::new_bool(true);
            let auto_start = NSNumber::new_bool(application.is_none());
            let sub_tap = NSDictionary::<NSString, AnyObject>::from_slices::<NSString>(
                &[
                    &key(kAudioSubTapUIDKey),
                    &key(kAudioSubTapDriftCompensationKey),
                ],
                &[uid.as_ref(), yes.as_ref()],
            );
            let taps = NSArray::from_retained_slice(&[sub_tap]);
            let options = NSDictionary::<NSString, AnyObject>::from_slices::<NSString>(
                &[
                    &key(kAudioAggregateDeviceNameKey),
                    &key(kAudioAggregateDeviceUIDKey),
                    &key(kAudioAggregateDeviceIsPrivateKey),
                    &key(kAudioAggregateDeviceTapAutoStartKey),
                    &key(kAudioAggregateDeviceTapListKey),
                ],
                &[
                    NSString::from_str("Patchbay Audio Tap").as_ref(),
                    NSUUID::new().UUIDString().as_ref(),
                    yes.as_ref(),
                    auto_start.as_ref(),
                    taps.as_ref(),
                ],
            );
            // SAFETY: NSDictionary and CFDictionary are toll-free bridged. The dictionary and
            // values are retained throughout the call.
            status(
                unsafe {
                    AudioHardwareCreateAggregateDevice(
                        NonNull::from(&*options).cast().as_ref(),
                        NonNull::from(&mut stream.aggregate),
                    )
                },
                "Création du périphérique de capture privé",
            )?;
            if stream.aggregate == kAudioObjectUnknown as AudioObjectID {
                return Err("Core Audio n'a pas créé le périphérique de capture privé.".into());
            }
            drop(description);

            let queue = FrameQueue::new(channels);
            let context = Arc::as_ptr(&queue).cast_mut().cast();
            stream.context = Some(Arc::clone(&queue));
            // SAFETY: The Arc keeps the context at a stable address until the callback is removed.
            status(
                unsafe {
                    AudioDeviceCreateIOProcID(
                        stream.aggregate,
                        Some(receive),
                        context,
                        NonNull::from(&mut stream.io_proc),
                    )
                },
                "Enregistrement du callback de capture",
            )?;
            // SAFETY: Both IDs belong to this stream and the context is still alive.
            status(
                unsafe { AudioDeviceStart(stream.aggregate, stream.io_proc) },
                "Démarrage de la capture audio",
            )?;
            stream.started = true;
            Ok((stream, queue))
        })
    }

    pub(super) fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub(super) fn check_alive(&self) -> Result<()> {
        let alive: u32 = property(self.aggregate, kAudioDevicePropertyDeviceIsAlive, &[])?;
        if alive == 0 {
            return Err("Le périphérique de capture Core Audio a été déconnecté.".into());
        }
        let format: AudioStreamBasicDescription = property(self.tap, kAudioTapPropertyFormat, &[])?;
        if pcm_format(&format) != Some((self.rate, self.channels)) {
            return Err("Le format de la capture Core Audio a changé.".into());
        }
        Ok(())
    }
}

fn pcm_format(format: &AudioStreamBasicDescription) -> Option<(u32, usize)> {
    let buffer_channels = if format.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0 {
        1
    } else {
        format.mChannelsPerFrame
    };
    (format.mFormatID == kAudioFormatLinearPCM
        && format.mFormatFlags & kAudioFormatFlagIsFloat != 0
        && format.mFormatFlags & kAudioFormatFlagIsBigEndian == 0
        && format.mBitsPerChannel == 32
        && matches!(format.mChannelsPerFrame, 1 | 2)
        && format.mBytesPerFrame == buffer_channels * size_of::<f32>() as u32
        && (8_000.0..=384_000.0).contains(&format.mSampleRate))
    .then(|| {
        (
            format.mSampleRate.round() as u32,
            format.mChannelsPerFrame as usize,
        )
    })
}

impl Drop for SystemTap {
    fn drop(&mut self) {
        // SAFETY: The registered IDs are torn down in reverse order. A registered IOProc's
        // context remains valid until the callback is unregistered.
        unsafe {
            if self.started {
                let _ = AudioDeviceStop(self.aggregate, self.io_proc);
            }
            if self.io_proc.is_some()
                && AudioDeviceDestroyIOProcID(self.aggregate, self.io_proc) != 0
                && let Some(context) = self.context.take()
            {
                // The aggregate's destruction can be asynchronous.
                std::mem::forget(context);
            }
            if self.aggregate != 0 {
                let _ = AudioHardwareDestroyAggregateDevice(self.aggregate);
            }
            if self.tap != 0 {
                let _ = AudioHardwareDestroyProcessTap(self.tap);
            }
        }
    }
}

fn key(key: &CStr) -> Retained<NSString> {
    NSString::from_str(key.to_str().expect("Core Audio dictionary keys are ASCII"))
}

unsafe extern "C-unwind" fn receive(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    context: *mut c_void,
) -> i32 {
    // SAFETY: Core Audio supplies the callback arguments. The context lives until the
    // IOProc is unregistered, and the callback never retains device-owned memory.
    unsafe {
        let queue = &*context.cast::<FrameQueue>();
        let buffer_count = (*input.as_ptr()).mNumberBuffers;
        if !matches!(buffer_count, 1 | 2) {
            return 0;
        }
        // AudioBufferList is a C flexible array of mNumberBuffers AudioBuffer values.
        let buffers = ptr::addr_of!((*input.as_ptr()).mBuffers).cast::<AudioBuffer>();
        let left = &*buffers;
        let Some(samples) = buffer_samples(left) else {
            return 0;
        };
        match (buffer_count, left.mNumberChannels, queue.channels) {
            (1, 1, 1 | 2) => {
                for &sample in samples {
                    queue.push(|_| sample);
                }
            }
            (1, 2, 2) => {
                for frame in samples.chunks_exact(2) {
                    queue.push(|channel| frame[channel]);
                }
            }
            (2, 1, 2) => {
                let right = &*buffers.add(1);
                if right.mNumberChannels == 1
                    && let Some(right) = buffer_samples(right)
                {
                    for (left, right) in samples.iter().zip(right) {
                        queue.push(|channel| if channel == 0 { *left } else { *right });
                    }
                }
            }
            _ => {}
        }
    }
    0
}

unsafe fn buffer_samples(buffer: &AudioBuffer) -> Option<&[f32]> {
    let pointer = buffer.mData.cast::<f32>();
    let bytes = buffer.mDataByteSize as usize;
    if pointer.is_null()
        || !pointer.is_aligned()
        || bytes == 0
        || !bytes.is_multiple_of(size_of::<f32>())
    {
        return None;
    }
    // SAFETY: The callback keeps this buffer alive while the slice is used.
    Some(unsafe { slice::from_raw_parts(pointer, bytes / size_of::<f32>()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_behavior_is_applied_to_the_native_tap_description() {
        autoreleasepool(|_| {
            // The description is tested without registering a tap or opening a device.
            let processes = NSArray::from_retained_slice(&[NSNumber::new_u32(42)]);
            let application = "audio.example.app";
            for (source, expected) in [
                (None, CATapMuteBehavior::Unmuted),
                (Some((application, false)), CATapMuteBehavior::Unmuted),
                (
                    Some((application, true)),
                    CATapMuteBehavior::MutedWhenTapped,
                ),
            ] {
                let description = tap_description(source, &processes).unwrap();
                // SAFETY: This reads the properties of our initialized description.
                assert_eq!(unsafe { description.isMuted() }, expected);
            }
        });
    }
}
