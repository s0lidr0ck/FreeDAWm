// Scout C++ reference harness: writes reference.json for `soundcraft_dsp::scout` tests.
//
// It includes Scout2's ScoutDSP.h read-only and runs three generated 48 kHz signals through
// prepare -> setDurationIndex -> startCapture -> process (512-sample blocks) until the capture
// finishes, then prints every Metrics field. The signals use a fixed LCG and only operations the
// Rust test reproduces exactly (the same generator lives in `tests.rs`).
//
// Built in a scratch directory, never inside the FireStation tree, twice, with the processor's
// ScopedNoDenormals (FTZ/DAZ) in both:
//   /fp:precise, the strict check of the translation (IEEE order of operations, as Rust):
//     cl /O2 /fp:precise /EHsc /std:c++17 /I C:\projects\A18\FireStation\Scout2\Source reference_harness.cpp
//   /fp:fast, the plugin's own release flags (Scout2 CMakeLists), i.e. what the owner's Scout2 reports:
//     cl /O2 /fp:fast /EHsc /std:c++17 /I C:\projects\A18\FireStation\Scout2\Source reference_harness.cpp
// reference.json is {"fp_precise": <precise build's output>, "fp_fast": <fast build's output>}
// (spectrum31 arrays written one value per line).
// Regenerated 2026-10-21 against Scout2 with the 6th-order Butterworth "v2 bands".

#include "ScoutDSP.h"
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <vector>
#include <xmmintrin.h>
#include <pmmintrin.h>

static constexpr double kPi = 3.14159265358979323846;
static constexpr int kRate = 48000;

struct Lcg
{
    uint32_t s;
    explicit Lcg (uint32_t seed) : s (seed) {}
    // Uniform in [-1, 1): exact float ops only.
    float next()
    {
        s = s * 1664525u + 1013904223u;
        return (float) (s >> 8) * (1.0f / 16777216.0f) * 2.0f - 1.0f;
    }
};

struct Pink   // Paul Kellet's economy pink filter, in float
{
    float b0 = 0, b1 = 0, b2 = 0;
    float step (float w)
    {
        b0 = 0.99765f * b0 + w * 0.0990460f;
        b1 = 0.96300f * b1 + w * 0.2965164f;
        b2 = 0.57000f * b2 + w * 1.0526913f;
        return b0 + b1 + b2 + w * 0.1848f;
    }
};

// (a) 1 kHz sine at -20 dBFS peak, 5 s, L = R.
static void sine1k (std::vector<float>& l, std::vector<float>& r)
{
    const int n = 5 * kRate;
    l.resize ((size_t) n); r.resize ((size_t) n);
    for (int i = 0; i < n; ++i)
    {
        const float x = (float) (0.1 * std::sin (2.0 * kPi * 1000.0 * (double) i / (double) kRate));
        l[(size_t) i] = x; r[(size_t) i] = x;
    }
}

// (b) stereo pink-ish noise, L/R decorrelated (independent generators), -18 dBFS RMS, 10 s.
static void pinkStereo (std::vector<float>& l, std::vector<float>& r)
{
    const int n = 10 * kRate;
    l.resize ((size_t) n); r.resize ((size_t) n);
    Lcg gl (0x12345678u), gr (0x9E3779B9u);
    Pink pl, pr;
    double ss = 0.0;
    for (int i = 0; i < n; ++i)
    {
        l[(size_t) i] = pl.step (gl.next());
        r[(size_t) i] = pr.step (gr.next());
        ss += (double) l[(size_t) i] * (double) l[(size_t) i] + (double) r[(size_t) i] * (double) r[(size_t) i];
    }
    const double target = 0.12589254117941673;   // 10^(-18/20)
    const float g = (float) (target / std::sqrt (ss / (2.0 * (double) n)));
    for (int i = 0; i < n; ++i) { l[(size_t) i] *= g; r[(size_t) i] *= g; }
}

// (c) drum-like: a 50 ms decaying 120 Hz burst every 500 ms over -60 dBFS noise, 8 s.
static void drumBursts (std::vector<float>& l, std::vector<float>& r)
{
    const int n = 8 * kRate;
    l.resize ((size_t) n); r.resize ((size_t) n);
    Lcg gl (0x00C0FFEEu), gr (0x0BADC0DEu);
    for (int i = 0; i < n; ++i)
    {
        const int pos = i % (kRate / 2);
        float b = 0.0f;
        if (pos < kRate / 20)
            b = (float) (0.5 * std::exp (-(double) pos / 480.0) * std::sin (2.0 * kPi * 120.0 * (double) pos / (double) kRate));
        l[(size_t) i] = b + 0.001f * gl.next();
        r[(size_t) i] = b + 0.001f * gr.next();
    }
}

static void printMetrics (const char* name, const ScoutDSP::Metrics& m, bool last)
{
    std::printf ("  \"%s\": {\n", name);
    std::printf ("    \"valid\": %s,\n", m.valid ? "true" : "false");
#define F(key, v) std::printf ("    \"" key "\": %.9g,\n", (double) (v))
    F ("duration_sec", m.durationSec);
    F ("sample_rate", m.sampleRate);
    F ("activity_ratio", m.activityRatio);
    std::printf ("    \"clipped_samples\": %d,\n", m.clippedSamples);
    F ("peak_hold_dbfs", m.peakHoldDbfs);
    F ("noise_floor_dbfs", m.noiseFloorDbfs);
    F ("peak_dbfs", m.peakDbfs);
    F ("true_peak_dbtp", m.truePeakDbtp);
    F ("rms_dbfs", m.rmsDbfs);
    F ("lufs_integrated", m.lufsIntegrated);
    F ("lra", m.lra);
    F ("crest_factor_db", m.crestFactorDb);
    F ("dynamic_range_db", m.dynamicRangeDb);
    F ("peak_to_lufs_db", m.peakToLufsDb);
    F ("transient_density", m.transientDensity);
    std::printf ("    \"spectrum31\": [");
    for (int i = 0; i < ScoutDSP::numBands; ++i)
        std::printf ("%s%.9g", i ? ", " : "", (double) m.spectrum31[(size_t) i]);
    std::printf ("],\n");
    F ("zone_sub", m.zoneSub);
    F ("zone_low", m.zoneLow);
    F ("zone_low_mid", m.zoneLowMid);
    F ("zone_mid", m.zoneMid);
    F ("zone_high_mid", m.zoneHighMid);
    F ("zone_high", m.zoneHigh);
    F ("zone_air", m.zoneAir);
    F ("stereo_correlation", m.stereoCorrelation);
    F ("stereo_width", m.stereoWidth);
    F ("low_freq_correlation", m.lowFreqCorrelation);
    F ("lr_balance_db", m.lrBalanceDb);
    F ("dc_offset", m.dcOffset);
    F ("spectral_centroid_hz", m.spectralCentroidHz);
    F ("mid_side_ratio_db", m.midSideRatioDb);
    F ("side_low_ratio_db", m.sideLowRatioDb);
    F ("noise_floor_p10_dbfs", m.noiseFloorP10Dbfs);
    F ("lufs_p10", m.lufsP10);
    F ("lufs_p50", m.lufsP50);
    F ("lufs_p90", m.lufsP90);
    F ("lufs_momentary_max", m.lufsMomentaryMax);
    std::printf ("    \"lufs_short_term_max\": %.9g\n", (double) m.lufsShortTermMax);
#undef F
    std::printf ("  }%s\n", last ? "" : ",");
}

static bool run (const char* name, int durationIndex, void (*gen) (std::vector<float>&, std::vector<float>&), bool last)
{
    std::vector<float> l, r;
    gen (l, r);

    ScoutDSP dsp;
    dsp.prepare ((double) kRate);
    dsp.setDurationIndex (durationIndex);
    dsp.startCapture();

    const int n = (int) l.size();
    for (int at = 0; at < n; at += 512)
    {
        const int len = std::min (512, n - at);
        float* ch[2] = { l.data() + at, r.data() + at };
        dsp.process (ch, 2, len);
    }
    if (! dsp.hasResult()) { std::fprintf (stderr, "%s: capture did not finish\n", name); return false; }
    printMetrics (name, dsp.takeResult(), last);
    return true;
}

int main()
{
    // The plugin's processBlock runs under juce::ScopedNoDenormals.
    _MM_SET_FLUSH_ZERO_MODE (_MM_FLUSH_ZERO_ON);
    _MM_SET_DENORMALS_ZERO_MODE (_MM_DENORMALS_ZERO_ON);

    std::printf ("{\n");
    bool ok = run ("sine_1k", 1, sine1k, false);          // 5 s
    ok = run ("pink_stereo", 3, pinkStereo, false) && ok; // 10 s
    ok = run ("drum_bursts", 2, drumBursts, true) && ok;  // 8 s
    std::printf ("}\n");
    return ok ? 0 : 1;
}
