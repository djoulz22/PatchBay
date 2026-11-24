#include "patchbay_driver.h"

#include <string.h>

enum { BoxObject = 2 };
#define BoxUID CFSTR(PB_BUNDLE_ID ".box")

typedef struct {
    bool fetch;
    UInt32 size;
    UInt32 item_size;
    CFTypeRef owned;
    union {
        UInt32 integer;
        Float64 real;
        CFTypeRef object;
        AudioObjectID ids[PB_MAX_DEVICES + 1];
        AudioServerPlugInCustomPropertyInfo custom;
        AudioValueRange range;
        AudioStreamBasicDescription format;
        AudioStreamRangedDescription ranged_format;
        AudioChannelLayout layout;
    } data;
} Property;

static OSStatus integer(Property *value, UInt32 number) {
    value->data.integer = number;
    value->size = sizeof(value->data.integer);
    return kAudioHardwareNoError;
}

static OSStatus object(Property *value, CFTypeRef reference) {
    value->size = sizeof(value->data.object);
    if (value->fetch) {
        value->owned = CFRetain(reference);
        value->data.object = value->owned;
    }
    return kAudioHardwareNoError;
}

static OSStatus ids(Property *value, size_t count) {
    value->item_size = sizeof(AudioObjectID);
    value->size = (UInt32)(count * value->item_size);
    return kAudioHardwareNoError;
}

static size_t copy_device_ids(AudioObjectID *objects) {
    const size_t count = PBListDeviceIDs(objects, PB_MAX_DEVICES);
    return count > PB_MAX_DEVICES ? PB_MAX_DEVICES : count;
}

static bool matches_class(UInt32 size, const void *qualifier, AudioClassID wanted) {
    if (size == 0) return true;
    if (!qualifier || size % sizeof(AudioClassID)) return false;
    const unsigned char *bytes = qualifier;
    for (size_t offset = 0; offset < size; offset += sizeof(AudioClassID)) {
        AudioClassID type;
        memcpy(&type, bytes + offset, sizeof(type));
        if (type == wanted || type == kAudioObjectClassID || type == kAudioObjectClassIDWildcard) {
            return true;
        }
    }
    return false;
}

static OSStatus translate_uid(Property *value, UInt32 size, const void *qualifier,
                              AudioObjectPropertySelector selector) {
    if (!value->fetch) return integer(value, kAudioObjectUnknown);
    if (size != sizeof(CFStringRef) || !qualifier) return kAudioHardwareBadPropertySizeError;
    CFStringRef uid;
    memcpy(&uid, qualifier, sizeof(uid));
    if (!uid || CFGetTypeID(uid) != CFStringGetTypeID()) return kAudioHardwareIllegalOperationError;
    if (selector == kAudioPlugInPropertyTranslateUIDToBox) {
        return integer(value, CFEqual(uid, BoxUID) ? BoxObject : kAudioObjectUnknown);
    }
    if (selector == kAudioPlugInPropertyTranslateUIDToDevice) {
        AudioObjectID devices[PB_MAX_DEVICES];
        const size_t count = copy_device_ids(devices);
        for (size_t index = 0; index < count; index++) {
            PBDeviceInfo device;
            if (!PBCopyDeviceInfo(devices[index], &device)) continue;
            const bool equal = CFEqual(uid, device.uid);
            CFRelease(device.uid);
            CFRelease(device.name);
            if (equal) return integer(value, device.id);
        }
    }
    return integer(value, kAudioObjectUnknown);
}

static OSStatus plugin_property(AudioObjectPropertySelector selector, UInt32 qualifier_size,
                                const void *qualifier, Property *value) {
    switch (selector) {
    case kAudioObjectPropertyBaseClass: return integer(value, kAudioObjectClassID);
    case kAudioObjectPropertyClass: return integer(value, kAudioPlugInClassID);
    case kAudioObjectPropertyOwner: return integer(value, kAudioObjectUnknown);
    case kAudioObjectPropertyName:
    case kAudioObjectPropertyManufacturer: return object(value, CFSTR("Patchbay"));
    case kAudioPlugInPropertyBundleID: return object(value, CFSTR(PB_BUNDLE_ID));
    case kAudioPlugInPropertyResourceBundle: return object(value, CFSTR(""));
    case kAudioObjectPropertyOwnedObjects: {
        size_t count = 0;
        if (matches_class(qualifier_size, qualifier, kAudioBoxClassID)) {
            value->data.ids[count++] = BoxObject;
        }
        if (matches_class(qualifier_size, qualifier, kAudioDeviceClassID)) {
            count += copy_device_ids(value->data.ids + count);
        }
        return ids(value, count);
    }
    case kAudioPlugInPropertyDeviceList: return ids(value, copy_device_ids(value->data.ids));
    case kAudioPlugInPropertyBoxList:
        value->data.ids[0] = BoxObject;
        return ids(value, 1);
    case kAudioPlugInPropertyClockDeviceList: return ids(value, 0);
    case kAudioPlugInPropertyTranslateUIDToDevice:
    case kAudioPlugInPropertyTranslateUIDToBox:
    case kAudioPlugInPropertyTranslateUIDToClockDevice:
        return translate_uid(value, qualifier_size, qualifier, selector);
    case kAudioObjectPropertyCustomPropertyInfoList:
        value->data.custom = (AudioServerPlugInCustomPropertyInfo){
            .mSelector = PB_CONFIG_PROPERTY,
            .mPropertyDataType = kAudioServerPlugInCustomPropertyDataTypeCFPropertyList,
            .mQualifierDataType = kAudioServerPlugInCustomPropertyDataTypeNone,
        };
        value->size = sizeof(value->data.custom);
        return kAudioHardwareNoError;
    case PB_CONFIG_PROPERTY:
        value->size = sizeof(value->data.object);
        if (value->fetch) {
            value->owned = PBCopyConfiguration();
            if (!value->owned) return kAudioHardwareUnspecifiedError;
            value->data.object = value->owned;
        }
        return kAudioHardwareNoError;
    default: return kAudioHardwareUnknownPropertyError;
    }
}

static OSStatus box_property(AudioObjectPropertySelector selector, Property *value) {
    switch (selector) {
    case kAudioObjectPropertyBaseClass: return integer(value, kAudioObjectClassID);
    case kAudioObjectPropertyClass: return integer(value, kAudioBoxClassID);
    case kAudioObjectPropertyOwner: return integer(value, kAudioObjectPlugInObject);
    case kAudioObjectPropertyName:
    case kAudioObjectPropertyManufacturer: return object(value, CFSTR("Patchbay"));
    case kAudioObjectPropertyOwnedObjects:
    case kAudioBoxPropertyClockDeviceList: return ids(value, 0);
    case kAudioBoxPropertyBoxUID: return object(value, BoxUID);
    case kAudioBoxPropertyTransportType: return integer(value, kAudioDeviceTransportTypeVirtual);
    case kAudioBoxPropertyHasAudio:
    case kAudioBoxPropertyAcquired: return integer(value, 1);
    case kAudioBoxPropertyHasVideo:
    case kAudioBoxPropertyHasMIDI:
    case kAudioBoxPropertyIsProtected:
    case kAudioBoxPropertyAcquisitionFailed: return integer(value, 0);
    case kAudioBoxPropertyDeviceList: return ids(value, copy_device_ids(value->data.ids));
    default: return kAudioHardwareUnknownPropertyError;
    }
}

static OSStatus device_property(const PBDeviceInfo *device, const AudioObjectPropertyAddress *address,
                                UInt32 qualifier_size, const void *qualifier, Property *value) {
    const bool global = address->mScope == kAudioObjectPropertyScopeGlobal;
    const bool input = address->mScope == kAudioObjectPropertyScopeInput;
    const bool output = address->mScope == kAudioObjectPropertyScopeOutput;
    if ((!global && !input && !output) || address->mElement != kAudioObjectPropertyElementMain) {
        return kAudioHardwareUnknownPropertyError;
    }
    const UInt32 channels = input ? device->input_channels : device->output_channels;

    switch (address->mSelector) {
    case kAudioObjectPropertyBaseClass: return integer(value, kAudioObjectClassID);
    case kAudioObjectPropertyClass: return integer(value, kAudioDeviceClassID);
    case kAudioObjectPropertyOwner: return integer(value, kAudioObjectPlugInObject);
    case kAudioObjectPropertyName: return object(value, device->name);
    case kAudioObjectPropertyManufacturer: return object(value, CFSTR("Patchbay"));
    case kAudioObjectPropertyModelName: return object(value, CFSTR("Interface audio virtuelle"));
    case kAudioDevicePropertyDeviceUID: return object(value, device->uid);
    case kAudioDevicePropertyModelUID: return object(value, CFSTR(PB_BUNDLE_ID ".device"));
    case kAudioDevicePropertyTransportType: return integer(value, kAudioDeviceTransportTypeVirtual);
    case kAudioDevicePropertyRelatedDevices:
        value->data.ids[0] = device->id;
        return ids(value, 1);
    case kAudioDevicePropertyClockDomain:
    case kAudioDevicePropertySafetyOffset:
    case kAudioDevicePropertyIsHidden: return integer(value, 0);
    case kAudioDevicePropertyDeviceIsAlive:
    case kAudioDevicePropertyClockIsStable:
    case kAudioDevicePropertyDeviceCanBeDefaultDevice: return integer(value, 1);
    case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice: return integer(value, !input);
    case kAudioDevicePropertyDeviceIsRunning: return integer(value, device->running);
    case kAudioDevicePropertyLatency: return integer(value, input ? PB_INPUT_LATENCY : 0);
    case kAudioDevicePropertyClockAlgorithm: return integer(value, kAudioDeviceClockAlgorithmRaw);
    case kAudioDevicePropertyZeroTimeStampPeriod: return integer(value, PB_ZERO_TIMESTAMP_PERIOD);
    case kAudioObjectPropertyControlList: return ids(value, 0);
    case kAudioObjectPropertyOwnedObjects:
        if (!matches_class(qualifier_size, qualifier, kAudioStreamClassID)) return ids(value, 0);
        /* fall through */
    case kAudioDevicePropertyStreams: {
        size_t count = 0;
        if (global || input) value->data.ids[count++] = device->input_stream;
        if (global || output) value->data.ids[count++] = device->output_stream;
        return ids(value, count);
    }
    case kAudioDevicePropertyNominalSampleRate:
        value->data.real = PB_SAMPLE_RATE;
        value->size = sizeof(value->data.real);
        return kAudioHardwareNoError;
    case kAudioDevicePropertyAvailableNominalSampleRates:
        value->data.range = (AudioValueRange){PB_SAMPLE_RATE, PB_SAMPLE_RATE};
        value->size = value->item_size = sizeof(value->data.range);
        return kAudioHardwareNoError;
    case kAudioDevicePropertyPreferredChannelsForStereo:
        if (global || channels < 2) return kAudioHardwareUnknownPropertyError;
        value->data.ids[0] = 1;
        value->data.ids[1] = 2;
        value->size = 2 * sizeof(UInt32);
        return kAudioHardwareNoError;
    case kAudioDevicePropertyPreferredChannelLayout:
        if (global) return kAudioHardwareUnknownPropertyError;
        value->data.layout = (AudioChannelLayout){
            .mChannelLayoutTag = channels == 1 ? kAudioChannelLayoutTag_Mono :
                                 channels == 2 ? kAudioChannelLayoutTag_Stereo :
                                 kAudioChannelLayoutTag_DiscreteInOrder | channels,
        };
        value->size = offsetof(AudioChannelLayout, mChannelDescriptions);
        return kAudioHardwareNoError;
    default: return kAudioHardwareUnknownPropertyError;
    }
}

static AudioStreamBasicDescription stream_format(UInt32 channels) {
    return (AudioStreamBasicDescription){
        .mSampleRate = PB_SAMPLE_RATE,
        .mFormatID = kAudioFormatLinearPCM,
        .mFormatFlags = kAudioFormatFlagsNativeFloatPacked,
        .mBytesPerPacket = channels * sizeof(Float32),
        .mFramesPerPacket = 1,
        .mBytesPerFrame = channels * sizeof(Float32),
        .mChannelsPerFrame = channels,
        .mBitsPerChannel = 8 * sizeof(Float32),
    };
}

static OSStatus stream_property(const PBDeviceInfo *device, bool input,
                                AudioObjectPropertySelector selector, Property *value) {
    const UInt32 channels = input ? device->input_channels : device->output_channels;
    switch (selector) {
    case kAudioObjectPropertyBaseClass: return integer(value, kAudioObjectClassID);
    case kAudioObjectPropertyClass: return integer(value, kAudioStreamClassID);
    case kAudioObjectPropertyOwner: return integer(value, device->id);
    case kAudioObjectPropertyOwnedObjects: return ids(value, 0);
    case kAudioStreamPropertyDirection: return integer(value, input);
    case kAudioStreamPropertyTerminalType: return integer(value, kAudioStreamTerminalTypeLine);
    case kAudioStreamPropertyStartingChannel:
    case kAudioStreamPropertyIsActive: return integer(value, 1);
    case kAudioStreamPropertyLatency: return integer(value, 0);
    case kAudioStreamPropertyPhysicalFormat:
    case kAudioStreamPropertyVirtualFormat:
        value->data.format = stream_format(channels);
        value->size = sizeof(value->data.format);
        return kAudioHardwareNoError;
    case kAudioStreamPropertyAvailablePhysicalFormats:
    case kAudioStreamPropertyAvailableVirtualFormats:
        value->data.ranged_format = (AudioStreamRangedDescription){
            .mFormat = stream_format(channels),
            .mSampleRateRange = {PB_SAMPLE_RATE, PB_SAMPLE_RATE},
        };
        value->size = value->item_size = sizeof(value->data.ranged_format);
        return kAudioHardwareNoError;
    default: return kAudioHardwareUnknownPropertyError;
    }
}

static OSStatus property(AudioObjectID id, const AudioObjectPropertyAddress *address,
                         UInt32 qualifier_size, const void *qualifier, Property *value) {
    if (!address) return kAudioHardwareIllegalOperationError;
    const bool root_scope = address->mScope == kAudioObjectPropertyScopeGlobal &&
                            address->mElement == kAudioObjectPropertyElementMain;
    if (id == kAudioObjectPlugInObject) {
        return root_scope ? plugin_property(address->mSelector, qualifier_size, qualifier, value) :
                            kAudioHardwareUnknownPropertyError;
    }
    if (id == BoxObject) {
        return root_scope ? box_property(address->mSelector, value) : kAudioHardwareUnknownPropertyError;
    }
    PBDeviceInfo device;
    if (!PBCopyDeviceInfo(id, &device)) return kAudioHardwareBadObjectError;
    OSStatus result;
    if (id == device.id) {
        result = device_property(&device, address, qualifier_size, qualifier, value);
    } else if (root_scope) {
        result = stream_property(&device, id == device.input_stream, address->mSelector, value);
    } else {
        result = kAudioHardwareUnknownPropertyError;
    }
    CFRelease(device.uid);
    CFRelease(device.name);
    return result;
}

Boolean PBHasProperty(AudioServerPlugInDriverRef driver, AudioObjectID id, pid_t client,
                      const AudioObjectPropertyAddress *address) {
    (void)driver;
    (void)client;
    Property value = {0};
    return property(id, address, 0, NULL, &value) == kAudioHardwareNoError;
}

OSStatus PBIsPropertySettable(AudioServerPlugInDriverRef driver, AudioObjectID id, pid_t client,
                              const AudioObjectPropertyAddress *address, Boolean *settable) {
    (void)driver;
    (void)client;
    if (!settable) return kAudioHardwareIllegalOperationError;
    Property value = {0};
    const OSStatus status = property(id, address, 0, NULL, &value);
    if (status == kAudioHardwareNoError) {
        *settable = id == kAudioObjectPlugInObject && address->mSelector == PB_CONFIG_PROPERTY;
    }
    return status;
}

OSStatus PBGetPropertyDataSize(AudioServerPlugInDriverRef driver, AudioObjectID id, pid_t client,
                               const AudioObjectPropertyAddress *address, UInt32 qualifier_size,
                               const void *qualifier, UInt32 *size) {
    (void)driver;
    (void)client;
    if (!size) return kAudioHardwareIllegalOperationError;
    Property value = {0};
    const OSStatus status = property(id, address, qualifier_size, qualifier, &value);
    if (status == kAudioHardwareNoError) *size = value.size;
    return status;
}

OSStatus PBGetPropertyData(AudioServerPlugInDriverRef driver, AudioObjectID id, pid_t client,
                           const AudioObjectPropertyAddress *address, UInt32 qualifier_size,
                           const void *qualifier, UInt32 capacity, UInt32 *size, void *data) {
    (void)driver;
    (void)client;
    if (!size) return kAudioHardwareIllegalOperationError;
    *size = 0;
    Property value = {.fetch = true};
    OSStatus status = property(id, address, qualifier_size, qualifier, &value);
    if (status == kAudioHardwareNoError && capacity < value.size) {
        if (value.item_size) {
            // The HAL may have looked up the list size just before another client added a device.
            value.size = capacity - capacity % value.item_size;
        } else {
            status = kAudioHardwareBadPropertySizeError;
        }
    }
    if (status == kAudioHardwareNoError && value.size && !data) {
        status = kAudioHardwareIllegalOperationError;
    }
    if (status != kAudioHardwareNoError) {
        if (value.owned) CFRelease(value.owned);
        return status;
    }
    if (value.size) memcpy(data, &value.data, value.size);
    *size = value.size;
    return kAudioHardwareNoError;
}

OSStatus PBSetPropertyData(AudioServerPlugInDriverRef driver, AudioObjectID id, pid_t client,
                           const AudioObjectPropertyAddress *address, UInt32 qualifier_size,
                           const void *qualifier, UInt32 size, const void *data) {
    Boolean settable = false;
    const OSStatus status = PBIsPropertySettable(driver, id, client, address, &settable);
    if (status != kAudioHardwareNoError) return status;
    if (!settable) return kAudioHardwareUnsupportedOperationError;
    if (qualifier_size || qualifier || size != sizeof(CFPropertyListRef)) {
        return kAudioHardwareBadPropertySizeError;
    }
    if (!data) return kAudioHardwareIllegalOperationError;
    CFPropertyListRef command;
    memcpy(&command, data, sizeof(command));
    if (!command || CFGetTypeID(command) != CFDictionaryGetTypeID()) {
        return kAudioHardwareIllegalOperationError;
    }
    return PBApplyCommand(command);
}
