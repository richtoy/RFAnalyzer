/*
 * nativedab - DAB/DAB+ receiver bridge for Richard's personal RF Analyzer fork.
 * License: GPL-2.0-or-later.
 */
#include "dab_bridge.h"

#include <cstdio>
#include <cstring>

// ---------------------------------------------------------------- PushDevice

PushDevice::PushDevice() : buffer(4 * 1024 * 1024) {}   // ~2 s at 2.048 Msps
PushDevice::~PushDevice() = default;

int32_t PushDevice::getSamples(std::complex<float> *v, int32_t n) {
    return buffer.getDataFromBuffer(v, n);
}

int32_t PushDevice::Samples() {
    return buffer.GetRingBufferReadAvailable();
}

void PushDevice::resetBuffer() {
    buffer.FlushRingBuffer();
}

int32_t PushDevice::push(const float *re, const float *im, int32_t n) {
    if ((int32_t) scratch.size() < n) scratch.resize(n);
    for (int32_t i = 0; i < n; i++) scratch[i] = std::complex<float>(re[i], im[i]);
    int32_t space = buffer.GetRingBufferWriteAvailable();
    int32_t toWrite = n < space ? n : space;
    buffer.putDataIntoBuffer(scratch.data(), toWrite);
    return n - toWrite;
}

// ---------------------------------------------------------------- JSON

std::string jsonEscape(const std::string &s) {
    std::string out;
    out.reserve(s.size() + 8);
    for (unsigned char c : s) {
        switch (c) {
            case '"': out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:
                if (c < 0x20) { char b[8]; snprintf(b, sizeof b, "\\u%04x", c); out += b; }
                else out += (char) c;
        }
    }
    return out;
}

// ---------------------------------------------------------------- callbacks

static DabBridge *B(void *ctx) { return static_cast<DabBridge *>(ctx); }

static void cbSync(bool b, void *ctx) {
    B(ctx)->addEvent(std::string("{\"t\":\"sync\",\"v\":") + (b ? "true" : "false") + "}");
}

static void cbSystem(bool sync, int16_t snr, int32_t freqOffset, void *ctx) {
    char b[128];
    snprintf(b, sizeof b, "{\"t\":\"sys\",\"sync\":%s,\"snr\":%d,\"off\":%d}", sync ? "true" : "false", snr, freqOffset);
    B(ctx)->addEvent(b);
}

static void cbFic(int16_t q, void *ctx) {
    char b[64];
    snprintf(b, sizeof b, "{\"t\":\"fic\",\"q\":%d}", q);
    B(ctx)->addEvent(b);
}

static void cbEnsemble(const std::string &name, int32_t id, void *ctx) {
    char b[64];
    snprintf(b, sizeof b, "\",\"id\":%d}", id);
    B(ctx)->addEvent("{\"t\":\"ens\",\"name\":\"" + jsonEscape(name) + b);
}

static void cbService(const std::string &name, int32_t sid, uint16_t subId, void *ctx) {
    char b[96];
    snprintf(b, sizeof b, "\",\"sid\":%u,\"sub\":%u}", (uint32_t) sid, subId);
    B(ctx)->addEvent("{\"t\":\"svc\",\"name\":\"" + jsonEscape(name) + b);
    B(ctx)->onServiceName(name, sid);
}

static void cbTime(int h, int m, void *ctx) {
    char b[64];
    snprintf(b, sizeof b, "{\"t\":\"time\",\"h\":%d,\"m\":%d}", h, m);
    B(ctx)->addEvent(b);
}

static void cbAudio(int16_t *buf, int size, int rate, bool stereo, void *ctx) {
    (void) stereo;
    B(ctx)->onAudio(buf, size, rate);
}

static void cbDls(const char *text, void *ctx) {
    B(ctx)->addEvent("{\"t\":\"dls\",\"text\":\"" + jsonEscape(text ? text : "") + "\"}");
}

static void cbBytes(uint8_t *, int16_t, uint8_t, void *) {}

static void emitProgram(const char *type, audiodata *d, void *ctx) {
    if (d == nullptr) return;
    char b[384];
    snprintf(b, sizeof b,
             "\",\"sid\":%u,\"pty\":%d,\"subch\":%d,\"start\":%d,\"length\":%d,\"short\":%s,\"prot\":%d,"
             "\"bitrate\":%d,\"ascty\":%d,\"lang\":%d}",
             d->SId, d->programType, d->subchId, d->startAddr, d->length, d->shortForm ? "true" : "false",
             d->protLevel, d->bitRate, d->ASCTy, d->language);
    B(ctx)->addEvent(std::string("{\"t\":\"") + type + "\",\"name\":\"" + jsonEscape(d->serviceName) +
                     "\",\"shortName\":\"" + jsonEscape(d->shortName) + b);
}

static void cbProgram(audiodata *d, void *ctx) { emitProgram("prog", d, ctx); }

static void cbQuality(int16_t fe, int16_t rs, int16_t aac, void *ctx) {
    char b[96];
    snprintf(b, sizeof b, "{\"t\":\"pq\",\"fe\":%d,\"rs\":%d,\"aac\":%d}", fe, rs, aac);
    B(ctx)->addEvent(b);
}

static void cbMot(uint8_t *data, int size, const char *name, int type, void *ctx) {
    B(ctx)->onSlide(data, size, name, type);
}

static void cbTii(tiiData *t, void *ctx) {
    if (t == nullptr) return;
    char b[160];
    snprintf(b, sizeof b, "{\"t\":\"tii\",\"main\":%d,\"sub\":%d,\"strength\":%.2f,\"ecc\":%d,\"eid\":%u}",
             t->mainId, t->subId, t->strength, t->ecc, t->EId);
    B(ctx)->addEvent(b);
}

// ---------------------------------------------------------------- DabBridge

DabBridge::DabBridge() : audio(48000 * 2 * 4), iqBuffer(8192) {
    api.dabMode = 1;
    api.thresholdValue = 3;
    api.syncsignal_Handler = cbSync;
    api.systemdata_Handler = cbSystem;
    api.name_of_ensemble = cbEnsemble;
    api.serviceName = cbService;
    api.fib_quality_Handler = cbFic;
    api.audioOut_Handler = cbAudio;
    api.dataOut_Handler = cbDls;
    api.bytesOut_Handler = cbBytes;
    api.programdata_Handler = cbProgram;
    api.program_quality_Handler = cbQuality;
    api.motdata_Handler = cbMot;
    api.tii_data_Handler = cbTii;
    api.timeHandler = cbTime;
    radio = dabInit(&device, &api, nullptr, &iqBuffer, this);
}

DabBridge::~DabBridge() {
    stop();
    if (radio != nullptr) dabExit(radio);
    radio = nullptr;
}

void DabBridge::start() {
    if (running || radio == nullptr) return;
    dabStartProcessing(radio);
    running = true;
}

void DabBridge::stop() {
    if (!running || radio == nullptr) return;
    dabStop(radio);
    running = false;
}

void DabBridge::reset() {
    if (radio == nullptr) return;
    {
        std::lock_guard<std::mutex> lock(mutex);
        services.clear();
        events.clear();
        slideNew = false;
    }
    bool wasRunning = running;
    stop();                       // joins the processor thread
    device.resetBuffer();
    audio.FlushRingBuffer();
    if (wasRunning) start();      // run() starts with a fresh FIC/ensemble state
}

int32_t DabBridge::push(const float *re, const float *im, int32_t n) {
    int32_t dropped = device.push(re, im, n);
    if (dropped > 0) droppedSamples += dropped;
    return dropped;
}

bool DabBridge::selectService(const std::string &name) {
    if (radio == nullptr || !is_audioService(radio, name)) return false;
    audiodata ad{};
    ad.defined = false;
    dataforAudioService(radio, name, ad, 0);
    if (!ad.defined) return false;
    // Until FIG 0/1 (subchannel organisation) has arrived, the service's subchannel can be
    // incomplete; starting the MSC backend with it reads past its buffers (SIGSEGV). Wait.
    if (!subchannelValid(ad)) return false;
    dabReset_msc(radio);
    set_audioChannel(radio, ad);
    audio.FlushRingBuffer();
    cbProgram(&ad, this);
    return true;
}

bool DabBridge::subchannelValid(const audiodata &ad) {
    return ad.subchId >= 0 && ad.subchId < 64 && ad.startAddr >= 0 && ad.length > 0 &&
           ad.startAddr + ad.length <= 864 && ad.bitRate >= 8 && ad.bitRate <= 384 &&
           ad.protLevel >= 0 && ad.protLevel <= 7;
}

bool DabBridge::selectServiceBySid(uint32_t sid) {
    std::string name;
    {
        std::lock_guard<std::mutex> lock(mutex);
        for (auto &s : services) if (s.second == sid) { name = s.first; break; }
    }
    if (name.empty() && radio != nullptr) name = dab_getserviceName(radio, sid);
    return !name.empty() && selectService(name);
}

bool DabBridge::describeService(uint32_t sid) {
    std::string name;
    {
        std::lock_guard<std::mutex> lock(mutex);
        for (auto &s : services) if (s.second == sid) { name = s.first; break; }
    }
    if (name.empty() || radio == nullptr) return false;
    if (!is_audioService(radio, name)) {
        char b[96];
        snprintf(b, sizeof b, "{\"t\":\"data\",\"sid\":%u}", sid);
        addEvent(b);
        return true;
    }
    audiodata ad{};
    ad.defined = false;
    dataforAudioService(radio, name, ad, 0);
    if (!ad.defined || !subchannelValid(ad)) return false;
    emitProgram("info", &ad, this);
    return true;
}

void DabBridge::onServiceName(const std::string &name, int32_t sid) {
    std::lock_guard<std::mutex> lock(mutex);
    for (auto &s : services) if (s.second == (uint32_t) sid) return;
    services.emplace_back(name, (uint32_t) sid);
}

void DabBridge::addEvent(const std::string &json) {
    std::lock_guard<std::mutex> lock(mutex);
    if (events.size() > 2000) events.pop_front();
    events.push_back(json);
}

std::string DabBridge::pollEvents() {
    std::lock_guard<std::mutex> lock(mutex);
    std::string out;
    while (!events.empty()) {
        out += events.front();
        out += '\n';
        events.pop_front();
    }
    return out;
}

void DabBridge::onAudio(const int16_t *buf, int size, int rate) {
    if (rate > 0) audioRate_.store(rate);
    int32_t space = audio.GetRingBufferWriteAvailable();
    if (size > space) audio.skipDataInBuffer(size - space);
    audio.putDataIntoBuffer(buf, size);
}

int32_t DabBridge::pollAudio(int16_t *out, int32_t capacity) {
    int32_t avail = audio.GetRingBufferReadAvailable();
    int32_t n = avail < capacity ? avail : capacity;
    n &= ~1;   // whole stereo frames
    if (n <= 0) return 0;
    return audio.getDataFromBuffer(out, n);
}

void DabBridge::onSlide(const uint8_t *data, int size, const char *name, int type) {
    if (data == nullptr || size <= 0) return;
    std::lock_guard<std::mutex> lock(mutex);
    slide.assign(data, data + size);
    slideName = name ? name : "";
    slideType = type;
    slideNew = true;
}

bool DabBridge::takeSlide(std::vector<uint8_t> &data, std::string &name, int &type) {
    std::lock_guard<std::mutex> lock(mutex);
    if (!slideNew) return false;
    data = slide;
    name = slideName;
    type = slideType;
    slideNew = false;
    return true;
}

int32_t DabBridge::pollConstellation(float *out, int32_t maxPoints) {
    int32_t avail = iqBuffer.GetRingBufferReadAvailable();
    std::vector<std::complex<float>> tmp(avail);
    if (avail > 0) iqBuffer.getDataFromBuffer(tmp.data(), avail);
    // keep the newest points
    int32_t start = avail > maxPoints ? avail - maxPoints : 0;
    int32_t k = 0;
    for (int32_t i = start; i < avail; i++) {
        float a = std::abs(tmp[i]);
        if (a <= 0) continue;
        out[2 * k] = tmp[i].real() / a;
        out[2 * k + 1] = tmp[i].imag() / a;
        k++;
    }
    return k;
}
