/*
 * CaptureView Spout bridge — loads SpoutLibrary.dll (BSD-2-Clause) and sends BGRA via SendImage.
 * SpoutLibrary is OpenGL-backed; we create a hidden GL context and prefer CPU share for reliability
 * with wgpu (no shared D3D texture yet).
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "spout_bridge.h"

/* SpoutLibrary.h #defines SPOUTLIBRARY_EXPORTS; force empty SPOUTAPI (we LoadLibrary GetSpout). */
#ifdef SPOUTLIBRARY_EXPORTS
#undef SPOUTLIBRARY_EXPORTS
#endif
#define SPOUT_BUILD_STATIC
#include "../../third_party/spout/SpoutLibrary.h"

#ifndef GL_BGRA_EXT
#define GL_BGRA_EXT 0x80E1
#endif

typedef SPOUTHANDLE(WINAPI* PFN_GetSpout)(VOID);

struct SpoutBridge {
    HMODULE module = nullptr;
    SPOUTHANDLE spout = nullptr;
    bool gl_ready = false;
    std::string last_error;
    char name_cache[256]{};
};

static void set_err(SpoutBridge* b, const char* msg) {
    if (b) {
        b->last_error = msg ? msg : "";
    }
}

static std::wstring widen(const char* utf8) {
    if (!utf8 || !*utf8) {
        return std::wstring();
    }
    int n = MultiByteToWideChar(CP_UTF8, 0, utf8, -1, nullptr, 0);
    if (n <= 0) {
        return std::wstring();
    }
    std::wstring out(static_cast<size_t>(n), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, utf8, -1, &out[0], n);
    if (!out.empty() && out.back() == L'\0') {
        out.pop_back();
    }
    return out;
}

extern "C" SPOUT_BRIDGE_API SpoutBridge* spout_bridge_create(const char* dll_dir) {
    auto* b = new SpoutBridge();

    wchar_t path[MAX_PATH]{};
    if (dll_dir && dll_dir[0]) {
        std::wstring wdir = widen(dll_dir);
        if (!wdir.empty()) {
            _snwprintf_s(path, _TRUNCATE, L"%s\\SpoutLibrary.dll", wdir.c_str());
            b->module = LoadLibraryW(path);
        }
    }
    if (!b->module) {
        b->module = LoadLibraryW(L"SpoutLibrary.dll");
    }
    if (!b->module) {
        set_err(b, "LoadLibrary SpoutLibrary.dll failed");
        return b;
    }

    auto get_spout = reinterpret_cast<PFN_GetSpout>(GetProcAddress(b->module, "GetSpout"));
    if (!get_spout) {
        set_err(b, "GetProcAddress(GetSpout) failed");
        return b;
    }

    b->spout = get_spout();
    if (!b->spout) {
        set_err(b, "GetSpout() returned null");
        return b;
    }

    /* Prefer CPU path so SendImage works without sharing GL/DX with wgpu. */
    b->spout->SetCPUshare(true);
    if (!b->spout->CreateOpenGL(nullptr)) {
        set_err(b, "CreateOpenGL failed — Spout sender unavailable");
        return b;
    }
    b->gl_ready = true;
    set_err(b, "");
    return b;
}

extern "C" SPOUT_BRIDGE_API void spout_bridge_destroy(SpoutBridge* bridge) {
    if (!bridge) {
        return;
    }
    if (bridge->spout) {
        bridge->spout->ReleaseSender(0);
        if (bridge->gl_ready) {
            bridge->spout->CloseOpenGL();
        }
        bridge->spout->Release();
        bridge->spout = nullptr;
    }
    if (bridge->module) {
        FreeLibrary(bridge->module);
        bridge->module = nullptr;
    }
    delete bridge;
}

extern "C" SPOUT_BRIDGE_API int spout_bridge_send_bgra(
    SpoutBridge* bridge,
    const char* name,
    const unsigned char* pixels,
    unsigned int width,
    unsigned int height) {
    if (!bridge || !bridge->spout || !bridge->gl_ready) {
        if (bridge) {
            set_err(bridge, "Spout bridge not ready");
        }
        return 0;
    }
    if (!pixels || width == 0 || height == 0) {
        set_err(bridge, "invalid frame");
        return 0;
    }
    const char* sender = (name && name[0]) ? name : "KapturKard";
    if (std::strncmp(bridge->name_cache, sender, sizeof(bridge->name_cache)) != 0) {
        bridge->spout->SetSenderName(sender);
        std::strncpy(bridge->name_cache, sender, sizeof(bridge->name_cache) - 1);
        bridge->name_cache[sizeof(bridge->name_cache) - 1] = '\0';
    }
    const bool ok = bridge->spout->SendImage(pixels, width, height, GL_BGRA_EXT, false);
    if (!ok) {
        set_err(bridge, "SendImage failed");
        return 0;
    }
    set_err(bridge, "");
    return 1;
}

extern "C" SPOUT_BRIDGE_API const char* spout_bridge_last_error(SpoutBridge* bridge) {
    if (!bridge) {
        return "";
    }
    return bridge->last_error.c_str();
}
