#include "patchbay_driver.h"

#include <CoreFoundation/CFPlugInCOM.h>
#include <mach/mach_time.h>
#include <math.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define PB_BOX_OBJECT 2
#define PB_RING_FRAMES 8192
#define PB_EMPTY_FRAME UINT64_MAX

typedef struct {
    UInt32 input;
    UInt32 output;
} ChannelCounts;

typedef struct {
    AudioObjectID id;
    CFStringRef uid;
    CFStringRef name;
    ChannelCounts channels;
    UInt32 shared_channels;
    _Atomic(UInt64) claim; // Object ID in the high word, active IO references in the low word.
    _Atomic(UInt32) running;
    _Atomic(UInt64) anchor;
    _Atomic(UInt64) clock_generation;
    _Atomic(UInt64) *positions;
    _Atomic(uint32_t) *samples;
} Device;

static pthread_mutex_t configuration_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_mutex_t devices_lock = PTHREAD_MUTEX_INITIALIZER;
static Device devices[PB_MAX_DEVICES];
static AudioObjectID next_device_id = 3;
static AudioServerPlugInHostRef host;
static Float64 host_ticks_per_frame;
static _Atomic(ULONG) references;
static AudioServerPlugInDriverInterface driver_interface;
static AudioServerPlugInDriverInterface *driver_pointer = &driver_interface;

static UInt64 device_claim(AudioObjectID id) {
    return (UInt64)id << 32;
}

static bool is_device_locked(const Device *device) {
    return device->id != kAudioObjectUnknown;
}

static Device *find_device_locked(AudioObjectID id) {
    if (id < 3) return NULL;
    for (size_t index = 0; index < PB_MAX_DEVICES; ++index) {
        if (devices[index].id == id) return &devices[index];
    }
    return NULL;
}

static Device *find_uid_locked(CFStringRef uid) {
    for (size_t index = 0; index < PB_MAX_DEVICES; ++index) {
        if (is_device_locked(&devices[index]) && CFEqual(devices[index].uid, uid)) {
            return &devices[index];
        }
    }
    return NULL;
}

// A CAS on the combined identity and refcount prevents a retired IO callback
// from ever accessing buffers belonging to a subsequent device in this slot.
static Device *acquire_device(AudioObjectID id) {
    if (id < 3) return NULL;
    const UInt64 identity = device_claim(id);
    for (size_t index = 0; index < PB_MAX_DEVICES; ++index) {
        Device *device = &devices[index];
        UInt64 claim = atomic_load_explicit(&device->claim, memory_order_acquire);
        while ((claim & ~((UInt64)UINT32_MAX)) == identity && (UInt32)claim != UINT32_MAX) {
            if (atomic_compare_exchange_weak_explicit(&device->claim, &claim, claim + 1,
                                                      memory_order_acquire, memory_order_relaxed)) {
                return device;
            }
        }
    }
    return NULL;
}

static void release_device(Device *device) {
    atomic_fetch_sub_explicit(&device->claim, 1, memory_order_release);
}

size_t PBListDeviceIDs(AudioObjectID *ids, size_t capacity) {
    size_t count = 0;
    pthread_mutex_lock(&devices_lock);
    for (size_t index = 0; index < PB_MAX_DEVICES; ++index) {
        if (!is_device_locked(&devices[index])) continue;
        if (ids && count < capacity) ids[count] = devices[index].id;
        ++count;
    }
    pthread_mutex_unlock(&devices_lock);
    return count;
}

bool PBCopyDeviceInfo(AudioObjectID object, PBDeviceInfo *out) {
    if (!out || object < 3) return false;
    bool found = false;
    pthread_mutex_lock(&devices_lock);
    for (size_t index = 0; index < PB_MAX_DEVICES; ++index) {
        const Device *device = &devices[index];
        if (!is_device_locked(device) || object < device->id || object > device->id + 2) continue;
        *out = (PBDeviceInfo) {
            .id = device->id,
            .input_stream = device->id + 1,
            .output_stream = device->id + 2,
            .uid = CFRetain(device->uid),
            .name = CFRetain(device->name),
            .input_channels = device->channels.input,
            .output_channels = device->channels.output,
            .running = atomic_load_explicit(&device->running, memory_order_relaxed) != 0,
        };
        found = true;
        break;
    }
    pthread_mutex_unlock(&devices_lock);
    return found;
}

static CFDictionaryRef device_record(CFStringRef uid, CFStringRef name, ChannelCounts channels) {
    const int input_count = (int)channels.input;
    const int output_count = (int)channels.output;
    const CFNumberRef input = CFNumberCreate(NULL, kCFNumberIntType, &input_count);
    const CFNumberRef output = CFNumberCreate(NULL, kCFNumberIntType, &output_count);
    CFDictionaryRef record = NULL;
    if (input && output) {
        const void *keys[] = { CFSTR("id"), CFSTR("name"), CFSTR("inputChannels"), CFSTR("outputChannels") };
        const void *values[] = { uid, name, input, output };
        record = CFDictionaryCreate(NULL, keys, values, 4,
                                    &kCFTypeDictionaryKeyCallBacks,
                                    &kCFTypeDictionaryValueCallBacks);
    }
    if (input) CFRelease(input);
    if (output) CFRelease(output);
    return record;
}

static CFMutableArrayRef copy_configuration_locked(const Device *skip) {
    CFMutableArrayRef result = CFArrayCreateMutable(NULL, PB_MAX_DEVICES, &kCFTypeArrayCallBacks);
    if (!result) return NULL;
    for (size_t index = 0; index < PB_MAX_DEVICES; ++index) {
        const Device *device = &devices[index];
        if (!is_device_locked(device) || device == skip) continue;
        CFDictionaryRef record = device_record(device->uid, device->name, device->channels);
        if (!record) {
            CFRelease(result);
            return NULL;
        }
        CFArrayAppendValue(result, record);
        CFRelease(record);
    }
    return result;
}

CFArrayRef PBCopyConfiguration(void) {
    pthread_mutex_lock(&devices_lock);
    CFArrayRef result = copy_configuration_locked(NULL);
    pthread_mutex_unlock(&devices_lock);
    return result;
}

static bool valid_uid(CFStringRef uid) {
    if (!uid || CFGetTypeID(uid) != CFStringGetTypeID()) return false;
    if (!CFStringHasPrefix(uid, CFSTR(PB_UID_PREFIX))) return false;
    const CFIndex prefix_length = sizeof(PB_UID_PREFIX) - 1;
    const CFIndex length = CFStringGetLength(uid) - prefix_length;
    if (length != 36) return false;
    for (CFIndex index = 0; index < length; ++index) {
        const UniChar character = CFStringGetCharacterAtIndex(uid, prefix_length + index);
        if (index == 8 || index == 13 || index == 18 || index == 23) {
            if (character != '-') return false;
        } else if (!((character >= '0' && character <= '9') ||
                     (character >= 'a' && character <= 'f'))) {
            return false;
        }
    }
    return true;
}

static bool valid_name(CFStringRef name) {
    if (!name || CFGetTypeID(name) != CFStringGetTypeID()) return false;
    const CFIndex length = CFStringGetLength(name);
    if (length == 0 || length > 128) return false;
    for (CFIndex index = 0; index < length; ++index) {
        const UniChar character = CFStringGetCharacterAtIndex(name, index);
        if (character < 32 || (character >= 127 && character <= 159)) return false;
    }
    return true;
}

static bool read_channel_count(CFNumberRef number, UInt32 *channels) {
    int count;
    if (!number || CFGetTypeID(number) != CFNumberGetTypeID() ||
        !CFNumberGetValue(number, kCFNumberIntType, &count) ||
        count < 1 || count > PB_MAX_CHANNELS) return false;
    *channels = (UInt32)count;
    return true;
}

static bool get_record(CFDictionaryRef record, CFStringRef *uid, CFStringRef *name,
                       ChannelCounts *channels, bool restore) {
    *uid = CFDictionaryGetValue(record, CFSTR("id"));
    *name = CFDictionaryGetValue(record, CFSTR("name"));
    const CFNumberRef input = CFDictionaryGetValue(record, CFSTR("inputChannels"));
    const CFNumberRef output = CFDictionaryGetValue(record, CFSTR("outputChannels"));
    if (!valid_uid(*uid) || !valid_name(*name)) return false;
    if (restore && !input && !output) {
        const CFNumberRef legacy = CFDictionaryGetValue(record, CFSTR("channels"));
        if (!read_channel_count(legacy, &channels->input)) return false;
        channels->output = channels->input;
        return true;
    }
    return read_channel_count(input, &channels->input) && read_channel_count(output, &channels->output);
}

static void notify_configuration(void) {
    if (!host->PropertiesChanged) return;
    const AudioObjectPropertyAddress plugin[] = {
        { kAudioPlugInPropertyDeviceList, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain },
        { kAudioObjectPropertyOwnedObjects, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain },
        { PB_CONFIG_PROPERTY, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain },
    };
    const AudioObjectPropertyAddress box = {
        kAudioBoxPropertyDeviceList, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain,
    };
    host->PropertiesChanged(host, kAudioObjectPlugInObject, 3, plugin);
    host->PropertiesChanged(host, PB_BOX_OBJECT, 1, &box);
}

static void notify_running(AudioObjectID id) {
    if (!host->PropertiesChanged) return;
    const AudioObjectPropertyAddress address = {
        kAudioDevicePropertyDeviceIsRunning, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain,
    };
    host->PropertiesChanged(host, id, 1, &address);
}

// Calls that mutate the roster are serialized by configuration_lock. We never
// invoke host storage APIs while holding the lock used by property queries.
static OSStatus create_device(CFStringRef uid, CFStringRef name, ChannelCounts channels,
                             bool persist, AudioObjectID *created) {
    pthread_mutex_lock(&devices_lock);
    Device *existing = find_uid_locked(uid);
    if (existing) {
        const bool same = existing->channels.input == channels.input &&
                          existing->channels.output == channels.output && CFEqual(existing->name, name);
        if (same && created) *created = existing->id;
        pthread_mutex_unlock(&devices_lock);
        return same ? kAudioHardwareNoError : kAudioHardwareIllegalOperationError;
    }
    Device *slot = NULL;
    for (size_t index = 0; index < PB_MAX_DEVICES; ++index) {
        Device *device = &devices[index];
        if (!is_device_locked(device)) {
            if (!slot) slot = device;
        } else if (CFStringCompare(name, device->name, kCFCompareCaseInsensitive) == kCFCompareEqualTo) {
            pthread_mutex_unlock(&devices_lock);
            return kAudioHardwareIllegalOperationError;
        }
    }
    if (!slot || next_device_id > UINT32_MAX - 3) {
        pthread_mutex_unlock(&devices_lock);
        return kAudioHardwareIllegalOperationError;
    }
    CFMutableArrayRef config = persist ? copy_configuration_locked(NULL) : NULL;
    pthread_mutex_unlock(&devices_lock);

    const UInt32 shared_channels = channels.input < channels.output ? channels.input : channels.output;
    _Atomic(UInt64) *positions = malloc(PB_RING_FRAMES * sizeof(*positions));
    _Atomic(uint32_t) *samples = malloc(PB_RING_FRAMES * shared_channels * sizeof(*samples));
    OSStatus status = kAudioHardwareNoError;
    if (!positions || !samples || (persist && !config)) {
        status = kAudioHardwareUnspecifiedError;
    } else if (persist) {
        CFDictionaryRef entry = device_record(uid, name, channels);
        if (!entry) {
            status = kAudioHardwareUnspecifiedError;
        } else {
            CFArrayAppendValue(config, entry);
            CFRelease(entry);
            status = host->WriteToStorage(host, CFSTR("devices-v1"), config);
        }
    }
    if (config) CFRelease(config);
    if (status != kAudioHardwareNoError) {
        free(positions);
        free(samples);
        return status;
    }

    for (size_t index = 0; index < PB_RING_FRAMES; ++index) {
        atomic_init(&positions[index], PB_EMPTY_FRAME);
    }
    for (size_t index = 0; index < PB_RING_FRAMES * shared_channels; ++index) {
        atomic_init(&samples[index], 0);
    }
    pthread_mutex_lock(&devices_lock);
    slot->id = next_device_id;
    next_device_id += 3;
    slot->uid = CFRetain(uid);
    slot->name = CFRetain(name);
    slot->channels = channels;
    slot->shared_channels = shared_channels;
    slot->positions = positions;
    slot->samples = samples;
    atomic_store_explicit(&slot->anchor, mach_absolute_time(), memory_order_relaxed);
    atomic_store_explicit(&slot->clock_generation, 2, memory_order_relaxed);
    atomic_store_explicit(&slot->running, 0, memory_order_relaxed);
    atomic_store_explicit(&slot->claim, device_claim(slot->id), memory_order_release);
    if (created) *created = slot->id;
    pthread_mutex_unlock(&devices_lock);
    return kAudioHardwareNoError;
}

static OSStatus remove_device(CFStringRef uid, AudioObjectID expected) {
    pthread_mutex_lock(&devices_lock);
    Device *device = find_uid_locked(uid);
    if (expected != kAudioObjectUnknown && (!device || device->id != expected)) {
        pthread_mutex_unlock(&devices_lock);
        return kAudioHardwareBadDeviceError;
    }
    if (!device) {
        pthread_mutex_unlock(&devices_lock);
        return kAudioHardwareNoError;
    }
    UInt64 claim = device_claim(device->id);
    if (atomic_load_explicit(&device->running, memory_order_relaxed) != 0 ||
        !atomic_compare_exchange_strong_explicit(&device->claim, &claim, 0,
                                                 memory_order_acq_rel, memory_order_relaxed)) {
        pthread_mutex_unlock(&devices_lock);
        return kAudioHardwareIllegalOperationError;
    }
    CFArrayRef config = copy_configuration_locked(device);
    pthread_mutex_unlock(&devices_lock);
    OSStatus status = config ? host->WriteToStorage(host, CFSTR("devices-v1"), config)
                             : kAudioHardwareUnspecifiedError;
    if (config) CFRelease(config);

    pthread_mutex_lock(&devices_lock);
    if (status == kAudioHardwareNoError) {
        CFRelease(device->uid);
        CFRelease(device->name);
        free(device->positions);
        free(device->samples);
        device->id = kAudioObjectUnknown;
        device->uid = NULL;
        device->name = NULL;
        device->positions = NULL;
        device->samples = NULL;
    } else {
        atomic_store_explicit(&device->claim, device_claim(device->id), memory_order_release);
    }
    pthread_mutex_unlock(&devices_lock);
    return status;
}

OSStatus PBApplyCommand(CFPropertyListRef command) {
    if (!command || CFGetTypeID(command) != CFDictionaryGetTypeID()) {
        return kAudioHardwareIllegalOperationError;
    }
    const CFStringRef action = CFDictionaryGetValue(command, CFSTR("action"));
    if (!action || CFGetTypeID(action) != CFStringGetTypeID()) {
        return kAudioHardwareIllegalOperationError;
    }
    CFStringRef uid = CFDictionaryGetValue(command, CFSTR("id"));
    CFStringRef name = NULL;
    ChannelCounts channels = {0};
    const bool creating = CFEqual(action, CFSTR("create"));
    if (creating ? !get_record(command, &uid, &name, &channels, false) :
        (!CFEqual(action, CFSTR("delete")) || !valid_uid(uid))) {
        return kAudioHardwareIllegalOperationError;
    }

    pthread_mutex_lock(&configuration_lock);
    const OSStatus status = !host || !host->WriteToStorage ? kAudioHardwareIllegalOperationError
        : creating ? create_device(uid, name, channels, true, NULL) : remove_device(uid, kAudioObjectUnknown);
    pthread_mutex_unlock(&configuration_lock);
    if (status == kAudioHardwareNoError) notify_configuration();
    return status;
}

static HRESULT STDMETHODCALLTYPE query_interface(void *driver, REFIID uuid, LPVOID *out) {
    if (!out) return E_POINTER;
    *out = NULL;
    if (driver != &driver_pointer) return E_NOINTERFACE;
    CFUUIDRef requested = CFUUIDCreateFromUUIDBytes(NULL, uuid);
    if (!requested) return E_NOINTERFACE;
    const bool matches = CFEqual(requested, kAudioServerPlugInDriverInterfaceUUID) ||
                        CFEqual(requested, IUnknownUUID);
    CFRelease(requested);
    if (!matches) return E_NOINTERFACE;
    atomic_fetch_add_explicit(&references, 1, memory_order_relaxed);
    *out = &driver_pointer;
    return S_OK;
}

static ULONG STDMETHODCALLTYPE add_ref(void *driver) {
    if (driver != &driver_pointer) return 0;
    return atomic_fetch_add_explicit(&references, 1, memory_order_relaxed) + 1;
}

static ULONG STDMETHODCALLTYPE release_ref(void *driver) {
    if (driver != &driver_pointer) return 0;
    ULONG current = atomic_load_explicit(&references, memory_order_relaxed);
    while (current != 0) {
        if (atomic_compare_exchange_weak_explicit(&references, &current, current - 1,
                                                 memory_order_relaxed, memory_order_relaxed)) {
            return current - 1;
        }
    }
    return 0;
}

static OSStatus STDMETHODCALLTYPE initialize(AudioServerPlugInDriverRef driver,
                                             AudioServerPlugInHostRef new_host) {
    if (driver != &driver_pointer || !new_host) return kAudioHardwareIllegalOperationError;

    pthread_mutex_lock(&configuration_lock);
    if (host) {
        pthread_mutex_unlock(&configuration_lock);
        return host == new_host ? kAudioHardwareNoError : kAudioHardwareIllegalOperationError;
    }
    mach_timebase_info_data_t timebase;
    if (mach_timebase_info(&timebase) != KERN_SUCCESS || timebase.numer == 0) {
        pthread_mutex_unlock(&configuration_lock);
        return kAudioHardwareUnspecifiedError;
    }
    host_ticks_per_frame = (1e9 / PB_SAMPLE_RATE) * timebase.denom / timebase.numer;
    host = new_host;
    CFPropertyListRef config = NULL;
    if (host->CopyFromStorage) host->CopyFromStorage(host, CFSTR("devices-v1"), &config);
    if (config) {
        if (CFGetTypeID(config) == CFArrayGetTypeID()) {
            const CFIndex count = CFArrayGetCount(config);
            for (CFIndex index = 0; index < count && index < PB_MAX_DEVICES; ++index) {
                CFDictionaryRef entry = CFArrayGetValueAtIndex(config, index);
                CFStringRef uid, name;
                ChannelCounts channels;
                if (entry && CFGetTypeID(entry) == CFDictionaryGetTypeID() &&
                    get_record(entry, &uid, &name, &channels, true)) {
                    create_device(uid, name, channels, false, NULL);
                }
            }
        }
        CFRelease(config);
    }
    pthread_mutex_unlock(&configuration_lock);
    return kAudioHardwareNoError;
}

static OSStatus STDMETHODCALLTYPE hal_create(AudioServerPlugInDriverRef driver,
                                              CFDictionaryRef description,
                                              const AudioServerPlugInClientInfo *client,
                                              AudioObjectID *out) {
    (void)client;
    if (driver != &driver_pointer || !description || !out) return kAudioHardwareIllegalOperationError;
    *out = kAudioObjectUnknown;
    CFStringRef uid, name;
    ChannelCounts channels;
    if (!get_record(description, &uid, &name, &channels, false)) return kAudioHardwareIllegalOperationError;
    pthread_mutex_lock(&configuration_lock);
    const OSStatus status = host && host->WriteToStorage
        ? create_device(uid, name, channels, true, out) : kAudioHardwareIllegalOperationError;
    pthread_mutex_unlock(&configuration_lock);
    if (status == kAudioHardwareNoError) notify_configuration();
    return status;
}

static OSStatus STDMETHODCALLTYPE hal_destroy(AudioServerPlugInDriverRef driver, AudioObjectID id) {
    PBDeviceInfo info;
    if (driver != &driver_pointer || !PBCopyDeviceInfo(id, &info)) return kAudioHardwareBadDeviceError;
    const bool is_device = info.id == id;
    CFRelease(info.name);
    pthread_mutex_lock(&configuration_lock);
    const OSStatus status = is_device && host && host->WriteToStorage
        ? remove_device(info.uid, id) : kAudioHardwareBadDeviceError;
    pthread_mutex_unlock(&configuration_lock);
    CFRelease(info.uid);
    if (status == kAudioHardwareNoError) notify_configuration();
    return status;
}

static OSStatus STDMETHODCALLTYPE device_client(AudioServerPlugInDriverRef driver, AudioObjectID id,
                                                 const AudioServerPlugInClientInfo *client) {
    (void)client;
    if (driver != &driver_pointer) return kAudioHardwareBadObjectError;
    pthread_mutex_lock(&devices_lock);
    const bool exists = find_device_locked(id) != NULL;
    pthread_mutex_unlock(&devices_lock);
    return exists ? kAudioHardwareNoError : kAudioHardwareBadDeviceError;
}

static OSStatus STDMETHODCALLTYPE configuration_change(AudioServerPlugInDriverRef driver,
                                                        AudioObjectID id, UInt64 change,
                                                        void *info) {
    (void)driver;
    (void)id;
    (void)change;
    (void)info;
    // Sample rate and stream layout are fixed for the lifetime of an interface.
    return kAudioHardwareUnsupportedOperationError;
}

static OSStatus STDMETHODCALLTYPE start_io(AudioServerPlugInDriverRef driver, AudioObjectID id,
                                            UInt32 client) {
    (void)client;
    if (driver != &driver_pointer) return kAudioHardwareBadObjectError;
    pthread_mutex_lock(&devices_lock);
    Device *device = find_device_locked(id);
    if (!device || !atomic_load_explicit(&device->claim, memory_order_relaxed)) {
        pthread_mutex_unlock(&devices_lock);
        return kAudioHardwareBadDeviceError;
    }
    UInt32 running = atomic_load_explicit(&device->running, memory_order_relaxed);
    if (running == UINT32_MAX) {
        pthread_mutex_unlock(&devices_lock);
        return kAudioHardwareIllegalOperationError;
    }
    if (running == 0) {
        atomic_fetch_add_explicit(&device->clock_generation, 1, memory_order_seq_cst);
        atomic_store_explicit(&device->anchor, mach_absolute_time(), memory_order_seq_cst);
        atomic_fetch_add_explicit(&device->clock_generation, 1, memory_order_seq_cst);
        for (size_t index = 0; index < PB_RING_FRAMES; ++index) {
            atomic_store_explicit(&device->positions[index], PB_EMPTY_FRAME, memory_order_relaxed);
        }
    }
    atomic_store_explicit(&device->running, running + 1, memory_order_release);
    pthread_mutex_unlock(&devices_lock);
    if (running == 0) notify_running(id);
    return kAudioHardwareNoError;
}

static OSStatus STDMETHODCALLTYPE stop_io(AudioServerPlugInDriverRef driver, AudioObjectID id,
                                           UInt32 client) {
    (void)client;
    if (driver != &driver_pointer) return kAudioHardwareBadObjectError;
    pthread_mutex_lock(&devices_lock);
    Device *device = find_device_locked(id);
    if (!device) {
        pthread_mutex_unlock(&devices_lock);
        return kAudioHardwareBadDeviceError;
    }
    UInt32 running = atomic_load_explicit(&device->running, memory_order_relaxed);
    if (running == 0) {
        pthread_mutex_unlock(&devices_lock);
        return kAudioHardwareIllegalOperationError;
    }
    atomic_store_explicit(&device->running, running - 1, memory_order_release);
    pthread_mutex_unlock(&devices_lock);
    if (running == 1) notify_running(id);
    return kAudioHardwareNoError;
}

static OSStatus STDMETHODCALLTYPE zero_stamp(AudioServerPlugInDriverRef driver, AudioObjectID id,
                                              UInt32 client, Float64 *sample, UInt64 *ticks,
                                              UInt64 *seed) {
    (void)client;
    if (driver != &driver_pointer) return kAudioHardwareBadObjectError;
    Device *device = acquire_device(id);
    if (!device) return kAudioHardwareBadDeviceError;

    OSStatus status = kAudioHardwareNotRunningError;
    for (int attempt = 0; attempt < 3; ++attempt) {
        const UInt64 generation = atomic_load_explicit(&device->clock_generation, memory_order_seq_cst);
        const UInt64 anchor = atomic_load_explicit(&device->anchor, memory_order_seq_cst);
        if ((generation & 1) ||
            atomic_load_explicit(&device->clock_generation, memory_order_seq_cst) != generation) continue;
        const UInt64 now = mach_absolute_time();
        const Float64 elapsed = now > anchor ? (Float64)(now - anchor) : 0.0;
        const Float64 period_ticks = host_ticks_per_frame * PB_ZERO_TIMESTAMP_PERIOD;
        const Float64 period = floor(elapsed / period_ticks);
        *sample = period * PB_ZERO_TIMESTAMP_PERIOD;
        *ticks = anchor + (UInt64)(period * period_ticks);
        *seed = generation / 2;
        status = kAudioHardwareNoError;
        break;
    }
    release_device(device);
    return status;
}

static OSStatus STDMETHODCALLTYPE will_do_io(AudioServerPlugInDriverRef driver, AudioObjectID id,
                                              UInt32 client, UInt32 operation, Boolean *will_do,
                                              Boolean *in_place) {
    (void)client;
    if (driver != &driver_pointer) return kAudioHardwareBadObjectError;
    Device *device = acquire_device(id);
    if (!device) return kAudioHardwareBadDeviceError;
    *will_do = operation == kAudioServerPlugInIOOperationReadInput ||
               operation == kAudioServerPlugInIOOperationWriteMix;
    *in_place = true;
    release_device(device);
    return kAudioHardwareNoError;
}

static OSStatus STDMETHODCALLTYPE io_boundary(AudioServerPlugInDriverRef driver, AudioObjectID id,
                                               UInt32 client, UInt32 operation, UInt32 frames,
                                               const AudioServerPlugInIOCycleInfo *cycle) {
    (void)id;
    (void)client;
    (void)operation;
    (void)frames;
    (void)cycle;
    return driver == &driver_pointer ? kAudioHardwareNoError : kAudioHardwareBadObjectError;
}

static void write_frames(Device *device, Float64 sample_time, UInt32 count, const Float32 *input) {
    if (!isfinite(sample_time) || sample_time <= -0x1p62 || sample_time >= 0x1p62) return;
    const SInt64 first = (SInt64)floor(sample_time);
    for (UInt32 frame = 0; frame < count; ++frame) {
        const SInt64 position = first + frame;
        if (position < 0) continue;
        const size_t slot = (size_t)position % PB_RING_FRAMES;
        atomic_store_explicit(&device->positions[slot], PB_EMPTY_FRAME, memory_order_seq_cst);
        for (UInt32 channel = 0; channel < device->shared_channels; ++channel) {
            const size_t index = (size_t)frame * device->channels.output + channel;
            uint32_t bits;
            memcpy(&bits, &input[index], sizeof(bits));
            atomic_store_explicit(&device->samples[slot * device->shared_channels + channel],
                                  bits, memory_order_seq_cst);
        }
        atomic_store_explicit(&device->positions[slot], (UInt64)position, memory_order_seq_cst);
    }
}

static void read_frames(Device *device, Float64 sample_time, UInt32 count, Float32 *output) {
    if (!isfinite(sample_time) || sample_time <= -0x1p62 || sample_time >= 0x1p62) {
        memset(output, 0, (size_t)count * device->channels.input * sizeof(*output));
        return;
    }
    const SInt64 first = (SInt64)floor(sample_time) - PB_INPUT_LATENCY;
    for (UInt32 frame = 0; frame < count; ++frame) {
        const SInt64 position = first + frame;
        Float32 *destination = &output[(size_t)frame * device->channels.input];
        if (position >= 0) {
            const size_t slot = (size_t)position % PB_RING_FRAMES;
            if (atomic_load_explicit(&device->positions[slot], memory_order_seq_cst) == (UInt64)position) {
                for (UInt32 channel = 0; channel < device->shared_channels; ++channel) {
                    const uint32_t bits = atomic_load_explicit(
                        &device->samples[slot * device->shared_channels + channel], memory_order_seq_cst);
                    memcpy(&destination[channel], &bits, sizeof(bits));
                }
                if (atomic_load_explicit(&device->positions[slot], memory_order_seq_cst) == (UInt64)position) {
                    memset(destination + device->shared_channels, 0,
                           (device->channels.input - device->shared_channels) * sizeof(*destination));
                    continue;
                }
            }
        }
        memset(destination, 0, device->channels.input * sizeof(*destination));
    }
}

static OSStatus STDMETHODCALLTYPE do_io(AudioServerPlugInDriverRef driver, AudioObjectID id,
                                         AudioObjectID stream, UInt32 client, UInt32 operation,
                                         UInt32 count, const AudioServerPlugInIOCycleInfo *cycle,
                                         void *main_buffer, void *secondary_buffer) {
    (void)client;
    (void)secondary_buffer;
    if (driver != &driver_pointer) return kAudioHardwareBadObjectError;
    if (!main_buffer || !cycle || count > PB_RING_FRAMES) return kAudioHardwareIllegalOperationError;
    Device *device = acquire_device(id);
    if (!device) return kAudioHardwareBadDeviceError;
    OSStatus status = kAudioHardwareNoError;
    if (atomic_load_explicit(&device->running, memory_order_acquire) == 0) {
        status = kAudioHardwareNotRunningError;
    } else if (operation == kAudioServerPlugInIOOperationWriteMix && stream == id + 2) {
        write_frames(device, cycle->mOutputTime.mSampleTime, count, main_buffer);
    } else if (operation == kAudioServerPlugInIOOperationReadInput && stream == id + 1) {
        read_frames(device, cycle->mInputTime.mSampleTime, count, main_buffer);
    } else {
        status = kAudioHardwareIllegalOperationError;
    }
    release_device(device);
    return status;
}

static AudioServerPlugInDriverInterface driver_interface = {
    .QueryInterface = query_interface,
    .AddRef = add_ref,
    .Release = release_ref,
    .Initialize = initialize,
    .CreateDevice = hal_create,
    .DestroyDevice = hal_destroy,
    .AddDeviceClient = device_client,
    .RemoveDeviceClient = device_client,
    .PerformDeviceConfigurationChange = configuration_change,
    .AbortDeviceConfigurationChange = configuration_change,
    .HasProperty = PBHasProperty,
    .IsPropertySettable = PBIsPropertySettable,
    .GetPropertyDataSize = PBGetPropertyDataSize,
    .GetPropertyData = PBGetPropertyData,
    .SetPropertyData = PBSetPropertyData,
    .StartIO = start_io,
    .StopIO = stop_io,
    .GetZeroTimeStamp = zero_stamp,
    .WillDoIOOperation = will_do_io,
    .BeginIOOperation = io_boundary,
    .DoIOOperation = do_io,
    .EndIOOperation = io_boundary,
};

__attribute__((visibility("default")))
void *Patchbay_Create(CFAllocatorRef allocator, CFUUIDRef requested_type) {
    (void)allocator;
    if (!requested_type || !CFEqual(requested_type, kAudioServerPlugInTypeUUID)) return NULL;
    add_ref(&driver_pointer);
    return &driver_pointer;
}
