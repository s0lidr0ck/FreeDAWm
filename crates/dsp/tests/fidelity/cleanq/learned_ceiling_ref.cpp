// The reference for `learned_ceiling.ref.txt`: a learned CleanQ ceiling from a fixed input,
// through the C++ CleanQCeiling.h maths (the averager, smoothToGrid, autoCeiling, conform -- the
// fixed-bell Conform of 2026-10-23, three ways, with the make-up's peak cap), and
// CleanQDynCeiling.h cutting that input against it,
// checked by `clean_q_ceiling::tests::a_learned_ceiling_matches_the_cpp_maths`. Built outside the
// C++ tree (the sources are only included), as the CleanQ Tests/ build.bat builds ceilingtest:
//   cl /O2 /EHsc /std:c++17 /fp:precise /I <CleanQ>\Source learned_ceiling_ref.cpp
//   learned_ceiling_ref.exe > learned_ceiling.ref.txt
#include "CleanQCeiling.h"
#include "CleanQDynCeiling.h"
#include <complex>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <algorithm>
using namespace fs::cq;
using Q = CleanQDSP;
namespace cl = fs::cq::ceiling;

static void fft (std::vector<std::complex<double>>& a)
{
    const size_t n = a.size();
    for (size_t i = 1, j = 0; i < n; ++i)
    {
        size_t bit = n >> 1;
        for (; j & bit; bit >>= 1) j ^= bit;
        j ^= bit;
        if (i < j) std::swap (a[i], a[j]);
    }
    for (size_t len = 2; len <= n; len <<= 1)
    {
        const double ang = -2.0 * Q::pi / (double) len;
        const double wr = std::cos (ang), wi = std::sin (ang);
        for (size_t i = 0; i < n; i += len)
        {
            double cr = 1.0, ci = 0.0;
            for (size_t k = 0; k < len / 2; ++k)
            {
                const auto u = a[i + k];
                const auto x = a[i + k + len / 2];
                const double vr = x.real() * cr - x.imag() * ci, vi = x.real() * ci + x.imag() * cr;
                a[i + k] = { u.real() + vr, u.imag() + vi };
                a[i + k + len / 2] = { u.real() - vr, u.imag() - vi };
                const double nr = cr * wr - ci * wi, ni = cr * wi + ci * wr;
                cr = nr; ci = ni;
            }
        }
    }
}

int main()
{
    const int total = 48000 * 2;
    std::vector<float> sig ((size_t) total);
    uint32_t seed = 12345u;
    float lp = 0.0f, lp2 = 0.0f, y1 = 0.0f, y2 = 0.0f;
    for (int n = 0; n < total; ++n)
    {
        seed = seed * 1664525u + 1013904223u;
        const float w = (float) (seed >> 8) / (float) (1u << 23) - 1.0f;
        lp += 0.05f * (w - lp);
        lp2 += 0.3f * (w - lp2);
        const float y = 1.9645745f * y1 - 0.9801f * y2 + 0.02f * w;   // a resonance near 1.2 kHz
        y2 = y1; y1 = y;
        sig[(size_t) n] = 0.6f * lp + 0.1f * w + 0.2f * lp2 + 0.5f * y;
    }

    std::vector<float> window ((size_t) Q::fftSize);
    Q::fillWindow (window.data());
    cl::SpectrumAverager avg;
    std::vector<float> mag ((size_t) Q::numBins);
    const int hop = Q::fftSize / 2;
    int frames = 0;
    for (int start = 0; start + Q::fftSize <= total; start += hop)
    {
        std::vector<std::complex<double>> x ((size_t) Q::fftSize);
        for (int k = 0; k < Q::fftSize; ++k) x[(size_t) k] = (double) (sig[(size_t) (start + k)] * window[(size_t) k]);
        fft (x);
        for (int k = 0; k < Q::numBins; ++k)
        {
            const double re = x[(size_t) k].real(), im = x[(size_t) k].imag();
            mag[(size_t) k] = (float) (std::sqrt (re * re + im * im) * Q::fftMagnitudeScale);
        }
        avg.add (mag.data(), Q::numBins, 1.0 / 30.0, 0.0);
        ++frames;
    }
    const auto grid = cl::smoothToGrid (avg.bins(), 48000.0, Q::fftSize, 3.0);
    const auto c = cl::autoCeiling (grid);
    std::printf ("frames %d\n", frames);
    for (double v : grid) std::printf ("grid %.17g\n", v);
    for (const auto& p : c.pts) std::printf ("node %.17g %.17g\n", p.hz, p.db);
    for (int i = 0; i < cl::gridN; ++i) std::printf ("curve %.17g\n", c.at (cl::gridHz (i)));
    // Conform of the capture onto the learned ceiling with one node pulled down 6 dB.
    auto d = c;
    cl::moveNode (d, 9, d.pts[9].db - 6.0, false);
    cl::ConformSettings cs;
    const auto r = cl::conform (grid, d, cs);
    for (double v : r.target) std::printf ("target %.17g\n", v);
    for (double v : r.weight) std::printf ("weight %.17g\n", v);
    for (const auto& b : r.bands) std::printf ("band %d %.17g %.17g %.17g\n", b.type, b.hz, b.db, b.q);
    std::printf ("rms %.17g %.17g %.17g\n", r.rmsBefore, r.rmsAfter, r.worstAfter);
    std::printf ("conform %.17g %.17g %.17g %.17g %.17g\n", r.askedLo, r.askedHi, r.curveLo, r.curveHi, r.makeupDb);
    // And onto a pink line through the capture (far from it: the range clamps), 70 %, a budget
    // of 8 bells, +/-6 dB; then the same cuts only.
    cl::Ceiling pink = cl::flatCeiling (-30.0);
    const double lvl = cl::fitNoiseLevel (grid, -3.0, 40.0, 12000.0);
    for (auto& p : pink.pts) p.db = std::clamp (cl::noiseLineDb (-3.0, lvl, p.hz), -96.0, 0.0);
    for (bool cuts : { false, true })
    {
        cl::ConformSettings ps;
        ps.amount = 0.7; ps.maxBands = 8; ps.maxDb = 6.0; ps.cutsOnly = cuts;
        ps.inputPeakDb = cuts ? -2.5 : -40.0;   // hot (the make-up is held) / quiet (it is not)
        const auto rp = cl::conform (grid, pink, ps);
        const char* tag = cuts ? "pcut" : "pink";
        for (double v : rp.target) std::printf ("%s_target %.17g\n", tag, v);
        for (const auto& b : rp.bands) std::printf ("%s_band %d %.17g %.17g %.17g\n", tag, b.type, b.hz, b.db, b.q);
        std::printf ("%s_conform %.17g %.17g %.17g %.17g %.17g %.17g %.17g %.17g %.17g %d\n", tag, rp.rmsBefore, rp.rmsAfter, rp.worstAfter,
                     rp.askedLo, rp.askedHi, rp.curveLo, rp.curveHi, rp.makeupDb, rp.makeupWantedDb, rp.makeupCapped ? 1 : 0);
    }

    // The dynamic ceiling cutting the same input (stereo: the right channel a half-second later)
    // against the learned ceiling 6 dB lower, in 512-sample blocks, with the default settings
    // switched on: every output sample's bits hashed (FNV-1a), every 101st printed.
    DynCeiling dyn;
    dyn.prepare (48000.0);
    std::vector<double> lowered ((size_t) cl::gridN);
    for (int i = 0; i < cl::gridN; ++i) lowered[(size_t) i] = c.at (cl::gridHz (i)) - 6.0;
    dyn.setCeiling (lowered);
    DynCeiling::Settings ds;
    ds.on = true;
    dyn.setSettings (ds);
    std::vector<float> L (sig), R ((size_t) total);
    for (int n = 0; n < total; ++n) R[(size_t) n] = sig[(size_t) ((n + 24000) % total)];
    for (int at = 0; at < total; at += 512)
        dyn.process (L.data() + at, R.data() + at, std::min (512, total - at));
    uint64_t h = 1469598103934665603ull;
    for (int n = 0; n < total; ++n)
        for (float v : { L[(size_t) n], R[(size_t) n] })
        {
            uint32_t b; std::memcpy (&b, &v, 4);
            for (int k = 0; k < 4; ++k) { h ^= (b >> (8 * k)) & 0xFFu; h *= 1099511628211ull; }
        }
    std::printf ("hash %llu\n", (unsigned long long) h);
    for (int n = 0; n < total; n += 101) std::printf ("out %d %.9g %.9g\n", n, L[(size_t) n], R[(size_t) n]);
    for (int i = 0; i < cl::gridN; ++i) std::printf ("cut %.9g\n", dyn.cutAt (i));
    return 0;
}
