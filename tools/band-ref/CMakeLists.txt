# band_ref: MIDI Studio's band (an offline MidiStudioProcessor) played through JSON scenarios, for the
# Rust port's event-for-event fidelity tests. Built by `cargo xtask band-refs` into target/band-ref.
#
# MIDI Studio is read-only: its CMakeLists.txt is added as a subdirectory with its binary dir inside OUR
# build tree, and only the targets BandRef needs (the MIDIStudio shared code, MIDIStudioEngine, JUCE) build.
cmake_minimum_required(VERSION 3.22)
project(BandRef VERSION 0.1.0 LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

set(MIDISTUDIO_DIR "" CACHE PATH "MIDI Studio checkout (required; cargo xtask band-refs passes it)")
if(NOT EXISTS "${MIDISTUDIO_DIR}/Source/PluginProcessor.h")
    message(FATAL_ERROR "MIDISTUDIO_DIR (${MIDISTUDIO_DIR}) is not a MIDI Studio checkout")
endif()
add_subdirectory("${MIDISTUDIO_DIR}" midistudio EXCLUDE_FROM_ALL)

juce_add_console_app(BandRef PRODUCT_NAME "BandRef")
target_sources(BandRef PRIVATE band_ref.cpp)
target_include_directories(BandRef PRIVATE "${MIDISTUDIO_DIR}/Source" "${MIDISTUDIO_DIR}/Common")
# The repo the default --library comes from (crates/band/library).
get_filename_component(BAND_REPO_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)
target_compile_definitions(BandRef PRIVATE BAND_REPO_DIR="${BAND_REPO_DIR}")
target_link_libraries(BandRef PRIVATE MIDIStudio juce::juce_audio_utils juce::juce_recommended_config_flags)
# Exactly MIDI Studio's ms_precise: the same arithmetic as the plugin (and as Rust, which never contracts to FMA).
if(MSVC)
    target_compile_options(BandRef PRIVATE /fp:precise /bigobj /Zc:__cplusplus /utf-8)
else()
    target_compile_options(BandRef PRIVATE -ffp-contract=off)
endif()
