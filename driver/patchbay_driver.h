#ifndef PATCHBAY_DRIVER_H
#define PATCHBAY_DRIVER_H

#include <CoreAudio/AudioServerPlugIn.h>
#include <stdbool.h>
#include <stddef.h>

#define PB_BUNDLE_ID "fr.patchbay.audio.driver"
#define PB_UID_PREFIX "fr.patchbay.audio.native."
#define PB_CONFIG_PROPERTY ((AudioObjectPropertySelector) 'pbcf')
#define PB_MAX_DEVICES 16
#define PB_MAX_CHANNELS 64
#define PB_SAMPLE_RATE 48000.0
#define PB_INPUT_LATENCY 512
#define PB_ZERO_TIMESTAMP_PERIOD 16384

typedef struct {
    AudioObjectID id;
    AudioObjectID input_stream;
    AudioObjectID output_stream;
    CFStringRef uid;
    CFStringRef name;
    UInt32 input_channels;
    UInt32 output_channels;
    bool running;
} PBDeviceInfo;

// Returns the current device count and copies at most capacity identifiers.
size_t PBListDeviceIDs(AudioObjectID *ids, size_t capacity);
// Accepts a device or one of its streams. The caller releases uid and name.
bool PBCopyDeviceInfo(AudioObjectID object, PBDeviceInfo *out);
CFArrayRef PBCopyConfiguration(void);
OSStatus PBApplyCommand(CFPropertyListRef command);

void *Patchbay_Create(CFAllocatorRef allocator, CFUUIDRef requested_type);

Boolean PBHasProperty(AudioServerPlugInDriverRef driver, AudioObjectID object, pid_t client,
                      const AudioObjectPropertyAddress *address);
OSStatus PBIsPropertySettable(AudioServerPlugInDriverRef driver, AudioObjectID object, pid_t client,
                              const AudioObjectPropertyAddress *address, Boolean *settable);
OSStatus PBGetPropertyDataSize(AudioServerPlugInDriverRef driver, AudioObjectID object, pid_t client,
                               const AudioObjectPropertyAddress *address, UInt32 qualifier_size,
                               const void *qualifier, UInt32 *size);
OSStatus PBGetPropertyData(AudioServerPlugInDriverRef driver, AudioObjectID object, pid_t client,
                           const AudioObjectPropertyAddress *address, UInt32 qualifier_size,
                           const void *qualifier, UInt32 capacity, UInt32 *size, void *data);
OSStatus PBSetPropertyData(AudioServerPlugInDriverRef driver, AudioObjectID object, pid_t client,
                           const AudioObjectPropertyAddress *address, UInt32 qualifier_size,
                           const void *qualifier, UInt32 size, const void *data);

#endif
