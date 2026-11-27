/*
 * JNI for com.mantz_it.nativedab.NativeDab - Richard's personal RF Analyzer fork.
 * License: GPL-2.0-or-later.
 */
#include <jni.h>
#include <android/log.h>
#include "dab_bridge.h"

#define TAG "nativedab"

static DabBridge *bridge(jlong h) { return reinterpret_cast<DabBridge *>(h); }

extern "C" {

JNIEXPORT jlong JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativeCreate(JNIEnv *, jclass) {
    auto *b = new DabBridge();
    __android_log_print(ANDROID_LOG_INFO, TAG, "created");
    return reinterpret_cast<jlong>(b);
}

JNIEXPORT void JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativeDestroy(JNIEnv *, jclass, jlong h) {
    delete bridge(h);
}

JNIEXPORT void JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativeStart(JNIEnv *, jclass, jlong h) {
    bridge(h)->start();
}

JNIEXPORT void JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativeStop(JNIEnv *, jclass, jlong h) {
    bridge(h)->stop();
}

JNIEXPORT void JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativeReset(JNIEnv *, jclass, jlong h) {
    bridge(h)->reset();
}

JNIEXPORT jint JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativePush(JNIEnv *env, jclass, jlong h, jfloatArray re, jfloatArray im, jint n) {
    jfloat *r = env->GetFloatArrayElements(re, nullptr);
    jfloat *i = env->GetFloatArrayElements(im, nullptr);
    jint dropped = bridge(h)->push(r, i, n);
    env->ReleaseFloatArrayElements(re, r, JNI_ABORT);
    env->ReleaseFloatArrayElements(im, i, JNI_ABORT);
    return dropped;
}

JNIEXPORT jboolean JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativeSelectService(JNIEnv *env, jclass, jlong h, jstring name) {
    const char *c = env->GetStringUTFChars(name, nullptr);
    std::string s(c);
    env->ReleaseStringUTFChars(name, c);
    return bridge(h)->selectService(s) ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT jboolean JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativeSelectServiceBySid(JNIEnv *, jclass, jlong h, jint sid) {
    return bridge(h)->selectServiceBySid((uint32_t) sid) ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT jboolean JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativeDescribeService(JNIEnv *, jclass, jlong h, jint sid) {
    return bridge(h)->describeService((uint32_t) sid) ? JNI_TRUE : JNI_FALSE;
}

/* Returned as bytes: the labels are UTF-8 and may hold characters NewStringUTF rejects. */
JNIEXPORT jbyteArray JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativePollEvents(JNIEnv *env, jclass, jlong h) {
    std::string s = bridge(h)->pollEvents();
    if (s.empty()) return nullptr;
    jbyteArray a = env->NewByteArray((jsize) s.size());
    env->SetByteArrayRegion(a, 0, (jsize) s.size(), reinterpret_cast<const jbyte *>(s.data()));
    return a;
}

JNIEXPORT jint JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativePollAudio(JNIEnv *env, jclass, jlong h, jshortArray out) {
    jsize cap = env->GetArrayLength(out);
    jshort *o = env->GetShortArrayElements(out, nullptr);
    jint n = bridge(h)->pollAudio(o, cap);
    env->ReleaseShortArrayElements(out, o, 0);
    return n;
}

JNIEXPORT jint JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativeAudioRate(JNIEnv *, jclass, jlong h) {
    return bridge(h)->audioRate();
}

JNIEXPORT jbyteArray JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativeTakeSlide(JNIEnv *env, jclass, jlong h) {
    std::vector<uint8_t> data;
    std::string name;
    int type = 0;
    if (!bridge(h)->takeSlide(data, name, type)) return nullptr;
    jbyteArray a = env->NewByteArray((jsize) data.size());
    env->SetByteArrayRegion(a, 0, (jsize) data.size(), reinterpret_cast<const jbyte *>(data.data()));
    return a;
}

JNIEXPORT jint JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativePollConstellation(JNIEnv *env, jclass, jlong h, jfloatArray out) {
    jsize cap = env->GetArrayLength(out) / 2;
    jfloat *o = env->GetFloatArrayElements(out, nullptr);
    jint n = bridge(h)->pollConstellation(o, cap);
    env->ReleaseFloatArrayElements(out, o, 0);
    return n;
}

JNIEXPORT jint JNICALL
Java_com_mantz_1it_nativedab_NativeDab_nativeDroppedSamples(JNIEnv *, jclass, jlong h) {
    return (jint) bridge(h)->droppedSamples.load();
}

}
