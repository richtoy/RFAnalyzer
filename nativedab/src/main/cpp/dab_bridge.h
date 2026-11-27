/*
 * nativedab - DAB/DAB+ receiver bridge for Richard's personal RF Analyzer fork.
 * License: GPL-2.0-or-later (links the dab-cmdline library and libfaad2).
 *
 * Wraps the dab-cmdline library behind a push/poll interface that suits RF Analyzer's
 * Demodulator thread: IQ samples at 2.048 Msps are pushed in, events (JSON lines), PCM audio and
 * MOT slides are polled out. No callbacks into Java from the library's threads.
 */
#pragma once

#include <atomic>
#include <complex>
#include <cstdint>
#include <deque>
#include <mutex>
#include <string>
#include <vector>

#include "dab-api.h"
#include "device-handler.h"
#include "ringbuffer.h"

/** Input device for the library: samples arrive through push() from RF Analyzer. */
class PushDevice : public deviceHandler {
public:
    PushDevice();
    ~PushDevice() override;
    int32_t getSamples(std::complex<float> *v, int32_t n) override;
    int32_t Samples() override;
    void resetBuffer() override;
    bool restartReader(int32_t) override { return true; }
    void stopReader() override {}
    /** Returns the number of samples dropped because the buffer was full. */
    int32_t push(const float *re, const float *im, int32_t n);
private:
    RingBuffer<std::complex<float>> buffer;
    std::vector<std::complex<float>> scratch;
};

class DabBridge {
public:
    DabBridge();
    ~DabBridge();
    void start();
    void stop();
    /** Retune: forget the ensemble and resynchronise. */
    void reset();
    int32_t push(const float *re, const float *im, int32_t n);
    /** Selects an audio service by its label; false if unknown or not audio. */
    bool selectService(const std::string &name);
    bool selectServiceBySid(uint32_t sid);
    /** Emits an "info" event (subchannel, bitrate, protection...) for a service without selecting it,
     *  or a "data" event for a non-audio service. False while the service's FIG 0/2 isn't known yet. */
    bool describeService(uint32_t sid);
    /** All queued events as JSON lines separated by '\n' ("" if none). */
    std::string pollEvents();
    /** Interleaved stereo int16 audio; returns the number of int16 values written. */
    int32_t pollAudio(int16_t *out, int32_t capacity);
    int32_t audioRate() const { return audioRate_.load(); }
    /** The newest MOT slide (empty if none since the last call) and its content name/type. */
    bool takeSlide(std::vector<uint8_t> &data, std::string &name, int &type);
    /** Up to n constellation points (interleaved I/Q floats), for the signal page. */
    int32_t pollConstellation(float *out, int32_t maxPoints);

    // --- called from library callbacks ---
    void addEvent(const std::string &json);
    void onAudio(const int16_t *buf, int size, int rate);
    void onSlide(const uint8_t *data, int size, const char *name, int type);
    void onServiceName(const std::string &name, int32_t sid);

    std::atomic<uint32_t> droppedSamples{0};

private:
    PushDevice device;
    API_struct api{};
    void *radio = nullptr;
    bool running = false;
    std::mutex mutex;
    std::deque<std::string> events;
    RingBuffer<int16_t> audio;
    std::atomic<int32_t> audioRate_{48000};
    std::vector<uint8_t> slide;
    std::string slideName;
    int slideType = 0;
    bool slideNew = false;
    RingBuffer<std::complex<float>> iqBuffer;
    std::vector<std::pair<std::string, uint32_t>> services;
    static bool subchannelValid(const audiodata &ad);
};

std::string jsonEscape(const std::string &s);
