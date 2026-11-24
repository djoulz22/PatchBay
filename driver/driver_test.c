#include "patchbay_driver.h"
#include <assert.h>
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>

static CFPropertyListRef stored;
static bool storage_fails;

static OSStatus changed(AudioServerPlugInHostRef host, AudioObjectID id, UInt32 count,
                        const AudioObjectPropertyAddress *addresses) {
    (void)host; (void)id; (void)count; (void)addresses;
    return noErr;
}

static OSStatus load(AudioServerPlugInHostRef host, CFStringRef key, CFPropertyListRef *out) {
    (void)host; (void)key;
    *out = stored ? CFRetain(stored) : NULL;
    return noErr;
}

static OSStatus save(AudioServerPlugInHostRef host, CFStringRef key, CFPropertyListRef value) {
    (void)host; (void)key;
    if (storage_fails) return kAudioHardwareUnspecifiedError;
    if (stored) CFRelease(stored);
    stored = CFRetain(value);
    return noErr;
}

static OSStatus command(AudioServerPlugInDriverRef driver, CFStringRef action, CFStringRef uid,
                        CFStringRef name, int input_channels, int output_channels) {
    CFNumberRef inputs = CFNumberCreate(NULL, kCFNumberIntType, &input_channels);
    CFNumberRef outputs = CFNumberCreate(NULL, kCFNumberIntType, &output_channels);
    const void *keys[] = {CFSTR("action"), CFSTR("id"), CFSTR("name"),
                          CFSTR("inputChannels"), CFSTR("outputChannels")};
    const void *values[] = {action, uid, name, inputs, outputs};
    CFIndex length = CFEqual(action, CFSTR("create")) ? 5 : 2;
    CFDictionaryRef value = CFDictionaryCreate(NULL, keys, values, length,
                                               &kCFTypeDictionaryKeyCallBacks,
                                               &kCFTypeDictionaryValueCallBacks);
    const AudioObjectPropertyAddress address = {PB_CONFIG_PROPERTY, kAudioObjectPropertyScopeGlobal, 0};
    OSStatus status = (*driver)->SetPropertyData(driver, kAudioObjectPlugInObject, 10, &address,
                                                 0, NULL, sizeof(value), &value);
    CFRelease(value);
    CFRelease(inputs);
    CFRelease(outputs);
    return status;
}

static UInt32 property(AudioServerPlugInDriverRef driver, AudioObjectID object,
                       AudioObjectPropertySelector selector, AudioObjectPropertyScope scope,
                       UInt32 capacity, void *result) {
    const AudioObjectPropertyAddress address = {selector, scope, 0};
    UInt32 size = 0;
    assert((*driver)->HasProperty(driver, object, 10, &address));
    assert((*driver)->GetPropertyData(driver, object, 10, &address, 0, NULL,
                                      capacity, &size, result) == noErr);
    return size;
}

static AudioObjectID check_stream(AudioServerPlugInDriverRef driver, AudioObjectID device,
                                  bool input, UInt32 channels) {
    AudioObjectID stream = 0;
    assert(property(driver, device, kAudioDevicePropertyStreams,
                    input ? kAudioObjectPropertyScopeInput : kAudioObjectPropertyScopeOutput,
                    sizeof(stream), &stream) == sizeof(stream));
    UInt32 direction = 99;
    property(driver, stream, kAudioStreamPropertyDirection, kAudioObjectPropertyScopeGlobal,
             sizeof(direction), &direction);
    assert(direction == (UInt32)input);
    AudioStreamBasicDescription format;
    property(driver, stream, kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal,
             sizeof(format), &format);
    assert(format.mChannelsPerFrame == channels && format.mSampleRate == PB_SAMPLE_RATE);
    assert(format.mFormatID == kAudioFormatLinearPCM && (format.mFormatFlags & kAudioFormatFlagIsFloat));
    assert(format.mBytesPerFrame == channels * sizeof(float));
    return stream;
}

static void io(AudioServerPlugInDriverRef driver, AudioObjectID device, AudioObjectID stream,
               bool input, Float64 sample_time, UInt32 frames, float *buffer) {
    AudioServerPlugInIOCycleInfo cycle = {0};
    cycle.mInputTime.mFlags = cycle.mOutputTime.mFlags = kAudioTimeStampSampleTimeValid;
    cycle.mInputTime.mSampleTime = cycle.mOutputTime.mSampleTime = sample_time;
    UInt32 operation = input ? kAudioServerPlugInIOOperationReadInput : kAudioServerPlugInIOOperationWriteMix;
    assert((*driver)->DoIOOperation(driver, device, stream, 1, operation, frames, &cycle,
                                   buffer, NULL) == noErr);
}

int main(int argc, char **argv) {
    assert(argc == 2);
    void *image = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!image) { fprintf(stderr, "%s\n", dlerror()); return 1; }
    void *(*factory)(CFAllocatorRef, CFUUIDRef) = dlsym(image, "Patchbay_Create");
    assert(factory);
    AudioServerPlugInDriverRef driver = factory(NULL, kAudioServerPlugInTypeUUID);
    assert(driver);
    AudioServerPlugInHostInterface host = { .PropertiesChanged = changed,
                                            .CopyFromStorage = load, .WriteToStorage = save };
    assert((*driver)->Initialize(driver, &host) == noErr);
    CFStringRef uid_a = CFSTR(PB_UID_PREFIX "34343434-3434-4434-a434-343434343434");
    CFStringRef uid_b = CFSTR(PB_UID_PREFIX "56565656-5656-4565-a656-565656565656");
    storage_fails = true;
    assert(command(driver, CFSTR("create"), uid_a, CFSTR("Patchbay test A"), 2, 8) != noErr);
    AudioObjectID devices[PB_MAX_DEVICES];
    assert(property(driver, kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList,
                    kAudioObjectPropertyScopeGlobal, sizeof(devices), devices) == 0);
    storage_fails = false;
    assert(command(driver, CFSTR("create"), uid_a, CFSTR("Hors limites"), 2, 65) != noErr);
    assert(command(driver, CFSTR("create"), uid_a, CFSTR("Hors limites"), 0, 2) != noErr);
    assert(command(driver, CFSTR("create"), uid_a, CFSTR("Patchbay test A"), 2, 8) == noErr);
    assert(command(driver, CFSTR("create"), uid_b, CFSTR("Patchbay test B"), 64, 2) == noErr);
    assert(command(driver, CFSTR("create"), uid_b, CFSTR("Doublon"), 2, 2) != noErr);
    assert(CFArrayGetCount(stored) == 2);
    assert(property(driver, kAudioObjectPlugInObject, kAudioPlugInPropertyDeviceList,
                    kAudioObjectPropertyScopeGlobal, sizeof(devices), devices) == 2 * sizeof(AudioObjectID));
    AudioObjectID device_a = 0, device_b = 0;
    for (size_t i = 0; i < 2; i++) {
        CFStringRef uid;
        property(driver, devices[i], kAudioDevicePropertyDeviceUID, kAudioObjectPropertyScopeGlobal,
                 sizeof(uid), &uid);
        if (CFEqual(uid, uid_a)) device_a = devices[i];
        if (CFEqual(uid, uid_b)) device_b = devices[i];
        CFRelease(uid);
    }
    assert(device_a && device_b && device_a != device_b);
    const AudioObjectID a_in = check_stream(driver, device_a, true, 2);
    const AudioObjectID a_out = check_stream(driver, device_a, false, 8);
    const AudioObjectID b_in = check_stream(driver, device_b, true, 64);
    const AudioObjectID b_out = check_stream(driver, device_b, false, 2);
    assert((*driver)->StartIO(driver, device_a, 1) == noErr);
    assert((*driver)->StartIO(driver, device_b, 1) == noErr);
    assert(command(driver, CFSTR("delete"), uid_b, NULL, 0, 0) != noErr);
    Float64 sample;
    UInt64 ticks, seed;
    assert((*driver)->GetZeroTimeStamp(driver, device_a, 1, &sample, &ticks, &seed) == noErr);
    assert(sample >= 0 && ticks != 0 && seed != 0);
    enum { frames = 4 };
    float a_sent[8 * frames] = { [0] = 0.125f, [1] = -0.25f, [7] = 0.875f, [9] = 0.375f };
    const float a_expected[2 * frames] = {0.125f, -0.25f, 0, 0.375f};
    float a_received[2 * frames] = {0};
    float b_sent[2 * frames] = { [1] = 0.625f, [2] = -0.75f };
    const float b_expected[64 * frames] = { [1] = 0.625f, [64] = -0.75f };
    float b_received[64 * frames] = {0};
    const Float64 write_at = 16382;
    io(driver, device_a, a_out, false, write_at, frames, a_sent);
    io(driver, device_b, b_out, false, write_at, frames, b_sent);
    io(driver, device_a, a_in, true, write_at + PB_INPUT_LATENCY - 1, 1, a_received);
    assert(a_received[0] == 0 && a_received[1] == 0);
    io(driver, device_a, a_in, true, write_at + PB_INPUT_LATENCY, frames, a_received);
    io(driver, device_b, b_in, true, write_at + PB_INPUT_LATENCY, frames, b_received);
    assert(memcmp(a_expected, a_received, sizeof(a_expected)) == 0);
    assert(memcmp(b_expected, b_received, sizeof(b_expected)) == 0);
    assert((*driver)->StopIO(driver, device_a, 1) == noErr);
    assert((*driver)->StartIO(driver, device_a, 1) == noErr);
    float silence[2 * frames] = {0};
    io(driver, device_a, a_in, true, write_at + PB_INPUT_LATENCY, frames, a_received);
    assert(memcmp(silence, a_received, sizeof(silence)) == 0);
    assert((*driver)->StopIO(driver, device_a, 1) == noErr);
    assert((*driver)->StopIO(driver, device_b, 1) == noErr);
    assert(command(driver, CFSTR("delete"), uid_a, NULL, 0, 0) == noErr);
    assert(CFArrayGetCount(stored) == 1);
    assert(command(driver, CFSTR("delete"), uid_b, NULL, 0, 0) == noErr);
    assert(CFArrayGetCount(stored) == 0);
    CFRelease(stored);
    puts("Deux interfaces isolées, 2 entrées / 8 sorties et 64 entrées / 2 sorties, PCM vérifié.");
    return 0;
}
