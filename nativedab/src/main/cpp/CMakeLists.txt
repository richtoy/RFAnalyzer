# nativedab: DAB/DAB+ receiver for Richard's personal RF Analyzer fork.
#
#   dab/      dab-cmdline library by Jan van Katwijk (GPL-2.0-or-later), FFTW replaced by KISS FFT
#   faad2/    libfaad2 (GPL-2.0-or-later) for DAB+ HE-AAC v2 (sideloaded FOSS build only)
#   kissfft/  KISS FFT (BSD-3-Clause)
#   dab_bridge.cpp / dab_jni.cpp: push-style input, polled events/audio/slides for Kotlin
#
# Builds for Android (externalNativeBuild) and, with -DNATIVEDAB_HOST_TEST=ON, as a desktop test
# tool that decodes a raw IQ file (see host_test.cpp).
cmake_minimum_required(VERSION 3.18.1)
project(nativedab C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -O3 -w")
set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -O3 -w")

option(NATIVEDAB_HOST_TEST "Build the desktop test tool" OFF)

file(GLOB FAAD_SOURCES ${CMAKE_CURRENT_SOURCE_DIR}/faad2/libfaad/*.c)
add_library(faad_static STATIC ${FAAD_SOURCES})
target_include_directories(faad_static PUBLIC faad2/include PRIVATE faad2/libfaad)
target_compile_definitions(faad_static PRIVATE
    HAVE_INTTYPES_H=1 HAVE_MEMCPY=1 HAVE_STRING_H=1 HAVE_STRINGS_H=1
    HAVE_SYS_STAT_H=1 HAVE_SYS_TYPES_H=1 HAVE_LRINTF=1 PACKAGE_VERSION="2.11-rfa")
set_property(TARGET faad_static PROPERTY POSITION_INDEPENDENT_CODE ON)

set(DAB dab)
set(DAB_SOURCES
    ${DAB}/dab-api.cpp
    ${DAB}/device-handler.cpp
    ${DAB}/src/dab-processor.cpp
    ${DAB}/src/time-converter.cpp
    ${DAB}/src/ofdm/ensemble.cpp
    ${DAB}/src/ofdm/phasereference.cpp
    ${DAB}/src/ofdm/phasetable.cpp
    ${DAB}/src/ofdm/freq-interleaver.cpp
    ${DAB}/src/ofdm/ofdm-decoder.cpp
    ${DAB}/src/ofdm/sample-reader.cpp
    ${DAB}/src/ofdm/timesyncer.cpp
    ${DAB}/src/ofdm/fic-handler.cpp
    ${DAB}/src/ofdm/fib-config.cpp
    ${DAB}/src/ofdm/fib-decoder.cpp
    ${DAB}/src/ofdm/tii-detector.cpp
    ${DAB}/src/backend/firecode-checker.cpp
    ${DAB}/src/backend/backend-base.cpp
    ${DAB}/src/backend/galois.cpp
    ${DAB}/src/backend/reed-solomon.cpp
    ${DAB}/src/backend/msc-handler.cpp
    ${DAB}/src/backend/virtual-backend.cpp
    ${DAB}/src/backend/audio-backend.cpp
    ${DAB}/src/backend/data-backend.cpp
    ${DAB}/src/backend/audio/mp4processor.cpp
    ${DAB}/src/backend/audio/mp2processor.cpp
    ${DAB}/src/backend/data/virtual-datahandler.cpp
    ${DAB}/src/backend/data/tdc-datahandler.cpp
    ${DAB}/src/backend/data/pad-handler.cpp
    ${DAB}/src/backend/data/data-processor.cpp
    ${DAB}/src/backend/data/mot/mot-handler.cpp
    ${DAB}/src/backend/data/mot/mot-dir.cpp
    ${DAB}/src/backend/data/mot/mot-object.cpp
    ${DAB}/src/support/band-handler.cpp
    ${DAB}/src/support/charsets.cpp
    ${DAB}/src/support/protection.cpp
    ${DAB}/src/support/protTables.cpp
    ${DAB}/src/support/eep-protection.cpp
    ${DAB}/src/support/uep-protection.cpp
    ${DAB}/src/support/fft-handler.cpp
    ${DAB}/src/support/dab-params.cpp
    ${DAB}/src/support/tii_table.cpp
    ${DAB}/src/support/viterbi-spiral/viterbi-spiral.cpp
    ${DAB}/src/support/viterbi-spiral/spiral-no-sse.c
    kissfft/kiss_fft.c
    dab_bridge.cpp
)
set(DAB_INCLUDES
    ${CMAKE_CURRENT_SOURCE_DIR}
    ${DAB} ${DAB}/includes ${DAB}/includes/ofdm ${DAB}/includes/backend
    ${DAB}/includes/backend/audio ${DAB}/includes/backend/data
    ${DAB}/includes/backend/data/mot ${DAB}/includes/backend/data/epg
    ${DAB}/includes/support ${DAB}/includes/support/viterbi-spiral
    kissfft faad2/include
)


if (NATIVEDAB_HOST_TEST)
    add_executable(dab_host_test ${DAB_SOURCES} host_test.cpp)
    target_include_directories(dab_host_test PRIVATE ${DAB_INCLUDES})
    target_link_libraries(dab_host_test faad_static pthread m)
else ()
    add_library(nativedab SHARED ${DAB_SOURCES} dab_jni.cpp)
    target_include_directories(nativedab PRIVATE ${DAB_INCLUDES})
    target_link_libraries(nativedab faad_static log android m)
endif ()
