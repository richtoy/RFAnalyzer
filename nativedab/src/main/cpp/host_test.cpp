/*
 * Desktop test for the DAB bridge: decodes a raw IQ file (unsigned 8-bit RTL-SDR, signed 8-bit
 * HackRF or float32 IQ) at 2.048 Msps and prints the events; optionally plays a service and writes
 * its audio to a raw s16le stereo file.
 *
 *   dab_host_test <file> <u8|s8|f32> [service name] [audio.raw]
 *
 * License: GPL-2.0-or-later.
 */
#include "dab_bridge.h"
#include <chrono>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: %s file u8|s8|f32 [service] [audio.raw]\n", argv[0]); return 1; }
    FILE *f = fopen(argv[1], "rb");
    if (!f) { perror("open"); return 1; }
    std::string fmt = argv[2];
    std::string service = argc > 3 ? argv[3] : "";
    FILE *aout = argc > 4 ? fopen(argv[4], "wb") : nullptr;

    DabBridge bridge;
    bridge.start();
    const int N = 2048 * 50;   // 50 ms
    std::vector<float> re(N), im(N);
    std::vector<uint8_t> raw(N * 8);
    std::vector<int16_t> pcm(48000 * 2);
    bool selected = false;
    long total = 0, audioSamples = 0;
    while (true) {
        int n;
        if (fmt == "f32") {
            n = fread(raw.data(), 8, N, f);
            for (int i = 0; i < n; i++) { memcpy(&re[i], &raw[8 * i], 4); memcpy(&im[i], &raw[8 * i + 4], 4); }
        } else {
            n = fread(raw.data(), 2, N, f);
            for (int i = 0; i < n; i++) {
                if (fmt == "u8") { re[i] = (raw[2 * i] - 127.5f) / 128; im[i] = (raw[2 * i + 1] - 127.5f) / 128; }
                else { re[i] = (int8_t) raw[2 * i] / 128.0f; im[i] = (int8_t) raw[2 * i + 1] / 128.0f; }
            }
        }
        if (n <= 0) break;
        // real time is not needed, but do not overrun the 2 s buffer
        while (bridge.push(re.data(), im.data(), n) > 0) std::this_thread::sleep_for(std::chrono::milliseconds(5));
        total += n;
        std::string ev = bridge.pollEvents();
        if (!ev.empty()) printf("[%.2fs]\n%s", total / 2048000.0, ev.c_str());
        if (!selected && !service.empty() && total > 2048000L * 3) {
            selected = bridge.selectService(service);
            printf("select '%s': %s\n", service.c_str(), selected ? "ok" : "not found");
        }
        int k = bridge.pollAudio(pcm.data(), (int) pcm.size());
        if (k > 0) { audioSamples += k / 2; if (aout) fwrite(pcm.data(), 2, k, aout); }
        std::vector<uint8_t> slide; std::string name; int type;
        if (bridge.takeSlide(slide, name, type)) printf("slide '%s' type %d, %zu bytes\n", name.c_str(), type, slide.size());
        std::this_thread::sleep_for(std::chrono::milliseconds(2));
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(1500));
    printf("%s", bridge.pollEvents().c_str());
    printf("input %.1f s, audio %.1f s at %d Hz, dropped %u\n", total / 2048000.0, audioSamples / (double) bridge.audioRate(),
           bridge.audioRate(), bridge.droppedSamples.load());
    bridge.stop();
    if (aout) fclose(aout);
    return 0;
}
