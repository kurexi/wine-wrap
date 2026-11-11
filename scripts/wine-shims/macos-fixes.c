/* Fixes for macOS behaviour that hurts games under Wine, loaded into every
 * Wine process with DYLD_INSERT_LIBRARIES (see wine_env in launch.rs).
 *
 * 1. Metal's on-disk compiled-shader cache is limited to 1 GiB.
 *
 *    Metal keeps the GPU code it compiles in an LRU file cache (libCoreFSCache,
 *    "functions.data" under the shader cache directory) and clamps the cache
 *    to 1 GiB: MTL_SHADER_CACHE_SIZE can only lower it. Entries used during
 *    the current run are never evicted, so once a game has used 1 GiB of
 *    shaders, every further one is dropped instead of stored and has to be
 *    compiled again on the next launch. Phantom Blade Zero needs about
 *    1.8 GiB and so spent 8 minutes compiling shaders on every start.
 *
 *    The cache library itself takes a 64-bit size, so hand it a larger one.
 *    WINEWRAP_SHADER_CACHE_MB sets the limit in MiB (default 16384); 1024 or
 *    less leaves Metal's own limit alone.
 *
 * 2. Freed malloc memory is never returned to the system under Rosetta
 *    (optional).
 *
 *    The system allocator gives free pages back with
 *    madvise(MADV_FREE_REUSABLE). In a translated process (4K pages on a 16K
 *    kernel) that call succeeds but frees nothing, so every transient
 *    allocation stays in the process's footprint for good: Phantom Blade Zero
 *    carried 3.7 GB of free heap this way. Mapping fresh anonymous memory
 *    over the range does release the pages, and leaves them zero-filled,
 *    which is what the allocator is prepared for.
 *
 *    It made no difference to Phantom Blade Zero, whose memory is in use
 *    rather than free, so it stays off unless WINEWRAP_MADVISE_FIX=1.
 *
 * 3. Games budget their graphics memory from a figure that is too generous.
 *
 *    D3DMetal reports Metal's recommendedMaxWorkingSetSize as the adapter's
 *    video memory: 12 GB on a 16 GB Mac, although that memory is shared with
 *    the game itself, D3DMetal's shader caches and the rest of the system.
 *    WINEWRAP_VRAM_MB makes Metal report that many MiB instead, so a game
 *    sizes its texture pools for it. Unset, nothing changes.
 */
#include <stdint.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/sysctl.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <mach/vm_statistics.h>
#include <objc/runtime.h>
#include <objc/message.h>

#define INTERPOSE(replacement, original) \
    __attribute__((used, section("__DATA,__interpose"))) \
    static const struct { const void *new, *old; } interpose_##original = \
    { (const void *)replacement, (const void *)original }

/* ---- 1. shader cache size ---- */

/* First 16 bytes of the options Metal passes; the rest is not touched. */
struct fscache_options
{
    uint64_t size;
    uint16_t version, key_length, flags, reserved;
};

extern int fscache_open_with_key(const char *path, struct fscache_options *options,
                                 void **cache, const void *key);

#define METAL_CACHE_LIMIT 0x40000000ull

static int winewrap_fscache_open_with_key(const char *path, struct fscache_options *options,
                                          void **cache, const void *key)
{
    /* Only caches left at Metal's ceiling; a smaller one was asked for. */
    if (options && options->size == METAL_CACHE_LIMIT)
    {
        const char *env = getenv("WINEWRAP_SHADER_CACHE_MB");
        uint64_t mb = env ? strtoull(env, NULL, 10) : 16384;

        if (mb > 1024)
        {
            struct fscache_options larger = *options;
            larger.size = mb << 20;
            return fscache_open_with_key(path, &larger, cache, key);
        }
    }
    return fscache_open_with_key(path, options, cache, key);
}
INTERPOSE(winewrap_fscache_open_with_key, fscache_open_with_key);

/* ---- 2. returning freed malloc memory ---- */

static int fix_madvise(void)
{
    static int enabled = -1;

    if (enabled < 0)
    {
        int translated = 0;
        size_t size = sizeof(translated);

        if (sysctlbyname("sysctl.proc_translated", &translated, &size, NULL, 0)) translated = 0;
        enabled = translated && getenv("WINEWRAP_MADVISE_FIX");
    }
    return enabled;
}

/* Release the whole kernel pages inside [addr, addr + len), as long as they
 * are ordinary read-write malloc memory. Returns 0 if any part was left. */
static int release_malloc_pages(void *addr, size_t len)
{
    const uintptr_t mask = vm_kernel_page_size - 1;
    uintptr_t start = ((uintptr_t)addr + mask) & ~mask, end = ((uintptr_t)addr + len) & ~mask;

    while (start < end)
    {
        mach_vm_address_t region = start;
        mach_vm_size_t size = 0;
        natural_t depth = 16;
        vm_region_submap_info_data_64_t info;
        mach_msg_type_number_t count = VM_REGION_SUBMAP_INFO_COUNT_64;
        uintptr_t stop;

        if (mach_vm_region_recurse(mach_task_self(), &region, &size, &depth,
                                   (vm_region_recurse_info_t)&info, &count) || region > start)
            return 0;
        /* share_mode is no help: a region split by an earlier call reports as shared */
        if (info.is_submap || info.external_pager || info.user_wired_count
            || info.protection != (VM_PROT_READ | VM_PROT_WRITE)
            || info.user_tag < VM_MEMORY_MALLOC || info.user_tag > VM_MEMORY_MALLOC_MEDIUM)
            return 0;
        stop = region + size < end ? region + size : end;
        if (mmap((void *)start, stop - start, PROT_READ | PROT_WRITE,
                 MAP_ANON | MAP_PRIVATE | MAP_FIXED, VM_MAKE_TAG(info.user_tag), 0) == MAP_FAILED)
            return 0;
        start = stop;
    }
    return 1;
}

static int winewrap_madvise(void *addr, size_t len, int advice)
{
    if (advice == MADV_FREE_REUSABLE && fix_madvise() && release_malloc_pages(addr, len)) return 0;
    return madvise(addr, len, advice);
}
INTERPOSE(winewrap_madvise, madvise);

/* ---- 3. reported video memory ---- */

extern id MTLCreateSystemDefaultDevice(void);
extern id MTLCopyAllDevices(void);

static uint64_t vram_bytes;

static uint64_t winewrap_recommended_working_set(id self, SEL sel)
{
    return vram_bytes;
}

static void limit_device_memory(id device)
{
    static Class done;
    const char *env;
    Class class;

    if (!device || !(env = getenv("WINEWRAP_VRAM_MB"))) return;
    if (!(vram_bytes = strtoull(env, NULL, 10) << 20)) return;
    class = object_getClass(device);
    if (class == done) return;
    done = class;
    /* add or replace on the device's own class, leaving its superclasses alone */
    class_replaceMethod(class, sel_registerName("recommendedMaxWorkingSetSize"),
                        (IMP)winewrap_recommended_working_set, "Q@:");
}

static id winewrap_MTLCreateSystemDefaultDevice(void)
{
    id device = MTLCreateSystemDefaultDevice();
    limit_device_memory(device);
    return device;
}
INTERPOSE(winewrap_MTLCreateSystemDefaultDevice, MTLCreateSystemDefaultDevice);

static id winewrap_MTLCopyAllDevices(void)
{
    id devices = MTLCopyAllDevices();
    unsigned long count = devices ? ((unsigned long (*)(id, SEL))objc_msgSend)(devices, sel_registerName("count")) : 0;

    for (unsigned long i = 0; i < count; i++)
        limit_device_memory(((id (*)(id, SEL, unsigned long))objc_msgSend)(devices, sel_registerName("objectAtIndex:"), i));
    return devices;
}
INTERPOSE(winewrap_MTLCopyAllDevices, MTLCopyAllDevices);
