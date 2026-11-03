// Scratch harness: ShaleDSP.h (read-only, included from the FireStation tree) driven with a
// sidechain and Kit Link events, for comparison with the Rust port. Not part of any repo.
#include "ShaleDSP.h"
#include <cstdio>
#include <vector>
#include <string>
#include <cstdlib>

static std::vector<float> readRaw(const char* p, size_t n) {
    std::vector<float> v(n);
    FILE* f = std::fopen(p, "rb"); if (!f) { std::printf("no %s\n", p); std::exit(1); }
    std::fread(v.data(), sizeof(float), n, f); std::fclose(f); return v;
}

int main(int argc, char** argv) {
    // args: scenario n in.raw out.raw
    const int scenario = std::atoi(argv[1]);
    const size_t n = (size_t) std::atoll(argv[2]);
    auto in = readRaw(argv[3], 4 * n);   // mainL mainR scL scR
    std::vector<float> out(2 * n);
    static ShaleDSP dsp;
    dsp.prepare(48000.0, 512);
    // the processor's forced push: every value, in parameter order
    std::vector<float> v(ShaleDSP::numParams);
    const float defs[] = {0,3,0.55f,0.5f,0,1,0.4f,0,2,0.35f,0.2f,0,0,0.5f,0.5f,1,0.4f,0.6f,0,0,0,1,0.3333f,0.5f,0,0,0,0.5f,0,0.1667f,0,0,0.5f,0,0};
    for (int i = 0; i < ShaleDSP::numParams; ++i) v[i] = defs[i];
    if (scenario == 0) { v[ShaleDSP::pScSource] = 1; v[ShaleDSP::pThreshold] = 0.5f; v[ShaleDSP::pDebleed] = 0.5f; }
    if (scenario == 1) { v[ShaleDSP::pLink] = 1; v[ShaleDSP::pLinkRole] = 2; v[ShaleDSP::pReject] = 0.8f; v[ShaleDSP::pLookahead] = 4; v[ShaleDSP::pReduction] = 0.6f; }
    if (scenario == 2) { v[ShaleDSP::pLink] = 1; v[ShaleDSP::pLinkRole] = 1; v[ShaleDSP::pInstrument] = 10; v[ShaleDSP::pReduction] = 0.5f; }
    if (scenario == 3) { v[ShaleDSP::pLink] = 1; v[ShaleDSP::pReject] = 1.0f; v[ShaleDSP::pDebleed] = 0.6f; v[ShaleDSP::pDebleedMode] = 1; v[ShaleDSP::pReduction] = 0.7f; }
    for (int i = 0; i < ShaleDSP::numParams; ++i) dsp.setParameter(i, v[i]);
    float bufL[512], bufR[512], scL[512], scR[512];
    long trig = 0;
    for (size_t pos = 0; pos < n; pos += 512) {
        const int len = (int) std::min<size_t>(512, n - pos);
        for (int i = 0; i < len; ++i) { bufL[i] = in[pos + i]; bufR[i] = in[n + pos + i]; scL[i] = in[2*n + pos + i]; scR[i] = in[3*n + pos + i]; }
        // link events: partners every 7000 samples, neighbours (0 dB) every 11000, at their offset
        for (int i = 0; i < len; ++i) {
            const size_t t = pos + i;
            if (scenario >= 1 && t % 7000 == 100) dsp.addPartnerHit(0.6f, i);
            if (scenario >= 1 && t % 11000 == 50) dsp.addNeighbourBeacon(0.0f, i);
        }
        float* ch[2] = { bufL, bufR };
        const float* sc[2] = { scL, scR };
        dsp.process(ch, 2, len, sc, 2);
        trig += dsp.getNumTriggers();
        dsp.clearTriggers();
        for (int i = 0; i < len; ++i) { out[pos + i] = bufL[i]; out[n + pos + i] = bufR[i]; }
    }
    FILE* f = std::fopen(argv[4], "wb"); std::fwrite(out.data(), sizeof(float), out.size(), f); std::fclose(f);
    std::printf("scenario %d: %ld triggers, %d deferred\n", scenario, trig, dsp.deferSeq.load());
    return 0;
}
