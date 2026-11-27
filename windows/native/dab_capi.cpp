/*
 * C API over the nativedab DabBridge (../nativedab/src/main/cpp) for the Windows RF Analyzer.
 * License: GPL-2.0-or-later.
 *
 * Rust can only call plain C functions, so each DabBridge method gets a C wrapper that takes the
 * bridge as an opaque `void *` handle (created by rfa_dab_create, freed by rfa_dab_destroy).
 * Memory rules: input buffers belong to the caller and are only read during the call. Strings
 * and slides returned to Rust are malloc'd copies that Rust frees with rfa_dab_free, so the
 * C++ objects never hand out pointers into their own storage.
 * DabBridge runs the decoder on its own threads and does its own locking; these wrappers are
 * called from the Rust engine thread only.
 */
#include "dab_bridge.h"
#include <cstdlib>
#include <cstring>

extern "C" {

void *rfa_dab_create() {
    try { return new DabBridge(); } catch (...) { return nullptr; }
}
void rfa_dab_destroy(void *h) { delete static_cast<DabBridge *>(h); }
void rfa_dab_start(void *h) { static_cast<DabBridge *>(h)->start(); }
void rfa_dab_stop(void *h) { static_cast<DabBridge *>(h)->stop(); }

/* Interleaved complex float samples. Returns the number dropped (buffer full).
 * DabBridge wants separate I and Q arrays, so de-interleave into scratch vectors that persist
 * per thread (thread_local) to avoid an allocation on every call. */
int rfa_dab_push(void *h, const float *iq, int n) {
    static thread_local std::vector<float> re, im;
    if ((int) re.size() < n) { re.resize(n); im.resize(n); }
    for (int i = 0; i < n; i++) { re[i] = iq[2 * i]; im[i] = iq[2 * i + 1]; }
    return static_cast<DabBridge *>(h)->push(re.data(), im.data(), n);
}

int rfa_dab_select_sid(void *h, uint32_t sid) { return static_cast<DabBridge *>(h)->selectServiceBySid(sid) ? 1 : 0; }
int rfa_dab_describe(void *h, uint32_t sid) { return static_cast<DabBridge *>(h)->describeService(sid) ? 1 : 0; }

/* Returns a malloc'd NUL-terminated string of JSON lines, or NULL if none. Free with rfa_dab_free. */
char *rfa_dab_poll_events(void *h) {
    std::string s = static_cast<DabBridge *>(h)->pollEvents();
    if (s.empty()) return nullptr;
    char *out = (char *) malloc(s.size() + 1);
    memcpy(out, s.data(), s.size());
    out[s.size()] = 0;
    return out;
}

void rfa_dab_free(void *p) { free(p); }

int rfa_dab_poll_audio(void *h, int16_t *out, int cap) { return static_cast<DabBridge *>(h)->pollAudio(out, cap); }
int rfa_dab_audio_rate(void *h) { return static_cast<DabBridge *>(h)->audioRate(); }

/* Returns 1 and a malloc'd copy of the newest slide (free with rfa_dab_free), 0 if no new slide. */
int rfa_dab_take_slide(void *h, uint8_t **data, int *size, int *type, char *name, int name_cap) {
    std::vector<uint8_t> d;
    std::string n;
    int t = 0;
    if (!static_cast<DabBridge *>(h)->takeSlide(d, n, t)) return 0;
    /* malloc(0) may return NULL, so always allocate at least one byte */
    *data = (uint8_t *) malloc(d.size() ? d.size() : 1);
    memcpy(*data, d.data(), d.size());
    *size = (int) d.size();
    *type = t;
    if (name && name_cap > 0) {
        strncpy(name, n.c_str(), name_cap - 1);
        name[name_cap - 1] = 0;
    }
    return 1;
}

/* Writes up to max_points (x, y) pairs into out[0 .. 2*max_points); returns the count. */
int rfa_dab_poll_constellation(void *h, float *out, int max_points) {
    return static_cast<DabBridge *>(h)->pollConstellation(out, max_points);
}

uint32_t rfa_dab_dropped(void *h) { return static_cast<DabBridge *>(h)->droppedSamples.load(); }

}
