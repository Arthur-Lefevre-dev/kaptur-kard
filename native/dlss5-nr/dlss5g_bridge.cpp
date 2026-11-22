// SPDX-License-Identifier: MIT
// CaptureView DLSS-G bridge — NeuralScreen-style EvaluateFeature + dedicated present queue.
// Public NGX Feature_FrameGeneration (11) params only. No NeuralScreen PolyForm source.
// Proprietary nvngx_dlssg.dll is BYO — never vendored.

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <d3d11.h>
#include <d3d12.h>
#include <dxgi1_4.h>
#include <wrl/client.h>

#include <algorithm>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <string>
#include <vector>

#include "nvof_flow.h"

using Microsoft::WRL::ComPtr;
using NGXResult = int;
static constexpr NGXResult NGX_SUCCESS = 1;
static constexpr int FG_FEATURE_ID = 11; // NVSDK_NGX_Feature_FrameGeneration
static constexpr unsigned long long APP_ID = 141959980ULL;
static constexpr const char* PROJECT_ID = "53f803cc-a12f-4d69-90d5-19b7599cad19";
static constexpr unsigned MAX_GENERATED = 5; // ×6 → 5 intermediates

struct NGXHandle { unsigned int Id; };

struct NGXParameter {
    virtual void Set(const char*, unsigned long long) = 0;
    virtual void Set(const char*, float) = 0;
    virtual void Set(const char*, double) = 0;
    virtual void Set(const char*, unsigned int) = 0;
    virtual void Set(const char*, int) = 0;
    virtual void Set(const char*, ID3D11Resource*) = 0;
    virtual void Set(const char*, ID3D12Resource*) = 0;
    virtual void Set(const char*, void*) = 0;
    virtual NGXResult Get(const char*, unsigned long long*) const = 0;
    virtual NGXResult Get(const char*, float*) const = 0;
    virtual NGXResult Get(const char*, double*) const = 0;
    virtual NGXResult Get(const char*, unsigned int*) const = 0;
    virtual NGXResult Get(const char*, int*) const = 0;
    virtual NGXResult Get(const char*, ID3D11Resource**) const = 0;
    virtual NGXResult Get(const char*, ID3D12Resource**) const = 0;
    virtual NGXResult Get(const char*, void**) const = 0;
    virtual void Reset() = 0;
};

struct NGXPathListInfo {
    wchar_t const* const* Path;
    unsigned int Length;
};
enum NGXLoggingLevel { NGX_LOG_OFF = 0 };
using NGXLogCallback = void(__cdecl*)(const char*, NGXLoggingLevel, int);
struct NGXLoggingInfo {
    NGXLoggingLevel LoggingLevel;
    NGXLogCallback Callback;
    void* UserData;
    bool DisableOtherLoggingSinks;
};
struct NGXFeatureCommonInfoInternal;
struct NGXFeatureCommonInfo {
    NGXPathListInfo PathListInfo;
    NGXFeatureCommonInfoInternal* InternalData;
    NGXLoggingInfo LoggingInfo;
};

using InitExtFn = NGXResult(__cdecl*)(unsigned long long, const wchar_t*, ID3D12Device*, int, const void*);
using SnippetInitFn = NGXResult(__cdecl*)(unsigned long long, const wchar_t*, ID3D12Device*, const void*, int);
using InitProjectIdFn = NGXResult(__cdecl*)(const char*, int, const char*, const wchar_t*, ID3D12Device*, int, const void*);
using AllocParamsFn = NGXResult(__cdecl*)(NGXParameter**);
using CreateFeatureFn = NGXResult(__cdecl*)(ID3D12GraphicsCommandList*, int, NGXParameter*, NGXHandle**);
using EvaluateFeatureFn = NGXResult(__cdecl*)(ID3D12GraphicsCommandList*, const NGXHandle*, const NGXParameter*, void*);
using ReleaseFeatureFn = NGXResult(__cdecl*)(NGXHandle*);
using ShutdownFn = NGXResult(__cdecl*)();

static std::mutex g_mutex;
static std::string g_last_error;
static std::wstring g_runtime_dir;
static int g_gpu_index = 0;
static std::string g_gpu_name = "unknown";
static bool g_initialized = false;

static HMODULE g_core_mod = nullptr;
static HMODULE g_fg_mod = nullptr;
static InitExtFn g_core_init_ext = nullptr;
static InitProjectIdFn g_core_init_project = nullptr;
static AllocParamsFn g_alloc_params = nullptr;
static CreateFeatureFn g_core_create = nullptr;
static EvaluateFeatureFn g_core_eval = nullptr;
static ReleaseFeatureFn g_core_release = nullptr;
static ShutdownFn g_core_shutdown = nullptr;
static SnippetInitFn g_fg_init = nullptr;
static CreateFeatureFn g_fg_create = nullptr;
static EvaluateFeatureFn g_fg_eval = nullptr;
static ReleaseFeatureFn g_fg_release = nullptr;

static ComPtr<IDXGIAdapter1> g_adapter;
static ComPtr<ID3D12Device> g_device;
static ComPtr<ID3D12CommandQueue> g_work_queue;    // EvaluateFeature
static ComPtr<ID3D12CommandQueue> g_present_queue; // HIGH — NeuralScreen-style present/copy
static ComPtr<ID3D12CommandAllocator> g_work_alloc;
static ComPtr<ID3D12CommandAllocator> g_present_alloc;
static ComPtr<ID3D12GraphicsCommandList> g_work_cmd;
static ComPtr<ID3D12GraphicsCommandList> g_present_cmd;
static ComPtr<ID3D12Fence> g_fence;
static UINT64 g_fence_value = 0;
static HANDLE g_fence_event = nullptr;

static HWND g_present_hwnd = nullptr;
static ComPtr<IDXGISwapChain3> g_swapchain;
static ComPtr<ID3D12Resource> g_swap_buffers[2];
static UINT g_swap_index = 0;
static bool g_present_queue_ready = false;

static NGXParameter* g_params = nullptr;
static NGXHandle* g_feature = nullptr;

static ComPtr<ID3D12Resource> g_backbuffer;   // current color (RGBA16F)
static ComPtr<ID3D12Resource> g_hudless;      // same as backbuffer for video
static ComPtr<ID3D12Resource> g_depth;        // flat R32F
static ComPtr<ID3D12Resource> g_mvec;
static ComPtr<ID3D12Resource> g_out_interp;
static ComPtr<ID3D12Resource> g_out_real;
static ComPtr<ID3D12Resource> g_upload_color;
static ComPtr<ID3D12Resource> g_upload_depth;
static ComPtr<ID3D12Resource> g_upload_mvec;
static ComPtr<ID3D12Resource> g_readback;
static ComPtr<ID3D12Resource> g_present_copy; // DEFAULT copy target for present queue

static UINT g_width = 0, g_height = 0;
static UINT g_color_pitch = 0, g_depth_pitch = 0, g_mvec_pitch = 0, g_read_pitch = 0;
static UINT64 g_color_bytes = 0, g_depth_bytes = 0, g_mvec_bytes = 0, g_read_bytes = 0;
static unsigned g_feature_max_gen = 0;

// Identity / ortho defaults for video (no game camera).
static float g_mat_identity[16] = {
    1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1
};

static void SetError(const char* fmt, ...) {
    char buf[4096];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    g_last_error = buf;
}

static void CopyError(char* dst, int cap) {
    if (!dst || cap <= 0) return;
    const size_t n = std::min<size_t>(g_last_error.size(), static_cast<size_t>(cap - 1));
    memcpy(dst, g_last_error.data(), n);
    dst[n] = '\0';
}

static std::wstring Join(const std::wstring& a, const std::wstring& b) {
    if (a.empty()) return b;
    wchar_t c = a.back();
    if (c == L'\\' || c == L'/') return a + b;
    return a + L"\\" + b;
}

static bool FileExists(const std::wstring& p) {
    DWORD a = GetFileAttributesW(p.c_str());
    return a != INVALID_FILE_ATTRIBUTES && !(a & FILE_ATTRIBUTE_DIRECTORY);
}

static unsigned long long FileTimeKey(const FILETIME& ft) {
    ULARGE_INTEGER u{};
    u.LowPart = ft.dwLowDateTime;
    u.HighPart = ft.dwHighDateTime;
    return u.QuadPart;
}

static HMODULE LoadCoreNGX(const std::wstring& runtime) {
    const std::wstring local = Join(runtime, L"_nvngx.dll");
    if (FileExists(local)) {
        if (HMODULE m = LoadLibraryW(local.c_str())) return m;
    }
    if (HMODULE m = LoadLibraryW(L"_nvngx.dll")) return m;

    wchar_t windows_dir[MAX_PATH] = {};
    UINT windows_len = GetWindowsDirectoryW(windows_dir, MAX_PATH);
    if (windows_len == 0 || windows_len >= MAX_PATH) return nullptr;
    const std::wstring repo = std::wstring(windows_dir) + L"\\System32\\DriverStore\\FileRepository";
    const std::wstring pat = repo + L"\\nv*.inf_*";

    struct Candidate { std::wstring path; unsigned long long stamp; };
    std::vector<Candidate> candidates;
    WIN32_FIND_DATAW fd{};
    HANDLE h = FindFirstFileW(pat.c_str(), &fd);
    if (h != INVALID_HANDLE_VALUE) {
        do {
            if (!(fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY)) continue;
            if (wcscmp(fd.cFileName, L".") == 0 || wcscmp(fd.cFileName, L"..") == 0) continue;
            std::wstring candidate = repo + L"\\" + fd.cFileName + L"\\_nvngx.dll";
            WIN32_FILE_ATTRIBUTE_DATA fad{};
            if (GetFileAttributesExW(candidate.c_str(), GetFileExInfoStandard, &fad) &&
                !(fad.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY)) {
                candidates.push_back({candidate, FileTimeKey(fad.ftLastWriteTime)});
            }
        } while (FindNextFileW(h, &fd));
        FindClose(h);
    }
    std::sort(candidates.begin(), candidates.end(),
              [](const Candidate& a, const Candidate& b) { return a.stamp > b.stamp; });
    for (const Candidate& c : candidates) {
        if (HMODULE m = LoadLibraryW(c.path.c_str())) return m;
    }
    return nullptr;
}

static LRESULT CALLBACK PresentWndProc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    return DefWindowProcW(hwnd, msg, wp, lp);
}

static bool EnsurePresentHwnd() {
    if (g_present_hwnd) return true;
    WNDCLASSW wc{};
    wc.lpfnWndProc = PresentWndProc;
    wc.hInstance = GetModuleHandleW(nullptr);
    wc.lpszClassName = L"CaptureViewDlssGPresent";
    RegisterClassW(&wc);
    g_present_hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        wc.lpszClassName,
        L"CaptureView DLSS-G Present",
        WS_POPUP,
        0, 0, 64, 64,
        nullptr, nullptr, wc.hInstance, nullptr);
    if (!g_present_hwnd) {
        SetError("CreateWindow for dedicated DXGI present surface failed: Win32 %lu", GetLastError());
        return false;
    }
    ShowWindow(g_present_hwnd, SW_HIDE);
    return true;
}

static ComPtr<ID3D12Device> CreateDevice(int nvidia_index) {
    ComPtr<IDXGIFactory4> factory;
    if (FAILED(CreateDXGIFactory1(IID_PPV_ARGS(&factory)))) return nullptr;
    int seen = 0;
    for (UINT i = 0;; ++i) {
        ComPtr<IDXGIAdapter1> adapter;
        if (factory->EnumAdapters1(i, &adapter) == DXGI_ERROR_NOT_FOUND) break;
        DXGI_ADAPTER_DESC1 desc{};
        adapter->GetDesc1(&desc);
        if ((desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) || desc.VendorId != 0x10DE) continue;
        if (seen++ != nvidia_index) continue;
        char gpu_utf8[512] = {};
        WideCharToMultiByte(CP_UTF8, 0, desc.Description, -1, gpu_utf8, static_cast<int>(sizeof(gpu_utf8)), nullptr, nullptr);
        if (gpu_utf8[0]) g_gpu_name = gpu_utf8;
        ComPtr<ID3D12Device> d;
        if (SUCCEEDED(D3D12CreateDevice(adapter.Get(), D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&d)))) {
            g_adapter = adapter;
            return d;
        }
        return nullptr;
    }
    return nullptr;
}

static bool WaitFence(UINT64 value) {
    if (g_fence->GetCompletedValue() >= value) return true;
    if (FAILED(g_fence->SetEventOnCompletion(value, g_fence_event))) return false;
    DWORD w = WaitForSingleObject(g_fence_event, 30000);
    return w == WAIT_OBJECT_0;
}

static bool ExecuteWorkAndWait() {
    HRESULT hr = g_work_cmd->Close();
    if (FAILED(hr)) { SetError("Work cmd Close failed 0x%08X", static_cast<unsigned>(hr)); return false; }
    ID3D12CommandList* lists[] = { g_work_cmd.Get() };
    g_work_queue->ExecuteCommandLists(1, lists);
    ++g_fence_value;
    if (FAILED(g_work_queue->Signal(g_fence.Get(), g_fence_value))) {
        SetError("Work queue Signal failed"); return false;
    }
    if (!WaitFence(g_fence_value)) { SetError("Timed out waiting for DLSS-G EvaluateFeature"); return false; }
    g_work_alloc->Reset();
    g_work_cmd->Reset(g_work_alloc.Get(), nullptr);
    return true;
}

static bool ExecutePresentAndWait() {
    HRESULT hr = g_present_cmd->Close();
    if (FAILED(hr)) { SetError("Present cmd Close failed 0x%08X", static_cast<unsigned>(hr)); return false; }
    ID3D12CommandList* lists[] = { g_present_cmd.Get() };
    g_present_queue->ExecuteCommandLists(1, lists);
    ++g_fence_value;
    if (FAILED(g_present_queue->Signal(g_fence.Get(), g_fence_value))) {
        SetError("Present queue Signal failed"); return false;
    }
    if (!WaitFence(g_fence_value)) { SetError("Timed out waiting for present-queue copy"); return false; }
    g_present_alloc->Reset();
    g_present_cmd->Reset(g_present_alloc.Get(), nullptr);
    return true;
}

static void WaitAllIdle() {
    if (!g_fence) return;
    if (g_work_queue) {
        ++g_fence_value;
        if (SUCCEEDED(g_work_queue->Signal(g_fence.Get(), g_fence_value)))
            WaitFence(g_fence_value);
    }
    if (g_present_queue) {
        ++g_fence_value;
        if (SUCCEEDED(g_present_queue->Signal(g_fence.Get(), g_fence_value)))
            WaitFence(g_fence_value);
    }
}

static D3D12_RESOURCE_BARRIER Barrier(ID3D12Resource* r, D3D12_RESOURCE_STATES before, D3D12_RESOURCE_STATES after) {
    D3D12_RESOURCE_BARRIER b{};
    b.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    b.Transition.pResource = r;
    b.Transition.Subresource = D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES;
    b.Transition.StateBefore = before;
    b.Transition.StateAfter = after;
    return b;
}

static ComPtr<ID3D12Resource> CreateTexture(
    UINT w, UINT h, DXGI_FORMAT format,
    D3D12_RESOURCE_STATES state, D3D12_RESOURCE_FLAGS flags) {
    D3D12_RESOURCE_DESC d{};
    d.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    d.Width = w; d.Height = h; d.DepthOrArraySize = 1; d.MipLevels = 1;
    d.Format = format;
    d.SampleDesc.Count = 1;
    d.Flags = flags;
    D3D12_HEAP_PROPERTIES hp{};
    hp.Type = D3D12_HEAP_TYPE_DEFAULT;
    ComPtr<ID3D12Resource> r;
    if (FAILED(g_device->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &d, state, nullptr, IID_PPV_ARGS(&r))))
        return nullptr;
    return r;
}

static ComPtr<ID3D12Resource> CreateLinearBuffer(UINT64 bytes, D3D12_HEAP_TYPE type, D3D12_RESOURCE_STATES state) {
    D3D12_RESOURCE_DESC d{};
    d.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    d.Width = bytes; d.Height = 1; d.DepthOrArraySize = 1; d.MipLevels = 1;
    d.SampleDesc.Count = 1; d.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    D3D12_HEAP_PROPERTIES hp{};
    hp.Type = type;
    ComPtr<ID3D12Resource> r;
    if (FAILED(g_device->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &d, state, nullptr, IID_PPV_ARGS(&r))))
        return nullptr;
    return r;
}

static uint16_t FloatToHalf(float f) {
    uint32_t x; memcpy(&x, &f, sizeof(x));
    uint32_t s = (x >> 16) & 0x8000u;
    int32_t e = static_cast<int32_t>((x >> 23) & 0xff) - 127 + 15;
    uint32_t m = x & 0x7fffffu;
    if (e <= 0) {
        if (e < -10) return static_cast<uint16_t>(s);
        m = (m | 0x800000u) >> (1 - e);
        return static_cast<uint16_t>(s | (m >> 13));
    }
    if (e >= 31) return static_cast<uint16_t>(s | 0x7c00u);
    return static_cast<uint16_t>(s | (static_cast<uint32_t>(e) << 10) | (m >> 13));
}

static float HalfToFloat(uint16_t h) {
    uint32_t s = (h >> 15) & 1, e = (h >> 10) & 0x1f, m = h & 0x3ff, x;
    if (e == 0) {
        if (m == 0) x = s << 31;
        else {
            e = 1;
            while (!(m & 0x400)) { m <<= 1; --e; }
            m &= 0x3ff;
            x = (s << 31) | ((e + 112) << 23) | (m << 13);
        }
    } else if (e == 0x1f) x = (s << 31) | 0x7f800000u | (m << 13);
    else x = (s << 31) | ((e + 112) << 23) | (m << 13);
    float f; memcpy(&f, &x, sizeof(f)); return f;
}

static bool SetupD3D12() {
    g_device = CreateDevice(g_gpu_index);
    if (!g_device) { SetError("Could not create D3D12 device for NVIDIA GPU index %d", g_gpu_index); return false; }

    D3D12_COMMAND_QUEUE_DESC work_q{};
    work_q.Type = D3D12_COMMAND_LIST_TYPE_DIRECT;
    work_q.Priority = D3D12_COMMAND_QUEUE_PRIORITY_NORMAL;
    if (FAILED(g_device->CreateCommandQueue(&work_q, IID_PPV_ARGS(&g_work_queue)))) {
        SetError("CreateCommandQueue(work) failed"); return false;
    }

    D3D12_COMMAND_QUEUE_DESC present_q{};
    present_q.Type = D3D12_COMMAND_LIST_TYPE_DIRECT;
    present_q.Priority = D3D12_COMMAND_QUEUE_PRIORITY_HIGH;
    if (FAILED(g_device->CreateCommandQueue(&present_q, IID_PPV_ARGS(&g_present_queue)))) {
        SetError("CreateCommandQueue(present HIGH) failed"); return false;
    }

    if (FAILED(g_device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&g_work_alloc)))) {
        SetError("CreateCommandAllocator(work) failed"); return false;
    }
    if (FAILED(g_device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&g_present_alloc)))) {
        SetError("CreateCommandAllocator(present) failed"); return false;
    }
    if (FAILED(g_device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, g_work_alloc.Get(), nullptr, IID_PPV_ARGS(&g_work_cmd)))) {
        SetError("CreateCommandList(work) failed"); return false;
    }
    if (FAILED(g_device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, g_present_alloc.Get(), nullptr, IID_PPV_ARGS(&g_present_cmd)))) {
        SetError("CreateCommandList(present) failed"); return false;
    }
    if (FAILED(g_device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&g_fence)))) {
        SetError("CreateFence failed"); return false;
    }
    g_fence_event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!g_fence_event) { SetError("CreateEvent failed"); return false; }
    return true;
}

static bool SetupDxgiPresentSwapchain(UINT w, UINT h) {
    g_present_queue_ready = false;
    g_swapchain.Reset();
    g_swap_buffers[0].Reset();
    g_swap_buffers[1].Reset();
    if (!EnsurePresentHwnd()) return false;

    // Resize hidden window to content size (driver schedules Presents on this chain).
    SetWindowPos(g_present_hwnd, HWND_BOTTOM, 0, 0, static_cast<int>(w), static_cast<int>(h),
                 SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_HIDEWINDOW);

    ComPtr<IDXGIFactory4> factory;
    if (FAILED(CreateDXGIFactory1(IID_PPV_ARGS(&factory)))) {
        SetError("CreateDXGIFactory1 for present swapchain failed");
        return false;
    }

    DXGI_SWAP_CHAIN_DESC1 scd{};
    scd.Width = w;
    scd.Height = h;
    scd.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    scd.SampleDesc.Count = 1;
    scd.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
    scd.BufferCount = 2;
    scd.SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD;
    scd.Flags = 0;

    ComPtr<IDXGISwapChain1> sc1;
    HRESULT hr = factory->CreateSwapChainForHwnd(
        g_present_queue.Get(), g_present_hwnd, &scd, nullptr, nullptr, &sc1);
    if (FAILED(hr)) {
        SetError("CreateSwapChainForHwnd(present queue) failed: 0x%08X", static_cast<unsigned>(hr));
        return false;
    }
    factory->MakeWindowAssociation(g_present_hwnd, DXGI_MWA_NO_ALT_ENTER);
    if (FAILED(sc1.As(&g_swapchain))) {
        SetError("QueryInterface IDXGISwapChain3 failed");
        return false;
    }
    for (UINT i = 0; i < 2; ++i) {
        if (FAILED(g_swapchain->GetBuffer(i, IID_PPV_ARGS(&g_swap_buffers[i])))) {
            SetError("GetBuffer(%u) failed", i);
            return false;
        }
    }
    g_swap_index = g_swapchain->GetCurrentBackBufferIndex();
    g_present_queue_ready = true;
    return true;
}

static bool LoadNGX() {
    g_core_mod = LoadCoreNGX(g_runtime_dir);
    if (!g_core_mod) {
        SetError("Could not load _nvngx.dll (runtime override or DriverStore)");
        return false;
    }
    const std::wstring fg_path = Join(g_runtime_dir, L"nvngx_dlssg.dll");
    if (!FileExists(fg_path)) { SetError("nvngx_dlssg.dll not found in runtime folder (BYO)"); return false; }
    g_fg_mod = LoadLibraryW(fg_path.c_str());
    if (!g_fg_mod) { SetError("LoadLibrary(nvngx_dlssg.dll) failed: Win32 %lu", GetLastError()); return false; }

    g_core_init_ext = reinterpret_cast<InitExtFn>(GetProcAddress(g_core_mod, "NVSDK_NGX_D3D12_Init_Ext"));
    g_core_init_project = reinterpret_cast<InitProjectIdFn>(GetProcAddress(g_core_mod, "NVSDK_NGX_D3D12_Init_ProjectID"));
    g_alloc_params = reinterpret_cast<AllocParamsFn>(GetProcAddress(g_core_mod, "NVSDK_NGX_D3D12_AllocateParameters"));
    g_core_create = reinterpret_cast<CreateFeatureFn>(GetProcAddress(g_core_mod, "NVSDK_NGX_D3D12_CreateFeature"));
    g_core_eval = reinterpret_cast<EvaluateFeatureFn>(GetProcAddress(g_core_mod, "NVSDK_NGX_D3D12_EvaluateFeature"));
    g_core_release = reinterpret_cast<ReleaseFeatureFn>(GetProcAddress(g_core_mod, "NVSDK_NGX_D3D12_ReleaseFeature"));
    g_core_shutdown = reinterpret_cast<ShutdownFn>(GetProcAddress(g_core_mod, "NVSDK_NGX_D3D12_Shutdown"));

    g_fg_init = reinterpret_cast<SnippetInitFn>(GetProcAddress(g_fg_mod, "NVSDK_NGX_D3D12_Init_Ext"));
    g_fg_create = reinterpret_cast<CreateFeatureFn>(GetProcAddress(g_fg_mod, "NVSDK_NGX_D3D12_CreateFeature"));
    g_fg_eval = reinterpret_cast<EvaluateFeatureFn>(GetProcAddress(g_fg_mod, "NVSDK_NGX_D3D12_EvaluateFeature"));
    g_fg_release = reinterpret_cast<ReleaseFeatureFn>(GetProcAddress(g_fg_mod, "NVSDK_NGX_D3D12_ReleaseFeature"));

    if (!g_core_init_ext || !g_alloc_params || !g_core_create || !g_core_eval || !g_core_release || !g_core_shutdown) {
        SetError("Required NGX core exports missing"); return false;
    }
    // Snippet exports preferred; fall back to core Create/Evaluate if snippet lacks them.
    if (!g_fg_init) {
        SetError("nvngx_dlssg.dll missing NVSDK_NGX_D3D12_Init_Ext"); return false;
    }
    return true;
}

static bool InitNGXSession() {
    const wchar_t* paths[1] = { g_runtime_dir.c_str() };
    NGXPathListInfo pli{ paths, 1 };
    NGXFeatureCommonInfo fci{};
    fci.PathListInfo = pli;
    fci.LoggingInfo.LoggingLevel = NGX_LOG_OFF;

    bool core_ok = false;
    if (g_core_init_project) {
        for (int ver = 0x13; ver <= 0x20 && !core_ok; ++ver) {
            NGXResult r = g_core_init_project(PROJECT_ID, 0, "0.4.0", g_runtime_dir.c_str(), g_device.Get(), ver, nullptr);
            core_ok = (r == NGX_SUCCESS);
        }
    }
    if (!core_ok) {
        for (int ver = 0x13; ver <= 0x20 && !core_ok; ++ver) {
            NGXResult r = g_core_init_ext(APP_ID, g_runtime_dir.c_str(), g_device.Get(), ver, &fci);
            core_ok = (r == NGX_SUCCESS);
        }
    }
    if (!core_ok) { SetError("NGX core init failed for API 0x13..0x20"); return false; }

    NGXResult sr = g_fg_init(APP_ID, g_runtime_dir.c_str(), g_device.Get(), &fci, 0x15);
    if (sr != NGX_SUCCESS) {
        // Alternate arg order used by some snippets (device, version, info).
        using AltInit = NGXResult(__cdecl*)(unsigned long long, const wchar_t*, ID3D12Device*, int, const void*);
        auto alt = reinterpret_cast<AltInit>(GetProcAddress(g_fg_mod, "NVSDK_NGX_D3D12_Init_Ext"));
        if (alt) sr = alt(APP_ID, g_runtime_dir.c_str(), g_device.Get(), 0x15, &fci);
    }
    if (sr != NGX_SUCCESS) {
        SetError("DLSS-G snippet Init_Ext failed: 0x%08X", static_cast<unsigned>(sr));
        return false;
    }

    NGXResult ar = g_alloc_params(&g_params);
    if (ar != NGX_SUCCESS || !g_params) {
        SetError("AllocateParameters failed: 0x%08X", static_cast<unsigned>(ar));
        return false;
    }
    return true;
}

static void ReleaseFeatureAndResources() {
    WaitAllIdle();
    if (g_feature) {
        if (g_fg_release) g_fg_release(g_feature);
        else if (g_core_release) g_core_release(g_feature);
        g_feature = nullptr;
    }
    g_backbuffer.Reset(); g_hudless.Reset(); g_depth.Reset(); g_mvec.Reset();
    g_out_interp.Reset(); g_out_real.Reset();
    g_upload_color.Reset(); g_upload_depth.Reset(); g_upload_mvec.Reset();
    g_readback.Reset(); g_present_copy.Reset();
    g_swapchain.Reset();
    g_swap_buffers[0].Reset(); g_swap_buffers[1].Reset();
    g_width = g_height = 0;
    g_feature_max_gen = 0;
    g_present_queue_ready = false;
}

static bool UploadRgbToColor(const float* rgb, bool execute) {
    void* mapped = nullptr;
    if (FAILED(g_upload_color->Map(0, nullptr, &mapped)) || !mapped) {
        SetError("color upload Map failed"); return false;
    }
    auto* dst = static_cast<uint8_t*>(mapped);
    for (UINT y = 0; y < g_height; ++y) {
        auto* row = reinterpret_cast<uint16_t*>(dst + static_cast<size_t>(y) * g_color_pitch);
        for (UINT x = 0; x < g_width; ++x) {
            const size_t i = (static_cast<size_t>(y) * g_width + x) * 3;
            row[x * 4 + 0] = FloatToHalf(rgb[i + 0]);
            row[x * 4 + 1] = FloatToHalf(rgb[i + 1]);
            row[x * 4 + 2] = FloatToHalf(rgb[i + 2]);
            row[x * 4 + 3] = FloatToHalf(1.0f);
        }
    }
    g_upload_color->Unmap(0, nullptr);

    auto to_copy = Barrier(g_backbuffer.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
    g_work_cmd->ResourceBarrier(1, &to_copy);
    D3D12_TEXTURE_COPY_LOCATION dstLoc{};
    dstLoc.pResource = g_backbuffer.Get();
    dstLoc.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    D3D12_TEXTURE_COPY_LOCATION srcLoc{};
    srcLoc.pResource = g_upload_color.Get();
    srcLoc.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    srcLoc.PlacedFootprint.Footprint.Format = DXGI_FORMAT_R16G16B16A16_FLOAT;
    srcLoc.PlacedFootprint.Footprint.Width = g_width;
    srcLoc.PlacedFootprint.Footprint.Height = g_height;
    srcLoc.PlacedFootprint.Footprint.Depth = 1;
    srcLoc.PlacedFootprint.Footprint.RowPitch = g_color_pitch;
    g_work_cmd->CopyTextureRegion(&dstLoc, 0, 0, 0, &srcLoc, nullptr);
    // Keep hudless in sync (video has no separate HUD layer).
    auto hud_to_copy = Barrier(g_hudless.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
    g_work_cmd->ResourceBarrier(1, &hud_to_copy);
    D3D12_TEXTURE_COPY_LOCATION hudDst = dstLoc;
    hudDst.pResource = g_hudless.Get();
    g_work_cmd->CopyTextureRegion(&hudDst, 0, 0, 0, &srcLoc, nullptr);

    D3D12_RESOURCE_BARRIER after[2] = {
        Barrier(g_backbuffer.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE),
        Barrier(g_hudless.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE),
    };
    g_work_cmd->ResourceBarrier(2, after);
    return !execute || ExecuteWorkAndWait();
}

static bool UploadFlatDepth(bool execute) {
    void* mapped = nullptr;
    if (FAILED(g_upload_depth->Map(0, nullptr, &mapped)) || !mapped) {
        SetError("depth upload Map failed"); return false;
    }
    auto* dst = static_cast<uint8_t*>(mapped);
    for (UINT y = 0; y < g_height; ++y) {
        auto* row = reinterpret_cast<float*>(dst + static_cast<size_t>(y) * g_depth_pitch);
        for (UINT x = 0; x < g_width; ++x) row[x] = 1.0f; // flat depth (NeuralScreen-style for 2D)
    }
    g_upload_depth->Unmap(0, nullptr);

    auto to_copy = Barrier(g_depth.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
    g_work_cmd->ResourceBarrier(1, &to_copy);
    D3D12_TEXTURE_COPY_LOCATION dstLoc{};
    dstLoc.pResource = g_depth.Get();
    dstLoc.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    D3D12_TEXTURE_COPY_LOCATION srcLoc{};
    srcLoc.pResource = g_upload_depth.Get();
    srcLoc.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    srcLoc.PlacedFootprint.Footprint.Format = DXGI_FORMAT_R32_FLOAT;
    srcLoc.PlacedFootprint.Footprint.Width = g_width;
    srcLoc.PlacedFootprint.Footprint.Height = g_height;
    srcLoc.PlacedFootprint.Footprint.Depth = 1;
    srcLoc.PlacedFootprint.Footprint.RowPitch = g_depth_pitch;
    g_work_cmd->CopyTextureRegion(&dstLoc, 0, 0, 0, &srcLoc, nullptr);
    auto to_srv = Barrier(g_depth.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
    g_work_cmd->ResourceBarrier(1, &to_srv);
    return !execute || ExecuteWorkAndWait();
}

static bool UploadMotionVectors(const NvofFlowFrame* flow, bool execute) {
    void* mapped = nullptr;
    if (FAILED(g_upload_mvec->Map(0, nullptr, &mapped)) || !mapped) {
        SetError("mvec upload Map failed"); return false;
    }
    memset(mapped, 0, static_cast<size_t>(g_mvec_bytes));
    if (flow && flow->has_flow && flow->width && flow->height &&
        flow->xy.size() >= static_cast<size_t>(flow->width) * flow->height * 2) {
        auto* dst = static_cast<uint8_t*>(mapped);
        for (UINT y = 0; y < g_height; ++y) {
            auto* row = reinterpret_cast<uint16_t*>(dst + static_cast<size_t>(y) * g_mvec_pitch);
            const UINT cy = std::min<UINT>(
                static_cast<UINT>((static_cast<uint64_t>(y) * flow->height) / g_height),
                flow->height - 1);
            for (UINT x = 0; x < g_width; ++x) {
                const UINT cx = std::min<UINT>(
                    static_cast<UINT>((static_cast<uint64_t>(x) * flow->width) / g_width),
                    flow->width - 1);
                const size_t i = (static_cast<size_t>(cy) * flow->width + cx) * 2;
                const float uvx = static_cast<float>(flow->xy[i + 0]) * (1.0f / 32.0f) / static_cast<float>(g_width);
                const float uvy = static_cast<float>(flow->xy[i + 1]) * (1.0f / 32.0f) / static_cast<float>(g_height);
                row[x * 2 + 0] = FloatToHalf(uvx);
                row[x * 2 + 1] = FloatToHalf(uvy);
            }
        }
    }
    g_upload_mvec->Unmap(0, nullptr);

    auto to_copy = Barrier(g_mvec.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
    g_work_cmd->ResourceBarrier(1, &to_copy);
    D3D12_TEXTURE_COPY_LOCATION dstLoc{};
    dstLoc.pResource = g_mvec.Get();
    dstLoc.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    D3D12_TEXTURE_COPY_LOCATION srcLoc{};
    srcLoc.pResource = g_upload_mvec.Get();
    srcLoc.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    srcLoc.PlacedFootprint.Footprint.Format = DXGI_FORMAT_R16G16_FLOAT;
    srcLoc.PlacedFootprint.Footprint.Width = g_width;
    srcLoc.PlacedFootprint.Footprint.Height = g_height;
    srcLoc.PlacedFootprint.Footprint.Depth = 1;
    srcLoc.PlacedFootprint.Footprint.RowPitch = g_mvec_pitch;
    g_work_cmd->CopyTextureRegion(&dstLoc, 0, 0, 0, &srcLoc, nullptr);
    auto to_srv = Barrier(g_mvec.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
    g_work_cmd->ResourceBarrier(1, &to_srv);
    return !execute || ExecuteWorkAndWait();
}

static void SetEvalParams(unsigned multi_count, unsigned multi_index, unsigned reset) {
    g_params->Set("Width", g_width);
    g_params->Set("Height", g_height);
    g_params->Set("DLSSG.Width", g_width);
    g_params->Set("DLSSG.Height", g_height);
    g_params->Set("DLSSG.BackbufferFormat", static_cast<unsigned>(DXGI_FORMAT_R16G16B16A16_FLOAT));
    g_params->Set("DLSSG.MultiFrameCount", multi_count);
    g_params->Set("DLSSG.MultiFrameIndex", multi_index);
    g_params->Set("DLSSG.Reset", reset);

    g_params->Set("DLSSG.Backbuffer", g_backbuffer.Get());
    g_params->Set("DLSSG.HUDLess", g_hudless.Get());
    g_params->Set("DLSSG.Depth", g_depth.Get());
    g_params->Set("DLSSG.MVecs", g_mvec.Get());
    g_params->Set("DLSSG.OutputInterpolated", g_out_interp.Get());
    g_params->Set("DLSSG.OutputReal", g_out_real.Get());

    g_params->Set("DLSSG.CameraViewToClip", static_cast<void*>(g_mat_identity));
    g_params->Set("DLSSG.ClipToCameraView", static_cast<void*>(g_mat_identity));
    g_params->Set("DLSSG.ClipToLensClip", static_cast<void*>(g_mat_identity));
    g_params->Set("DLSSG.ClipToPrevClip", static_cast<void*>(g_mat_identity));
    g_params->Set("DLSSG.PrevClipToClip", static_cast<void*>(g_mat_identity));

    g_params->Set("DLSSG.JitterOffsetX", 0.0f);
    g_params->Set("DLSSG.JitterOffsetY", 0.0f);
    g_params->Set("DLSSG.MvecScaleX", static_cast<float>(g_width));
    g_params->Set("DLSSG.MvecScaleY", static_cast<float>(g_height));
    g_params->Set("DLSSG.CameraNear", 0.1f);
    g_params->Set("DLSSG.CameraFar", 1000.0f);
    g_params->Set("DLSSG.CameraFOV", 1.0f);
    g_params->Set("DLSSG.CameraAspectRatio", static_cast<float>(g_width) / static_cast<float>(std::max(1u, g_height)));
    g_params->Set("DLSSG.ColorBuffersHDR", 0u);
    g_params->Set("DLSSG.DepthInverted", 0u);
    g_params->Set("DLSSG.CameraMotionIncluded", 1u);
    g_params->Set("DLSSG.NotRenderingGameFrames", 0u);
    g_params->Set("DLSSG.OrthoProjection", 1u);
    g_params->Set("DLSSG.MvecDilated", 0u);
}

static bool AllocateResources(UINT w, UINT h, unsigned max_gen) {
    g_backbuffer = CreateTexture(w, h, DXGI_FORMAT_R16G16B16A16_FLOAT,
        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    g_hudless = CreateTexture(w, h, DXGI_FORMAT_R16G16B16A16_FLOAT,
        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    g_depth = CreateTexture(w, h, DXGI_FORMAT_R32_FLOAT,
        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_FLAG_NONE);
    g_mvec = CreateTexture(w, h, DXGI_FORMAT_R16G16_FLOAT,
        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_FLAG_NONE);
    g_out_interp = CreateTexture(w, h, DXGI_FORMAT_R16G16B16A16_FLOAT,
        D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    g_out_real = CreateTexture(w, h, DXGI_FORMAT_R16G16B16A16_FLOAT,
        D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    g_present_copy = CreateTexture(w, h, DXGI_FORMAT_R8G8B8A8_UNORM,
        D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_FLAG_NONE);
    if (!g_backbuffer || !g_hudless || !g_depth || !g_mvec || !g_out_interp || !g_out_real || !g_present_copy) {
        SetError("Failed to create DLSS-G D3D12 textures"); return false;
    }

    g_color_pitch = (w * 8u + 255u) & ~255u;
    g_depth_pitch = (w * 4u + 255u) & ~255u;
    g_mvec_pitch = (w * 4u + 255u) & ~255u;
    g_read_pitch = (w * 8u + 255u) & ~255u;
    g_color_bytes = static_cast<UINT64>(g_color_pitch) * h;
    g_depth_bytes = static_cast<UINT64>(g_depth_pitch) * h;
    g_mvec_bytes = static_cast<UINT64>(g_mvec_pitch) * h;
    g_read_bytes = static_cast<UINT64>(g_read_pitch) * h;

    g_upload_color = CreateLinearBuffer(g_color_bytes, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    g_upload_depth = CreateLinearBuffer(g_depth_bytes, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    g_upload_mvec = CreateLinearBuffer(g_mvec_bytes, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    g_readback = CreateLinearBuffer(g_read_bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    if (!g_upload_color || !g_upload_depth || !g_upload_mvec || !g_readback) {
        SetError("Failed to create DLSS-G upload/readback buffers"); return false;
    }

    g_width = w;
    g_height = h;
    g_feature_max_gen = max_gen;

    if (!SetupDxgiPresentSwapchain(w, h)) {
        // EvaluateFeature can still run; DXGI Present on the HIGH queue is best-effort.
        g_last_error.clear();
        g_present_queue_ready = false;
    }

    // CreateFeature with max multi-frame count.
    g_params->Set("Width", w);
    g_params->Set("Height", h);
    g_params->Set("DLSSG.Width", w);
    g_params->Set("DLSSG.Height", h);
    g_params->Set("DLSSG.BackbufferFormat", static_cast<unsigned>(DXGI_FORMAT_R16G16B16A16_FLOAT));
    g_params->Set("DLSSG.MultiFrameCountMax", max_gen);
    g_params->Set("CreationNodeMask", 1u);
    g_params->Set("VisibilityNodeMask", 1u);

    CreateFeatureFn create_fn = g_fg_create ? g_fg_create : g_core_create;
    NGXResult r = create_fn(g_work_cmd.Get(), FG_FEATURE_ID, g_params, &g_feature);
    if (!ExecuteWorkAndWait()) return false;
    if (r != NGX_SUCCESS || !g_feature) {
        SetError("CreateFeature(FrameGeneration=11) failed: 0x%08X — check GPU/driver/BYO nvngx_dlssg.dll",
                 static_cast<unsigned>(r));
        return false;
    }
    return true;
}

static bool EnsureFeature(UINT w, UINT h, unsigned max_gen) {
    if (g_feature && w == g_width && h == g_height && max_gen <= g_feature_max_gen)
        return true;
    ReleaseFeatureAndResources();
    return AllocateResources(w, h, max_gen);
}

static bool ReadbackInterp(float* out_rgb) {
    // Transition output → copy → readback on work queue, then present-queue copies to DXGI + Present.
    auto to_copy = Barrier(g_out_interp.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    g_work_cmd->ResourceBarrier(1, &to_copy);
    D3D12_TEXTURE_COPY_LOCATION src{};
    src.pResource = g_out_interp.Get();
    src.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    D3D12_TEXTURE_COPY_LOCATION dst{};
    dst.pResource = g_readback.Get();
    dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    dst.PlacedFootprint.Footprint.Format = DXGI_FORMAT_R16G16B16A16_FLOAT;
    dst.PlacedFootprint.Footprint.Width = g_width;
    dst.PlacedFootprint.Footprint.Height = g_height;
    dst.PlacedFootprint.Footprint.Depth = 1;
    dst.PlacedFootprint.Footprint.RowPitch = g_read_pitch;
    g_work_cmd->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
    auto back = Barrier(g_out_interp.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    g_work_cmd->ResourceBarrier(1, &back);
    if (!ExecuteWorkAndWait()) return false;

    void* mapped = nullptr;
    D3D12_RANGE range{ 0, static_cast<SIZE_T>(g_read_bytes) };
    if (FAILED(g_readback->Map(0, &range, &mapped)) || !mapped) {
        SetError("readback Map failed"); return false;
    }
    auto* srcb = static_cast<const uint8_t*>(mapped);
    for (UINT y = 0; y < g_height; ++y) {
        auto* row = reinterpret_cast<const uint16_t*>(srcb + static_cast<size_t>(y) * g_read_pitch);
        for (UINT x = 0; x < g_width; ++x) {
            const size_t o = (static_cast<size_t>(y) * g_width + x) * 3;
            out_rgb[o + 0] = HalfToFloat(row[x * 4 + 0]);
            out_rgb[o + 1] = HalfToFloat(row[x * 4 + 1]);
            out_rgb[o + 2] = HalfToFloat(row[x * 4 + 2]);
        }
    }
    D3D12_RANGE written{ 0, 0 };
    g_readback->Unmap(0, &written);
    return true;
}

static bool PresentInterpolatedOnDxgi() {
    if (!g_present_queue_ready || !g_swapchain) return true; // soft skip

    // Wait for work queue to finish Evaluate before present-queue copy.
    ++g_fence_value;
    if (FAILED(g_work_queue->Signal(g_fence.Get(), g_fence_value))) return false;
    if (FAILED(g_present_queue->Wait(g_fence.Get(), g_fence_value))) return false;

    g_swap_index = g_swapchain->GetCurrentBackBufferIndex();
    ID3D12Resource* back = g_swap_buffers[g_swap_index].Get();
    if (!back) return false;

    // Copy RGBA16F interp → R8G8B8A8 swap buffer via present_copy intermediate is heavy;
    // for pacing we Present a clear-sized flip: copy out_interp UAV → present path uses
    // a resource barrier + CopyResource when formats match. Formats differ, so skip pixel
    // copy and still call Present for queue pacing (NeuralScreen separates present queue).
    auto to_present = Barrier(back, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_PRESENT);
    (void)to_present;
    // Empty submit keeps the HIGH present queue live with the swapchain.
    if (!ExecutePresentAndWait()) return false;
    HRESULT hr = g_swapchain->Present(0, 0);
    if (FAILED(hr)) {
        SetError("IDXGISwapChain::Present on dedicated queue failed: 0x%08X", static_cast<unsigned>(hr));
        return false;
    }
    return true;
}

static void ShutdownUnlocked() {
    ReleaseFeatureAndResources();
    NvofShutdown();
    if (g_core_shutdown) g_core_shutdown();
    g_params = nullptr;
    if (g_fence_event) { CloseHandle(g_fence_event); g_fence_event = nullptr; }
    g_work_cmd.Reset(); g_present_cmd.Reset();
    g_work_alloc.Reset(); g_present_alloc.Reset();
    g_work_queue.Reset(); g_present_queue.Reset();
    g_fence.Reset(); g_device.Reset(); g_adapter.Reset();
    if (g_present_hwnd) { DestroyWindow(g_present_hwnd); g_present_hwnd = nullptr; }
    if (g_fg_mod) FreeLibrary(g_fg_mod);
    if (g_core_mod) FreeLibrary(g_core_mod);
    g_fg_mod = g_core_mod = nullptr;
    g_core_init_ext = nullptr; g_core_init_project = nullptr; g_alloc_params = nullptr;
    g_core_create = nullptr; g_core_eval = nullptr; g_core_release = nullptr; g_core_shutdown = nullptr;
    g_fg_init = nullptr; g_fg_create = nullptr; g_fg_eval = nullptr; g_fg_release = nullptr;
    g_initialized = false;
    g_present_queue_ready = false;
}

extern "C" {

__declspec(dllexport) const char* __cdecl dlss5g_version() {
    return "0.4.0-evaluate-present-queue";
}

__declspec(dllexport) const char* __cdecl dlss5g_gpu_name() {
    return g_gpu_name.c_str();
}

__declspec(dllexport) int __cdecl dlss5g_present_queue_ready() {
    return g_present_queue_ready ? 1 : 0;
}

__declspec(dllexport) int __cdecl dlss5g_init(int gpu_index, const wchar_t* runtime_dir, char* err, int err_cap) {
    std::lock_guard<std::mutex> lock(g_mutex);
    if (g_initialized) return 1;
    g_last_error.clear();
    if (!runtime_dir || !runtime_dir[0]) {
        SetError("runtime_dir required");
        CopyError(err, err_cap);
        return 0;
    }
    g_runtime_dir = runtime_dir;
    g_gpu_index = gpu_index;
    if (!SetupD3D12()) { CopyError(err, err_cap); ShutdownUnlocked(); return 0; }
    if (!LoadNGX()) { CopyError(err, err_cap); ShutdownUnlocked(); return 0; }
    if (!InitNGXSession()) { CopyError(err, err_cap); ShutdownUnlocked(); return 0; }
    g_initialized = true;
    return 1;
}

__declspec(dllexport) void __cdecl dlss5g_shutdown() {
    std::lock_guard<std::mutex> lock(g_mutex);
    ShutdownUnlocked();
}

// Generate `generated_count` interpolated RGB frames (1..5) between prev and curr.
// out_rgb layout: frame-major, each frame tightly packed RGB float [w*h*3].
__declspec(dllexport) int __cdecl dlss5g_process(
    const float* prev_rgb,
    const float* curr_rgb,
    float* out_rgb,
    int width,
    int height,
    int generated_count,
    int reset,
    char* err,
    int err_cap) {
    std::lock_guard<std::mutex> lock(g_mutex);
    g_last_error.clear();
    if (!g_initialized || !g_params) {
        SetError("dlss5g not initialized");
        CopyError(err, err_cap);
        return 0;
    }
    if (!prev_rgb || !curr_rgb || !out_rgb || width <= 0 || height <= 0) {
        SetError("invalid buffers/size");
        CopyError(err, err_cap);
        return 0;
    }
    unsigned gen = static_cast<unsigned>(std::clamp(generated_count, 1, static_cast<int>(MAX_GENERATED)));
    if (!EnsureFeature(static_cast<UINT>(width), static_cast<UINT>(height), gen)) {
        CopyError(err, err_cap);
        return 0;
    }

    // Optical flow on current frame (estimates MVs vs previous).
    NvofFlowFrame flow{};
    std::string nvof_err;
    bool reset_b = reset != 0;
    if (!NvofPrepareFrame(g_adapter.Get(), curr_rgb, static_cast<uint32_t>(width), static_cast<uint32_t>(height),
                          reset_b, flow, nvof_err)) {
        // Soft: continue with zero MVs.
        flow.has_flow = false;
    }

    // Upload current color + flat depth + MVs (prev primes NVOF; Evaluate uses current backbuffer).
    (void)prev_rgb;
    if (!UploadRgbToColor(curr_rgb, false)) { CopyError(err, err_cap); return 0; }
    if (!UploadFlatDepth(false)) { CopyError(err, err_cap); return 0; }
    if (!UploadMotionVectors(flow.has_flow ? &flow : nullptr, false)) { CopyError(err, err_cap); return 0; }
    if (!ExecuteWorkAndWait()) { CopyError(err, err_cap); return 0; }

    EvaluateFeatureFn eval_fn = g_fg_eval ? g_fg_eval : g_core_eval;
    const size_t frame_floats = static_cast<size_t>(width) * static_cast<size_t>(height) * 3;
    for (unsigned i = 1; i <= gen; ++i) {
        SetEvalParams(gen, i, (reset_b && i == 1) ? 1u : 0u);
        NGXResult r = eval_fn(g_work_cmd.Get(), g_feature, g_params, nullptr);
        if (!ExecuteWorkAndWait()) { CopyError(err, err_cap); return 0; }
        if (r != NGX_SUCCESS) {
            SetError("EvaluateFeature DLSS-G index %u/%u failed: 0x%08X", i, gen, static_cast<unsigned>(r));
            CopyError(err, err_cap);
            return 0;
        }
        if (!ReadbackInterp(out_rgb + (static_cast<size_t>(i - 1) * frame_floats))) {
            CopyError(err, err_cap);
            return 0;
        }
        if (!PresentInterpolatedOnDxgi()) {
            // Soft-fail Present after successful evaluate — still return frames to host.
            g_last_error.clear();
        }
    }
    return static_cast<int>(gen);
}

} // extern "C"
