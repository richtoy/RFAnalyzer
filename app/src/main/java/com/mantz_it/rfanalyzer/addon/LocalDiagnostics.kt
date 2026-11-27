package com.mantz_it.rfanalyzer.addon

import com.mantz_it.rfanalyzer.addon.rds.RdsGroupParser
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.log10
import kotlin.math.sin

/** One second of FM/RDS signal measurements from the local decoders. */
data class FmSignalMetrics(
    val timestampMs: Long = 0,
    val frequencyHz: Long = 0,
    val pilotLevelDb: Float = -120f,
    val pilotLocked: Boolean = false,
    val stereo: Boolean = false,
    val peakDeviationKhz: Float = 0f,
    val rdsInjectionKhz: Float = 0f,
    val rdsSynced: Boolean = false,
    /** Block error rate over the last 100 groups (0..1). */
    val blerRecent: Float = 1f,
    val blerSinceTune: Float = 1f,
    val blocksTotal: Long = 0,
    val blocksOk: Long = 0,
    val blocksCorrected: Long = 0,
    val blocksFailed: Long = 0,
    val groupsPerSecond: Float = 0f,
    val extras: RdsGroupParser.Extras = RdsGroupParser.Extras(),
)

/**
 * <h1>RF Analyzer - Local decoder diagnostics</h1>
 *
 * Module:      LocalDiagnostics.kt
 * Description: Live state of this fork's local decoders for the technical screens (RDS signal
 *              panel, MPX spectrum, group histogram; DAB signal and ensemble pages) and their
 *              user settings (stereo, de-emphasis). Written from the Demodulator thread, read by
 *              Compose through StateFlows. Process-wide, like the decoders themselves.
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
object LocalDiagnostics {
    // --- settings (Demodulation tab) ---
    @Volatile var stereoEnabled = true
    @Volatile var deemphasisUs = 50.0
    private val _settings = MutableStateFlow(Pair(true, 50.0))
    val settings: StateFlow<Pair<Boolean, Double>> = _settings.asStateFlow()
    fun setStereo(enabled: Boolean) { stereoEnabled = enabled; _settings.value = Pair(enabled, deemphasisUs) }
    fun setDeemphasis(us: Double) { deemphasisUs = us; _settings.value = Pair(stereoEnabled, us) }

    private val _active = MutableStateFlow(false)
    val active: StateFlow<Boolean> = _active.asStateFlow()
    fun setActive(a: Boolean) { _active.value = a }

    // --- FM / RDS ---
    private val _fm = MutableStateFlow(FmSignalMetrics())
    val fm: StateFlow<FmSignalMetrics> = _fm.asStateFlow()

    /** BLER history (last 60 s) for the sparkline, oldest first. */
    private val _blerHistory = MutableStateFlow<List<Float>>(emptyList())
    val blerHistory: StateFlow<List<Float>> = _blerHistory.asStateFlow()

    /** Group counts by type name ("0A", "2A", ...) since tune. */
    private val _groupCounts = MutableStateFlow<Map<String, Int>>(emptyMap())
    val groupCounts: StateFlow<Map<String, Int>> = _groupCounts.asStateFlow()
    private val groupCountsWork = HashMap<String, Int>()
    private var groupCountsDirty = false

    fun onRetune(frequencyHz: Long) {
        _blerHistory.value = emptyList()
        groupCountsWork.clear(); _groupCounts.value = emptyMap()
        _fm.value = FmSignalMetrics(frequencyHz = frequencyHz)
    }

    fun publishFm(m: FmSignalMetrics) {
        _fm.value = m
        _blerHistory.value = (_blerHistory.value + m.blerRecent).takeLast(60)
        if (groupCountsDirty) { _groupCounts.value = HashMap(groupCountsWork); groupCountsDirty = false }
    }

    fun onRdsGroup(blocks: IntArray) {
        val b = blocks[1]
        val name = "${b ushr 12}${if ((b ushr 11) and 1 == 0) "A" else "B"}"
        groupCountsWork[name] = (groupCountsWork[name] ?: 0) + 1
        groupCountsDirty = true
    }

    // --- MPX spectrum (0 .. fs/2), averaged, published 4x per second ---
    private const val FFT_SIZE = 2048
    private val fftBuf = FloatArray(FFT_SIZE)
    private var fftFill = 0
    private var mpxRate = 192_000
    private val avgDb = FloatArray(FFT_SIZE / 2) { -100f }
    private var lastSpectrumMs = 0L
    private val window = FloatArray(FFT_SIZE) { (0.5 - 0.5 * cos(2 * PI * it / (FFT_SIZE - 1))).toFloat() }
    private val re = DoubleArray(FFT_SIZE)
    private val im = DoubleArray(FFT_SIZE)

    /** Spectrum of the demodulated multiplex in dB, bins 0..fs/2, and the sample rate. */
    private val _mpxSpectrum = MutableStateFlow<Pair<FloatArray, Int>?>(null)
    val mpxSpectrum: StateFlow<Pair<FloatArray, Int>?> = _mpxSpectrum.asStateFlow()

    /** Set by the MPX view while it is visible, so the FFT only runs when needed. */
    @Volatile var mpxSpectrumWanted = false

    fun onMpx(mpx: FloatArray, n: Int, rate: Int) {
        if (!mpxSpectrumWanted) return
        val now = System.currentTimeMillis()
        if (now - lastSpectrumMs < 250) return
        mpxRate = rate
        var i = 0
        while (i < n && fftFill < FFT_SIZE) fftBuf[fftFill++] = mpx[i++]
        if (fftFill < FFT_SIZE) return
        fftFill = 0
        lastSpectrumMs = now
        for (k in 0 until FFT_SIZE) { re[k] = (fftBuf[k] * window[k]).toDouble(); im[k] = 0.0 }
        fft(re, im)
        val norm = 2.0 / (FFT_SIZE * 0.5)
        val out = FloatArray(FFT_SIZE / 2)
        for (k in 0 until FFT_SIZE / 2) {
            val mag = Math.hypot(re[k], im[k]) * norm
            val db = (20 * log10(mag + 1e-9)).toFloat()
            avgDb[k] = avgDb[k] * 0.6f + db * 0.4f
            out[k] = avgDb[k]
        }
        _mpxSpectrum.value = Pair(out, rate)
    }

    /** In-place radix-2 FFT. */
    private fun fft(re: DoubleArray, im: DoubleArray) {
        val n = re.size
        var j = 0
        for (i in 1 until n) {
            var bit = n shr 1
            while (j and bit != 0) { j = j xor bit; bit = bit shr 1 }
            j = j xor bit
            if (i < j) { var t = re[i]; re[i] = re[j]; re[j] = t; t = im[i]; im[i] = im[j]; im[j] = t }
        }
        var len = 2
        while (len <= n) {
            val ang = -2 * PI / len
            val wr = cos(ang); val wi = sin(ang)
            var i = 0
            while (i < n) {
                var cr = 1.0; var ci = 0.0
                for (k in 0 until len / 2) {
                    val ur = re[i + k]; val ui = im[i + k]
                    val vr = re[i + k + len / 2] * cr - im[i + k + len / 2] * ci
                    val vi = re[i + k + len / 2] * ci + im[i + k + len / 2] * cr
                    re[i + k] = ur + vr; im[i + k] = ui + vi
                    re[i + k + len / 2] = ur - vr; im[i + k + len / 2] = ui - vi
                    val ncr = cr * wr - ci * wi; ci = cr * wi + ci * wr; cr = ncr
                }
                i += len
            }
            len = len shl 1
        }
    }
}
