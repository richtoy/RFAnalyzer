package com.mantz_it.rfanalyzer.addon.fm

import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.exp
import kotlin.math.log10
import kotlin.math.max
import kotlin.math.sqrt

/**
 * <h1>RF Analyzer - FM multiplex (stereo) demodulator</h1>
 *
 * Module:      MpxDemodulator.kt
 * Description: Broadcast FM at 192 kHz complex baseband to stereo audio at 48 kHz:
 *              FM discriminator -> multiplex signal (MPX, 0..96 kHz), 19 kHz pilot PLL,
 *              38 kHz L-R decode locked to the pilot, 15 kHz audio low-pass, de-emphasis.
 *              The MPX of the last call stays available ([mpx]) for the RDS demodulator and
 *              the MPX spectrum view; pilot level/lock and peak deviation are measured as well.
 *
 * MPX units: 1.0 = 75 kHz deviation.
 *
 * Part of Richard's personal RF Analyzer fork (local decoders replacing the closed extension).
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
class MpxDemodulator(
    private val inputRate: Int = 192_000,
    maxPacketSize: Int = 1 shl 16,
) {
    companion object {
        const val AUDIO_RATE = 48_000
        const val PILOT_HZ = 19_000.0
        const val MAX_DEVIATION_HZ = 75_000.0
        private const val PILOT_LOCK_MIN_LEVEL = 0.02f   // 1.5 kHz deviation
    }

    /** Decode L-R when a pilot is present; false: always mono. */
    @Volatile var stereoEnabled = true
    /** De-emphasis time constant in microseconds (50 Europe, 75 Americas, 0 off). */
    @Volatile var deemphasisUs = 50.0
        set(value) { field = value; updateDeemphasis() }

    private val decimation = inputRate / AUDIO_RATE

    // MPX of the last process() call
    private var mpxBuf = FloatArray(maxPacketSize)
    var mpxSize = 0
        private set
    fun mpx(): FloatArray = mpxBuf

    // discriminator state
    private var lastRe = 0f
    private var lastIm = 0f
    private val discGain = (inputRate / (2 * PI * MAX_DEVIATION_HZ)).toFloat()

    // pilot PLL (phase in cycles)
    private var pllPhase = 0.0
    private val pllNominal = PILOT_HZ / inputRate
    private var pllFreq = pllNominal
    private val pllMaxOffset = 30.0 / inputRate
    private val pllKp: Double
    private val pllKi: Double
    private val iqAlpha = (1 - exp(-2 * PI * 150.0 / inputRate)).toFloat()
    private var pi1 = 0f; private var pq1 = 0f; private var pi2 = 0f; private var pq2 = 0f
    private var errAvg = 1.0
    private val errAlpha = 1 - exp(-2 * PI * 2.0 / inputRate)

    // audio filters (192 kHz -> 48 kHz)
    private val audioTaps = DspUtil.lowPassTaps(191, inputRate.toDouble(), 16_000.0)
    private val sumFir = DecimatingRealFir(audioTaps, decimation)
    private val diffFir = DecimatingRealFir(audioTaps, decimation)

    // de-emphasis at 48 kHz
    private var deAlpha = 1f
    private var deL = 0f
    private var deR = 0f

    // metrics
    /** Pilot amplitude in MPX units (≈0.08-0.10 on a stereo station). */
    @Volatile var pilotLevel = 0f
        private set
    @Volatile var pilotLocked = false
        private set
    private var peakAbsMpx = 0f
    /** Peak deviation over the last measurement window (Hz), see [takePeakDeviationHz]. */
    fun takePeakDeviationHz(): Float { val v = peakAbsMpx * MAX_DEVIATION_HZ.toFloat(); peakAbsMpx = 0f; return v }
    /** Current pilot PLL phase (cycles) and averaged phase error, for diagnostics/tests. */
    val pllPhaseCycles: Double get() = pllPhase
    val pllErrorAvg: Double get() = errAvg
    val pllFreqHz: Double get() = pllFreq * inputRate
    /** True while L-R is actually being decoded. */
    val isStereo: Boolean get() = stereoEnabled && pilotLocked

    init {
        val wn = 2 * PI * 4.0 / inputRate   // ~4 Hz loop
        pllKp = 2 * 0.707 * wn / (2 * PI) * 2 * PI
        pllKi = wn * wn
        updateDeemphasis()
    }

    private fun updateDeemphasis() {
        deAlpha = if (deemphasisUs <= 0) 1f else (1 - exp(-1.0 / (AUDIO_RATE * deemphasisUs * 1e-6))).toFloat()
    }

    fun reset() {
        lastRe = 0f; lastIm = 0f
        pllPhase = 0.0; pllFreq = pllNominal
        pi1 = 0f; pq1 = 0f; pi2 = 0f; pq2 = 0f; errAvg = 1.0
        sumFir.reset(); diffFir.reset()
        deL = 0f; deR = 0f
        pilotLevel = 0f; pilotLocked = false; peakAbsMpx = 0f
    }

    /**
     * Demodulates [n] complex samples. Writes stereo audio at 48 kHz to [outL]/[outR] and returns the
     * number of audio samples written (n / 4). The MPX is left in [mpx] (size [mpxSize]).
     */
    fun process(re: FloatArray, im: FloatArray, n: Int, outL: FloatArray, outR: FloatArray): Int {
        if (mpxBuf.size < n) mpxBuf = FloatArray(n)
        val mpx = mpxBuf
        var pRe = lastRe
        var pIm = lastIm
        var peak = peakAbsMpx
        for (i in 0 until n) {
            val r = re[i]; val q = im[i]
            // x[n] * conj(x[n-1])
            val dr = r * pRe + q * pIm
            val di = q * pRe - r * pIm
            val v = discGain * atan2(di, dr)
            mpx[i] = v
            val a = abs(v)
            if (a > peak) peak = a
            pRe = r; pIm = q
        }
        lastRe = pRe; lastIm = pIm
        peakAbsMpx = peak
        mpxSize = n

        val stereoWanted = stereoEnabled
        var outCount = 0
        var phase = pllPhase
        var freq = pllFreq
        val alpha = iqAlpha
        for (i in 0 until n) {
            val x = mpx[i]
            val c = SinCosTable.cos(phase)
            val s = SinCosTable.sin(phase)
            // pilot = A·cos(θ) => 38 kHz subcarrier sin(2ωt) = -sin(2θ) = -2·sin θ·cos θ
            val sub = -4f * s * c
            // mix by e^{-jθ}, two one-pole low-passes
            pi1 += alpha * (x * c - pi1); pq1 += alpha * (-x * s - pq1)
            pi2 += alpha * (pi1 - pi2); pq2 += alpha * (pq1 - pq2)
            val errCycles = atan2(pq2, pi2) / (2 * PI)
            errAvg += errAlpha * (abs(errCycles) - errAvg)
            freq += pllKi * errCycles
            if (freq > pllNominal + pllMaxOffset) freq = pllNominal + pllMaxOffset
            if (freq < pllNominal - pllMaxOffset) freq = pllNominal - pllMaxOffset
            phase += freq + pllKp * errCycles
            if (phase >= 1.0) phase -= 1.0

            val sumReady = sumFir.push(x)
            val diffReady = diffFir.push(x * sub)
            if (sumReady && diffReady) {
                val sum = sumFir.output
                val diff = if (stereoWanted && pilotLocked) diffFir.output else 0f
                var l = sum + diff
                var rr = sum - diff
                deL += deAlpha * (l - deL); l = deL
                deR += deAlpha * (rr - deR); rr = deR
                outL[outCount] = l
                outR[outCount] = rr
                outCount++
            }
        }
        pllPhase = phase
        pllFreq = freq
        val level = 2 * sqrt(pi2 * pi2 + pq2 * pq2)
        pilotLevel = level
        // hysteresis on the averaged phase error
        pilotLocked = level > PILOT_LOCK_MIN_LEVEL && errAvg < (if (pilotLocked) 0.12 else 0.06)
        return outCount
    }

    /** Pilot level in dB relative to full deviation (≈ -20 dB for 10 % injection). */
    fun pilotLevelDb(): Float = 20 * log10(max(pilotLevel, 1e-6f))
}
