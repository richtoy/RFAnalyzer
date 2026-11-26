package com.mantz_it.rfanalyzer.addon.fm

import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.sin

/**
 * <h1>RF Analyzer - Add-on DSP helpers</h1>
 *
 * Module:      DspUtil.kt
 * Description: Small, allocation-free building blocks for the local decoders (FM stereo, RDS):
 *              windowed-sinc FIR design, a decimating real FIR, a decimating complex FIR and a
 *              sine/cosine table. Pure Kotlin, no Android dependencies, so they can be unit-tested
 *              on the JVM.
 *
 * Part of Richard's personal RF Analyzer fork (local decoders replacing the closed extension).
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
object DspUtil {
    /** Low-pass windowed-sinc (Blackman) taps with unity DC gain. [cutoffHz] is the -6 dB point. */
    fun lowPassTaps(numTaps: Int, sampleRate: Double, cutoffHz: Double): FloatArray {
        val taps = FloatArray(numTaps)
        val fc = cutoffHz / sampleRate
        val m = numTaps - 1
        var sum = 0.0
        for (i in 0 until numTaps) {
            val x = i - m / 2.0
            val sinc = if (x == 0.0) 2 * PI * fc else sin(2 * PI * fc * x) / x
            val w = 0.42 - 0.5 * cos(2 * PI * i / m) + 0.08 * cos(4 * PI * i / m)
            val v = sinc * w
            taps[i] = v.toFloat()
            sum += v
        }
        for (i in 0 until numTaps) taps[i] = (taps[i] / sum).toFloat()
        return taps
    }
}

/** Fast sine/cosine for phases given in cycles (0..1). */
object SinCosTable {
    private const val BITS = 14
    const val SIZE = 1 shl BITS
    private const val MASK = SIZE - 1
    private val sinTable = FloatArray(SIZE) { kotlin.math.sin(2 * PI * it / SIZE).toFloat() }

    /** sin(2π·cycles) */
    fun sin(cycles: Double): Float = sinTable[((cycles - Math.floor(cycles)) * SIZE).toInt() and MASK]
    /** cos(2π·cycles) */
    fun cos(cycles: Double): Float = sinTable[(((cycles - Math.floor(cycles)) * SIZE).toInt() + SIZE / 4) and MASK]
}

/**
 * Decimating FIR for a real signal: [push] one input sample at a time, returns true when an
 * output sample is ready in [output] (every [decimation]-th input). Only the kept outputs are computed.
 */
class DecimatingRealFir(private val taps: FloatArray, private val decimation: Int) {
    private val n = taps.size
    // Doubled history so a contiguous window is always available.
    private val history = FloatArray(2 * n)
    private var pos = 0
    private var phase = 0
    var output = 0f
        private set

    fun push(x: Float): Boolean {
        history[pos] = x
        history[pos + n] = x
        pos++
        if (pos == n) pos = 0
        phase++
        if (phase < decimation) return false
        phase = 0
        // history[pos .. pos+n-1] holds the last n samples, oldest first
        var acc = 0f
        var j = pos
        for (k in n - 1 downTo 0) {
            acc += history[j] * taps[k]
            j++
        }
        output = acc
        return true
    }

    fun reset() {
        history.fill(0f); pos = 0; phase = 0; output = 0f
    }
}

/** Decimating FIR for a complex signal (same taps on I and Q). */
class DecimatingComplexFir(private val taps: FloatArray, private val decimation: Int) {
    private val n = taps.size
    private val histRe = FloatArray(2 * n)
    private val histIm = FloatArray(2 * n)
    private var pos = 0
    private var phase = 0
    var outRe = 0f
        private set
    var outIm = 0f
        private set

    fun push(re: Float, im: Float): Boolean {
        histRe[pos] = re; histRe[pos + n] = re
        histIm[pos] = im; histIm[pos + n] = im
        pos++
        if (pos == n) pos = 0
        phase++
        if (phase < decimation) return false
        phase = 0
        var accRe = 0f
        var accIm = 0f
        var j = pos
        for (k in n - 1 downTo 0) {
            val t = taps[k]
            accRe += histRe[j] * t
            accIm += histIm[j] * t
            j++
        }
        outRe = accRe
        outIm = accIm
        return true
    }

    fun reset() {
        histRe.fill(0f); histIm.fill(0f); pos = 0; phase = 0; outRe = 0f; outIm = 0f
    }
}
