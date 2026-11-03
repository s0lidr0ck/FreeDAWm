// band_ref: MIDI Studio's own band (an offline MidiStudioProcessor, built from the unmodified MS sources)
// played through one JSON scenario, written as event lines for the Rust port to match. See README.md.
//
//   BandRef --scenario <file.json> --out <file.events> [--library <dir>]
//   BandRef --dump-timeline <factory song name> [--out <file.json>] [--library <dir>]
//
// --library: the Library folder the processor reads (MIDISTUDIO_LIBRARY is pointed at it before the
// processor exists). Default: a temporary copy of <repo>/crates/band/library, deleted on exit. Nothing is
// read from or written to the MIDI Studio tree.
#include <juce_audio_processors/juce_audio_processors.h>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <algorithm>
#include <process.h>
#include <string>
#include <vector>
#include "PluginProcessor.h"
#include "Song.h"
#include "SongDoc.h"
#include "SongFile.h"
#include "SongLibrary.h"

#ifndef BAND_REPO_DIR
 #error "BAND_REPO_DIR must name the DAW repository (CMakeLists.txt sets it)"
#endif

using namespace fs::midistudio;

namespace
{
int fail (const juce::String& why)
{
    std::fprintf (stderr, "band_ref: %s\n", why.toRawUTF8());
    return 1;
}

/** FNV-1a, 64-bit, as xtask's `fidelity::fnv1a64` (the `# case` line ties a ref to its scenario's bytes). */
std::string fnv1a64Hex (const juce::MemoryBlock& bytes)
{
    uint64_t h = 0xcbf29ce484222325ull;
    for (size_t i = 0; i < bytes.getSize(); ++i) { h ^= (uint64_t) (uint8_t) bytes[i]; h *= 0x00000100000001b3ull; }
    char s[17];
    std::snprintf (s, sizeof (s), "%016llx", (unsigned long long) h);
    return s;
}

/** Harness.h's play head, unchanged: position, tempo, meter and a stop switch. */
struct Head : juce::AudioPlayHead
{
    double ppq = 0, bpm = 120; bool playing = true; int num = 4, den = 4;
    juce::Optional<PositionInfo> getPosition() const override
    { PositionInfo i; i.setIsPlaying (playing); i.setBpm (bpm); i.setPpqPosition (ppq); i.setTimeSignature (juce::AudioPlayHead::TimeSignature { num, den }); return i; }
};

bool isNumber (const juce::var& v) { return v.isInt() || v.isInt64() || v.isDouble(); }
/** A plain folder name (letters, digits, '-', '_'): never a path out of tests/fixtures/libraries. */
bool isOverlayName (const juce::var& v)
{
    if (! v.isString() || v.toString().isEmpty()) return false;
    for (auto c : v.toString())
        if (! (juce::CharacterFunctions::isLetterOrDigit (c) && c < 128) && c != '-' && c != '_') return false;
    return true;
}

/** A temporary folder name no other BandRef run uses: the process id, a per-process counter and the time. */
juce::File uniqueTempDir (const juce::String& prefix)
{
    static int counter = 0;
    return juce::File::getSpecialLocation (juce::File::tempDirectory)
        .getChildFile (prefix + juce::String (_getpid()) + "-" + juce::String (++counter) + "-" + juce::String (juce::Time::currentTimeMillis()));
}

/** A scenario's overlay folder: <scenarios>/../fixtures/libraries/<name>. */
juce::File overlayDir (const juce::File& scenarioFile, const juce::String& name)
{
    return scenarioFile.getParentDirectory().getSiblingFile ("fixtures").getChildFile ("libraries").getChildFile (name);
}

/** What the `# case` line hashes: the scenario file's bytes, then for each Players/*.json / Targets/*.json of its
    overlay (sorted by "<Players|Targets>/<file name>", byte order) a newline, that path, a newline and the file's
    bytes (xtask band_refs::case_bytes, the Rust runner's common::case_bytes). An edited overlay file makes the ref stale. */
juce::MemoryBlock caseBytes (const juce::File& scenarioFile, const juce::MemoryBlock& bytes, const juce::String& overlay)
{
    juce::MemoryBlock out (bytes);
    if (overlay.isEmpty()) return out;
    const auto dir = overlayDir (scenarioFile, overlay);
    std::vector<std::pair<std::string, juce::File>> files;
    for (auto sub : { "Players", "Targets" })
        for (auto& f : dir.getChildFile (sub).findChildFiles (juce::File::findFiles, false, "*.json"))
            files.push_back ({ std::string (sub) + "/" + f.getFileName().toStdString(), f });
    std::sort (files.begin(), files.end(), [] (const auto& a, const auto& b)
    {
        return std::lexicographical_compare (a.first.begin(), a.first.end(), b.first.begin(), b.first.end(),
                                             [] (char x, char y) { return (unsigned char) x < (unsigned char) y; });
    });
    for (auto& [rel, f] : files)
    {
        juce::MemoryBlock data;
        f.loadFileAsData (data);
        out.append ("\n", 1);
        out.append (rel.data(), rel.size());
        out.append ("\n", 1);
        out.append (data.getData(), data.getSize());
    }
    return out;
}

/** A scenario's `library_overlay`: the Library copied to a temporary folder with the overlay's Players / Targets
    files copied over it (a file of the same name replaced, a new one added), as the Rust runner's
    LibrarySource::with_user_folder reads them. Empty `name`: `lib` unchanged. */
bool overlayLibrary (const juce::File& scenarioFile, const juce::String& name, juce::File& lib, juce::File& tempCopy, juce::String& error)
{
    if (name.isEmpty()) return true;
    const auto dir = overlayDir (scenarioFile, name);
    if (! dir.getChildFile ("Players").isDirectory() && ! dir.getChildFile ("Targets").isDirectory())
    { error = "library_overlay: " + dir.getFullPathName() + " has no Players or Targets folder"; return false; }
    const auto merged = uniqueTempDir ("band-ref-overlay-");
    if (! lib.copyDirectoryTo (merged))
    { error = "could not copy " + lib.getFullPathName() + " to " + merged.getFullPathName(); merged.deleteRecursively(); return false; }
    for (auto sub : { "Players", "Targets" })
        for (auto& f : dir.getChildFile (sub).findChildFiles (juce::File::findFiles, false, "*.json"))
        {
            const auto dest = merged.getChildFile (sub).getChildFile (f.getFileName());
            if (! dest.getParentDirectory().createDirectory() || ! f.copyFileTo (dest))
            { error = "could not copy " + f.getFullPathName() + " to " + dest.getFullPathName(); merged.deleteRecursively(); return false; }
        }
    if (tempCopy != juce::File()) tempCopy.deleteRecursively();
    lib = tempCopy = merged;
    return true;
}
/** A number with no fractional part (3 or 3.0, never 3.5): meters, block sizes, block counts. */
bool isInteger (const juce::var& v) { return isNumber (v) && std::isfinite ((double) v) && std::floor ((double) v) == (double) v; }

/** A factory song file by name: <library>/Songs/Factory/<name>.song.json. */
std::optional<SongFileData> factorySong (const juce::File& library, const juce::String& name, juce::String& error)
{
    const auto f = library.getChildFile ("Songs").getChildFile ("Factory").getChildFile (juce::File::createLegalFileName (name) + ".song.json");
    if (! f.existsAsFile()) { error = "no factory song \"" + name + "\" (" + f.getFullPathName() + ")"; return std::nullopt; }
    juce::StringArray skipped;
    auto d = loadSongFile (f, skipped, error);
    if (d && ! skipped.isEmpty()) { error = f.getFullPathName() + " only reads leniently: " + skipped.joinIntoString ("; "); return std::nullopt; }
    return d;
}

// ---- --dump-timeline -----------------------------------------------------------------------------

juce::String num (double v) { char s[40]; std::snprintf (s, sizeof (s), "%.17g", v); return s; }
juce::String flag (bool b) { return b ? "true" : "false"; }

juce::String locJson (const SongLoc& l)
{
    return "{\"idx\": " + juce::String (l.idx) + ", \"loop\": " + juce::String (l.loop) + ", \"global\": " + juce::String (l.global)
         + ", \"start\": " + num (l.start) + ", \"end\": " + num (l.end) + ", \"ended\": " + flag (l.ended) + "}";
}

/** The 200 songLocate probes: 100 on a 0.75 ppq grid from -1 (exact binary values, so chord boundaries are hit
    exactly), then 100 off-grid ones (1.37 ppq apart, 0.00005 before the grid); bar length 4 on even i, 3 on odd. */
double probePpq (int i) { return i < 100 ? -1.0 + 0.75 * i : (double) (i - 100) * 1.37 - 0.00005; }
double probeBar (int i) { return i % 2 == 0 ? 4.0 : 3.0; }

int dumpTimeline (const juce::File& library, const juce::String& name, const juce::File& out)
{
    juce::String error;
    auto d = factorySong (library, name, error);
    if (! d) return fail (error);
    const auto t = buildTimeline (d->song);
    juce::String s;
    s << "{\n  \"song\": " << juce::JSON::toString (juce::var (name)) << ",\n";
    s << "  \"total_half\": " << t->totalHalf << ", \"arrangement_length\": " << t->arrangementLength
      << ", \"uniform_len_half\": " << t->uniformLenHalf << ", \"intro_half\": " << t->introHalf
      << ", \"has_ending\": " << flag (t->hasEnding) << ", \"meter_override\": " << t->meterOverride << ",\n";
    s << "  \"entries\": [\n";
    for (size_t i = 0; i < t->entries.size(); ++i)
    {
        const auto& e = t->entries[i];
        s << "    {\"start_half\": " << e.startHalf << ", \"len_half\": " << e.lenHalf << ", \"section\": " << e.section
          << ", \"chord_index\": " << e.chordIndex << ", \"section_chords\": " << e.sectionChords << ", \"arr_pos\": " << e.arrPos
          << ", \"slot\": " << e.slot << ", \"bass\": " << e.bass << ", \"energy_add\": " << e.energyAdd << ", \"take\": " << e.take << "}"
          << (i + 1 < t->entries.size() ? ",\n" : "\n");
    }
    s << "  ],\n  \"locate\": [\n";
    for (int i = 0; i < 200; ++i)
    {
        const double ppq = probePpq (i), bar = probeBar (i);
        s << "    {\"ppq\": " << num (ppq) << ", \"bar_len\": " << num (bar)
          << ", \"uniform\": " << locJson (songLocate (*t, ppq, bar, false)) << ", \"general\": " << locJson (songLocate (*t, ppq, bar, true)) << "}"
          << (i + 1 < 200 ? ",\n" : "\n");
    }
    s << "  ]\n}\n";
    if (out == juce::File()) { std::fputs (s.toRawUTF8(), stdout); return 0; }
    if (! out.replaceWithText (s, false, false, "\n")) return fail ("could not write " + out.getFullPathName());
    return 0;
}

// ---- --scenario ----------------------------------------------------------------------------------

struct Event
{
    bool hasAt = false; double atPpq = 0.0; int stoppedBlocks = -1;   // the trigger: at_ppq (hasAt) or stopped_blocks, exactly one
    juce::String param; float value = 0.0f; bool hasParam = false;
    double bpm = 0.0, seekTo = -1.0; int num = 0, den = 0; bool stop = false, play = false;
    bool fired = false;
};

/** Every scenario setting, read and checked before anything plays (a typo is an error, never a silent default). */
struct Scenario
{
    juce::String name;
    double sr = 48000.0, bpm = 120.0, bars = 8.0; int block = 512, num = 4, den = 4;
    juce::var song;                                       // void = defaultSong(); a string = a factory song; an object = a Song
    juce::var key, scale, take;                           // void = the factory song's, else the processor's
    std::vector<std::pair<MidiStudioProcessor::Feel, bool>> feel;
    std::array<juce::String, 6> players, targets;         // "" = the processor default
    std::vector<std::pair<juce::String, float>> params;   // in file order
    std::vector<Event> events;
};

bool readScenario (const juce::var& j, Scenario& s, juce::String& error)
{
    auto* o = j.getDynamicObject();
    if (o == nullptr) { error = "the scenario is not a JSON object"; return false; }
    static const juce::StringArray known { "name", "pending", "description", "sample_rate", "block", "bpm", "num", "den", "bars", "song",
                                           "key", "scale", "take", "feel", "players", "targets", "params", "events", "library_overlay" };
    for (auto& p : o->getProperties())
        if (! known.contains (p.name.toString())) { error = "unknown key \"" + p.name.toString() + "\""; return false; }
    if (! j["name"].isString() || j["name"].toString().isEmpty()) { error = "no \"name\""; return false; }
    s.name = j["name"].toString();
    if (const auto lo = j["library_overlay"]; ! lo.isVoid() && ! isOverlayName (lo))
    { error = "library_overlay must name a folder of tests/fixtures/libraries"; return false; }
    auto number = [&] (const char* k, double& v, double lo, double hi)
    {
        const auto x = j[k];
        if (x.isVoid()) return true;
        if (! isNumber (x) || (double) x < lo || (double) x > hi) { error = juce::String (k) + " must be a number in " + num (lo) + ".." + num (hi); return false; }
        v = (double) x; return true;
    };
    double block = s.block, n = s.num, d = s.den;
    if (! number ("sample_rate", s.sr, 8000.0, 384000.0) || ! number ("block", block, 1.0, 8192.0) || ! number ("bpm", s.bpm, 20.0, 999.0)
        || ! number ("num", n, 1.0, 32.0) || ! number ("den", d, 1.0, 32.0) || ! number ("bars", s.bars, 0.0, 4096.0)) return false;
    for (auto k : { "block", "num", "den" })
        if (! j[k].isVoid() && ! isInteger (j[k])) { error = juce::String (k) + " must be a whole number"; return false; }
    s.block = (int) block; s.num = (int) n; s.den = (int) d;
    s.song = j["song"];
    if (! s.song.isVoid() && ! s.song.isString() && s.song.getDynamicObject() == nullptr) { error = "song must be a factory song name or a Song object"; return false; }
    for (auto [k, dst] : { std::pair<const char*, juce::var*> { "key", &s.key }, { "scale", &s.scale }, { "take", &s.take } })
    {
        *dst = j[k];
        if (! dst->isVoid() && ! isNumber (*dst)) { error = juce::String (k) + " must be a number"; return false; }
    }
    if (const auto f = j["feel"]; ! f.isVoid())
    {
        auto* fo = f.getDynamicObject();
        if (fo == nullptr) { error = "feel must be an object"; return false; }
        static const std::pair<const char*, MidiStudioProcessor::Feel> names[] = {
            { "natural", MidiStudioProcessor::feelNatural }, { "tension", MidiStudioProcessor::feelTension },
            { "bass_lock", MidiStudioProcessor::feelBassLock }, { "fills_yield", MidiStudioProcessor::feelFillsYield },
            { "leave_space", MidiStudioProcessor::feelLeaveSpace }, { "dynamics", MidiStudioProcessor::feelDynamics },
            { "mod_wheel", MidiStudioProcessor::feelModWheel } };
        for (auto& p : fo->getProperties())
        {
            bool found = false;
            for (auto& [nm, fl] : names)
                if (p.name.toString() == nm)
                {
                    if (! p.value.isBool()) { error = "feel." + p.name.toString() + " must be true or false"; return false; }
                    s.feel.push_back ({ fl, (bool) p.value }); found = true;
                }
            if (! found) { error = "unknown feel flag \"" + p.name.toString() + "\""; return false; }
        }
    }
    for (auto [k, dst] : { std::pair<const char*, std::array<juce::String, 6>*> { "players", &s.players }, { "targets", &s.targets } })
    {
        const auto a = j[k];
        if (a.isVoid()) continue;
        if (! a.isArray() || a.size() != 6) { error = juce::String (k) + " must be an array of 6 names or nulls"; return false; }
        for (int i = 0; i < 6; ++i)
        {
            if (a[i].isVoid()) continue;
            if (! a[i].isString() || a[i].toString().isEmpty()) { error = juce::String (k) + "[" + juce::String (i) + "] must be a name or null"; return false; }
            (*dst)[(size_t) i] = a[i].toString();
        }
    }
    if (const auto p = j["params"]; ! p.isVoid())
    {
        auto* po = p.getDynamicObject();
        if (po == nullptr) { error = "params must be an object"; return false; }
        for (auto& q : po->getProperties())
        {
            if (! isNumber (q.value)) { error = "params." + q.name.toString() + " must be a number"; return false; }
            s.params.push_back ({ q.name.toString(), (float) (double) q.value });
        }
    }
    if (const auto ev = j["events"]; ! ev.isVoid())
    {
        if (! ev.isArray()) { error = "events must be an array"; return false; }
        for (int i = 0; i < ev.size(); ++i)
        {
            const auto e = ev[i];
            const juce::String at = "events[" + juce::String (i) + "]";
            auto* eo = e.getDynamicObject();
            if (eo == nullptr) { error = at + " is not an object"; return false; }
            static const juce::StringArray eknown { "at_ppq", "stopped_blocks", "param", "value", "seek_to", "bpm", "meter", "stop", "play" };
            for (auto& p : eo->getProperties())
                if (! eknown.contains (p.name.toString())) { error = at + ": unknown key \"" + p.name.toString() + "\""; return false; }
            Event x;
            const bool hasAt = ! e["at_ppq"].isVoid(), hasStopped = ! e["stopped_blocks"].isVoid();
            if (hasAt == hasStopped) { error = at + " needs exactly one of at_ppq / stopped_blocks"; return false; }
            if (hasAt)
            {
                if (! isNumber (e["at_ppq"]) || ! std::isfinite ((double) e["at_ppq"]) || (double) e["at_ppq"] < 0.0) { error = at + ".at_ppq must be a number >= 0"; return false; }
                x.hasAt = true; x.atPpq = (double) e["at_ppq"];
            }
            if (hasStopped)
            {
                if (! isInteger (e["stopped_blocks"]) || (double) e["stopped_blocks"] < 0.0) { error = at + ".stopped_blocks must be a whole number >= 0"; return false; }
                x.stoppedBlocks = (int) e["stopped_blocks"];
            }
            if (! e["param"].isVoid())
            {
                if (! e["param"].isString() || ! isNumber (e["value"])) { error = at + ": param needs a name and a numeric value"; return false; }
                x.hasParam = true; x.param = e["param"].toString(); x.value = (float) (double) e["value"];
            }
            else if (! e["value"].isVoid()) { error = at + ": value without param"; return false; }
            if (! e["bpm"].isVoid()) { if (! isNumber (e["bpm"]) || (double) e["bpm"] < 20.0 || (double) e["bpm"] > 999.0) { error = at + ".bpm must be 20..999"; return false; } x.bpm = (double) e["bpm"]; }
            if (! e["seek_to"].isVoid()) { if (! isNumber (e["seek_to"]) || (double) e["seek_to"] < 0.0) { error = at + ".seek_to must be >= 0"; return false; } x.seekTo = (double) e["seek_to"]; }
            if (const auto m = e["meter"]; ! m.isVoid())
            {
                if (! m.isArray() || m.size() != 2 || ! isInteger (m[0]) || ! isInteger (m[1]) || (double) m[0] < 1.0 || (double) m[1] < 1.0
                    || (double) m[0] > 32.0 || (double) m[1] > 32.0) { error = at + ".meter must be [num, den], whole numbers 1..32"; return false; }
                x.num = (int) m[0]; x.den = (int) m[1];
            }
            if (! e["stop"].isVoid()) { if (! e["stop"].isBool()) { error = at + ".stop must be true or false"; return false; } x.stop = (bool) e["stop"]; }
            if (! e["play"].isVoid()) { if (! e["play"].isBool()) { error = at + ".play must be true or false"; return false; } x.play = (bool) e["play"]; }
            s.events.push_back (x);
        }
    }
    return true;
}

int runScenario (const juce::File& library, const juce::File& file, const juce::File& outFile)
{
    juce::MemoryBlock bytes;
    if (! file.loadFileAsData (bytes)) return fail ("could not read " + file.getFullPathName());
    juce::var j;
    if (const auto r = juce::JSON::parse (bytes.toString(), j); r.failed()) return fail (file.getFullPathName() + ": " + r.getErrorMessage());
    Scenario s; juce::String error;
    if (! readScenario (j, s, error)) return fail (file.getFullPathName() + ": " + error);
    if (s.name != file.getFileNameWithoutExtension()) return fail (file.getFullPathName() + ": name \"" + s.name + "\" is not the file's name");

    // The song (and a factory song's own Key / Scale / Take / Natural / Tension for what the scenario leaves out).
    std::optional<Song> song;
    std::optional<SongFileData> factory;
    if (s.song.isString())
    {
        factory = factorySong (library, s.song.toString(), error);
        if (! factory) return fail (error);
        song = factory->song;
    }
    else if (s.song.getDynamicObject() != nullptr)
    {
        song = songFromVar (s.song, error);
        if (! song) return fail ("song: " + error);
    }

    MidiStudioProcessor p (true);              // the offline clone: no setup migration, no user default band
    auto setParam = [&] (const juce::String& id, float v)
    {
        if (p.apvts.getParameter (id) == nullptr) { error = "no such parameter \"" + id + "\""; return false; }
        p.setParamValue (id, v);
        return true;
    };
    if (song) p.setSong (*song);
    auto pick = [&] (const juce::var& v, int fromSong) { return ! v.isVoid() ? std::optional<float> ((float) (double) v) : (factory ? std::optional<float> ((float) fromSong) : std::nullopt); };
    if (auto v = pick (s.key, factory ? factory->key : 0))     if (! setParam ("key", *v)) return fail (error);
    if (auto v = pick (s.scale, factory ? factory->scale : 0)) if (! setParam ("scale", *v)) return fail (error);
    if (auto v = pick (s.take, factory ? factory->take : 1))   if (! setParam ("take", *v)) return fail (error);
    if (factory)
    {
        p.setFeel (MidiStudioProcessor::feelNatural, factory->natural);
        p.setFeel (MidiStudioProcessor::feelTension, factory->tension);
    }
    for (auto& [f, on] : s.feel) p.setFeel (f, on);
    // Instruments first (mK_instrument params, in file order), so a player of a non-default instrument can be named;
    // each change re-picks the players as the processor does (handleAsyncUpdate), and setPlayer then overrides.
    auto isInstrument = [] (const juce::String& id) { return id.length() == 13 && id.startsWith ("m") && id.endsWith ("_instrument"); };
    for (auto& [id, v] : s.params) if (isInstrument (id) && ! setParam (id, v)) return fail (error);
    for (int k = 0; k < 6; ++k)
        if (s.players[(size_t) k].isNotEmpty() && ! p.setPlayer (k, s.players[(size_t) k]))
            return fail ("musician " + juce::String (k + 1) + ": no player \"" + s.players[(size_t) k] + "\" for its instrument");
    for (int k = 0; k < 6; ++k)
        if (s.targets[(size_t) k].isNotEmpty() && ! p.setTargetByName (k, s.targets[(size_t) k]))
            return fail ("musician " + juce::String (k + 1) + ": no target \"" + s.targets[(size_t) k] + "\"");
    for (auto& [id, v] : s.params) if (! isInstrument (id) && ! setParam (id, v)) return fail (error);   // the rest, in file order
    for (auto& e : s.events) if (e.hasParam && p.apvts.getParameter (e.param) == nullptr) return fail ("events: no such parameter \"" + e.param + "\"");

    // Harness.h renderBand, block by block: the head steps `block / sr * bpm / 60` while playing.
    std::vector<MidiEvent> tap; tap.reserve (1 << 16);
    p.setRenderTap (&tap);
    Head h; h.bpm = s.bpm; h.num = s.num; h.den = s.den;
    p.setPlayHead (&h); p.prepareToPlay (s.sr, s.block);
    juce::AudioBuffer<float> buf (2, s.block); juce::MidiBuffer mb;
    const double endPpq = s.bars * ((double) s.num * 4.0 / (double) s.den);     // the first meter's bars
    juce::String lines;
    lines << "# case " << fnv1a64Hex (caseBytes (file, bytes, j["library_overlay"].isString() ? j["library_overlay"].toString() : juce::String())) << "\n";
    int blockIndex = 0, stoppedFor = 0; size_t next = 0;
    auto playBlock = [&]
    {
        tap.clear(); mb.clear();
        p.processBlock (buf, mb);
        for (auto& e : tap)
            lines << blockIndex << " " << e.offset << " " << (int) e.status << " " << (int) e.d1 << " " << (int) e.d2 << "\n";
        ++blockIndex;
    };
    constexpr int maxBlocks = 2000000;
    for (;;)
    {
        // Due events, in file order: at_ppq when this block starts at or after it; stopped_blocks once the transport
        // has been stopped for that many blocks.
        while (next < s.events.size())
        {
            auto& e = s.events[next];
            const bool due = e.hasAt ? h.ppq >= e.atPpq : (! h.playing && stoppedFor >= e.stoppedBlocks);
            if (! due) break;
            if (e.hasParam) p.setParamValue (e.param, e.value);
            if (e.bpm > 0.0) h.bpm = e.bpm;
            if (e.num > 0) { h.num = e.num; h.den = e.den; }
            if (e.seekTo >= 0.0) h.ppq = e.seekTo;
            if (e.stop && h.playing) { h.playing = false; stoppedFor = 0; }
            if (e.play) h.playing = true;
            e.fired = true; ++next;
        }
        if (h.playing && h.ppq >= endPpq) break;
        if (! h.playing && ! (next < s.events.size() && ! s.events[next].hasAt)) break;   // stopped for good
        if (blockIndex >= maxBlocks) return fail ("more than " + juce::String (maxBlocks) + " blocks: the scenario never ends");
        playBlock();
        if (h.playing) h.ppq += s.block / s.sr * h.bpm / 60.0;
        else ++stoppedFor;
    }
    h.playing = false; playBlock();            // then one block with the transport stopped
    p.setPlayHead (nullptr); p.setRenderTap (nullptr);
    for (size_t i = 0; i < s.events.size(); ++i)
        if (! s.events[i].fired) std::fprintf (stderr, "band_ref: %s: events[%d] never fired\n", s.name.toRawUTF8(), (int) i);

    if (! outFile.replaceWithText (lines, false, false, "\n")) return fail ("could not write " + outFile.getFullPathName());
    std::printf ("%s: %d blocks -> %s\n", s.name.toRawUTF8(), blockIndex, outFile.getFullPathName().toRawUTF8());
    return 0;
}
}

int main (int argc, char** argv)
{
    juce::ScopedJuceInitialiser_GUI init;
    juce::String scenario, out, dump, library;
    for (int i = 1; i < argc; ++i)
    {
        const juce::String a (argv[i]);
        const bool hasValue = i + 1 < argc;
        if (a == "--scenario" && hasValue) scenario = argv[++i];
        else if (a == "--out" && hasValue) out = argv[++i];
        else if (a == "--dump-timeline" && hasValue) dump = argv[++i];
        else if (a == "--library" && hasValue) library = argv[++i];
        else return fail ("usage: BandRef --scenario <file.json> --out <file.events> [--library <dir>]\n"
                          "       BandRef --dump-timeline <factory song name> [--out <file.json>] [--library <dir>]");
    }
    if (scenario.isEmpty() == dump.isEmpty()) return fail ("give exactly one of --scenario / --dump-timeline");
    if (scenario.isNotEmpty() && out.isEmpty()) return fail ("--scenario needs --out");

    // The Library: the one given, else a throw-away copy of the repo's (nothing of MS's, nothing written back).
    juce::File lib, tempCopy;
    if (library.isNotEmpty()) lib = juce::File::getCurrentWorkingDirectory().getChildFile (library);
    else
    {
        const auto repoLib = juce::File (BAND_REPO_DIR).getChildFile ("crates").getChildFile ("band").getChildFile ("library");
        tempCopy = uniqueTempDir ("band-ref-lib-");
        if (! repoLib.isDirectory() || ! repoLib.copyDirectoryTo (tempCopy))
        { tempCopy.deleteRecursively(); return fail ("could not copy " + repoLib.getFullPathName() + " to " + tempCopy.getFullPathName()); }
        lib = tempCopy;
    }
    if (! lib.isDirectory()) return fail ("no Library folder at " + lib.getFullPathName());
    const auto cwd = juce::File::getCurrentWorkingDirectory();
    if (scenario.isNotEmpty())
    {
        // The overlay is read before the processor exists (MIDISTUDIO_LIBRARY is read once, by its constructor).
        const auto file = cwd.getChildFile (scenario);
        const auto j = juce::JSON::parse (file.loadFileAsString());
        juce::String error;
        if (const auto lo = j["library_overlay"]; ! lo.isVoid())
        {
            if (! isOverlayName (lo)) error = file.getFullPathName() + ": library_overlay must name a folder of tests/fixtures/libraries";
            else overlayLibrary (file, lo.toString(), lib, tempCopy, error);
        }
        if (error.isNotEmpty()) { if (tempCopy != juce::File()) tempCopy.deleteRecursively(); return fail (error); }
    }
    _putenv_s ("MIDISTUDIO_LIBRARY", lib.getFullPathName().toRawUTF8());
    _putenv_s ("MIDISTUDIO_SONG", "");          // removed: the default song is defaultSong(), never a file

    const int rc = dump.isNotEmpty() ? dumpTimeline (lib, dump, out.isEmpty() ? juce::File() : cwd.getChildFile (out))
                                     : runScenario (lib, cwd.getChildFile (scenario), cwd.getChildFile (out));
    if (tempCopy != juce::File()) tempCopy.deleteRecursively();
    return rc;
}
