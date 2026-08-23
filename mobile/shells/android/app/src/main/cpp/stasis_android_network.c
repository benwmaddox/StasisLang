#include <stdint.h>

#ifndef STASIS_NETWORK_ENABLED
#define STASIS_NETWORK_ENABLED 0
#endif

#if STASIS_NETWORK_ENABLED
#include "stasis_network.h"
int32_t stasis_android_network_abi_version(void) {
    return (int32_t)stasis_network_abi_version();
}
#else
int32_t stasis_android_network_abi_version(void) { return 0; }
#endif
