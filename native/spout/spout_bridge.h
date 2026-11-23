/* CaptureView Spout send bridge — thin C ABI over Spout2 SpoutLibrary.dll (BSD). */
#pragma once

#ifdef __cplusplus
extern "C" {
#endif

#ifdef SPOUT_BRIDGE_EXPORTS
#define SPOUT_BRIDGE_API __declspec(dllexport)
#else
#define SPOUT_BRIDGE_API __declspec(dllimport)
#endif

typedef struct SpoutBridge SpoutBridge;

/* dll_dir: folder containing SpoutLibrary.dll (may be NULL → LoadLibrary path search). */
SPOUT_BRIDGE_API SpoutBridge* spout_bridge_create(const char* dll_dir);
SPOUT_BRIDGE_API void spout_bridge_destroy(SpoutBridge* bridge);

/* Send BGRA8 (top-left origin). Creates/updates sender named `name`. Returns 1 on success. */
SPOUT_BRIDGE_API int spout_bridge_send_bgra(
    SpoutBridge* bridge,
    const char* name,
    const unsigned char* pixels,
    unsigned int width,
    unsigned int height);

/* Optional diagnostic (never NULL; static empty string if unused). */
SPOUT_BRIDGE_API const char* spout_bridge_last_error(SpoutBridge* bridge);

#ifdef __cplusplus
}
#endif
