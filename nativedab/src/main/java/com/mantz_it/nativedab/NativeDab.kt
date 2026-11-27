package com.mantz_it.nativedab

/**
 * <h1>RF Analyzer - native DAB/DAB+ receiver</h1>
 *
 * JNI front of the nativedab library (dab-cmdline + libfaad2 + KISS FFT, see src/main/cpp).
 * Push 2.048 Msps complex samples with [nativePush]; poll events (JSON lines, UTF-8), PCM audio
 * (interleaved stereo int16 at [nativeAudioRate]), MOT slides and constellation points.
 * All functions take the handle from [nativeCreate]. Thread-safe enough for one producer
 * (the Demodulator thread) plus pollers.
 *
 * Part of Richard's personal RF Analyzer fork. License: GPL-2.0-or-later.
 */
object NativeDab {
    init {
        System.loadLibrary("nativedab")
    }

    @JvmStatic external fun nativeCreate(): Long
    @JvmStatic external fun nativeDestroy(handle: Long)
    @JvmStatic external fun nativeStart(handle: Long)
    @JvmStatic external fun nativeStop(handle: Long)
    @JvmStatic external fun nativeReset(handle: Long)
    @JvmStatic external fun nativePush(handle: Long, re: FloatArray, im: FloatArray, n: Int): Int
    @JvmStatic external fun nativeSelectService(handle: Long, name: String): Boolean
    @JvmStatic external fun nativeSelectServiceBySid(handle: Long, sid: Int): Boolean
    @JvmStatic external fun nativeDescribeService(handle: Long, sid: Int): Boolean
    @JvmStatic external fun nativePollEvents(handle: Long): ByteArray?
    @JvmStatic external fun nativePollAudio(handle: Long, out: ShortArray): Int
    @JvmStatic external fun nativeAudioRate(handle: Long): Int
    @JvmStatic external fun nativeTakeSlide(handle: Long): ByteArray?
    @JvmStatic external fun nativePollConstellation(handle: Long, out: FloatArray): Int
    @JvmStatic external fun nativeDroppedSamples(handle: Long): Int
}
