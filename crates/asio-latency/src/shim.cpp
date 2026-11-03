// SoundCraft's ASIOGetLatencies shim (see build.rs). asio-sys's bindings leave ASIOGetLatencies
// out, but the SDK library it builds and links (asio.lib) has it; this C wrapper gives Rust a
// stable, unmangled name for it. It defines no ASIO state of its own.
#include "asiosys.h"
#include "asio.h"

extern "C" long sc_asio_get_latencies(long* input, long* output) {
    if (!input || !output) {
        return ASE_InvalidParameter;
    }
    *input = 0;
    *output = 0;
    // A C++ exception must not unwind into Rust.
    try {
        return ASIOGetLatencies(input, output);
    } catch (...) {
        *input = 0;
        *output = 0;
        return ASE_HWMalfunction;
    }
}
