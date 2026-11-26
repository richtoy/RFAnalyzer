package com.mantz_it.rfanalyzer.addon.dab

import android.util.Log
import com.mantz_it.nativedab.NativeDab

/**
 * <h1>RF Analyzer - DAB receiver</h1>
 *
 * Module:      DabReceiver.kt
 * Description: Drives the native DAB/DAB+ receiver (nativedab) from the Demodulator thread: pushes
 *              the 2.048 Msps channel samples, folds its events into [LocalDab]'s state, plays the
 *              selected service and paces its audio by the input sample clock (DAB delivers audio in
 *              bursts of 24-120 ms frames). Restarts the receiver when the channel moves.
 *
 * Not thread-safe: all calls on the Demodulator thread, except [release] (synchronized).
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
class DabReceiver {
    companion object {
        private const val TAG = "DabReceiver"
        const val INPUT_RATE = 2_048_000
        private const val FIFO_FRAMES = 48_000 * 2         // 2 s at 48 kHz
        private const val PREBUFFER_MS = 300
        private const val STATUS_INTERVAL_MS = 500L
    }

    private var handle = 0L
    private var state = DabState()
    private var lastStatusMs = 0L
    private var tuneTimeMs = 0L
    private var lastDescribeMs = 0L
    private var handledRequest = -1
    private var pendingSid = -1L
    private var lastSelectAttemptMs = 0L
    private var firstServiceMs = 0L
    private var inputCount = 0L
    private var inputPower = 0.0
    private var inputPowerN = 0L
    private var gaps = 0
    private var lastPacketEndMs = 0L

    // audio
    private val pcm = ShortArray(16_384)
    private val fifoL = FloatArray(FIFO_FRAMES)
    private val fifoR = FloatArray(FIFO_FRAMES)
    private var fifoRead = 0
    private var fifoWrite = 0
    private var fifoCount = 0
    private var playing = false
    private var audioRate = 48_000
    private var owedFrames = 0.0
    private val constellation = FloatArray(2 * 1536)

    val outputRate: Int get() = audioRate

    @Synchronized
    private fun ensureStarted(): Boolean {
        if (handle != 0L) return true
        return try {
            handle = NativeDab.nativeCreate()
            NativeDab.nativeStart(handle)
            Log.i(TAG, "native receiver started")
            true
        } catch (e: Throwable) {
            Log.e(TAG, "native receiver unavailable: ${e.message}")
            handle = 0L
            false
        }
    }

    @Synchronized
    fun release() {
        if (handle == 0L) return
        val h = handle
        handle = 0L
        try {
            NativeDab.nativeStop(h)
            NativeDab.nativeDestroy(h)
        } catch (e: Throwable) {
            Log.w(TAG, "release: ${e.message}")
        }
        state = DabState()
        LocalDab.publish(state)
        clearAudio()
        Log.i(TAG, "native receiver released")
    }

    private fun clearAudio() {
        fifoRead = 0; fifoWrite = 0; fifoCount = 0; playing = false; owedFrames = 0.0
    }

    /** Feeds one packet of channel samples (centred on [frequencyHz]); [rate] must be 2.048 Msps. */
    @Synchronized
    fun process(re: FloatArray, im: FloatArray, n: Int, rate: Int, frequencyHz: Long) {
        if (n <= 0 || rate != INPUT_RATE) return
        if (!ensureStarted()) return
        val now = System.currentTimeMillis()
        if (kotlin.math.abs(frequencyHz - state.frequencyHz) > 100_000 || !state.active) {
            if (state.active) NativeDab.nativeReset(handle)
            state = DabState(active = true, frequencyHz = frequencyHz)
            tuneTimeMs = now
            pendingSid = -1; firstServiceMs = 0; handledRequest = LocalDab.requestSerial
            LocalDab.slide = null
            clearAudio()
            LocalDab.publish(state)
        } else if (frequencyHz != state.frequencyHz) {
            state = state.copy(frequencyHz = frequencyHz)
        }

        inputCount += n
        var p = 0.0
        var i = 0
        while (i < n) { p += re[i] * re[i] + im[i] * im[i]; i += 16 }
        inputPower += p; inputPowerN += (n + 15) / 16
        val dropped = NativeDab.nativePush(handle, re, im, n)
        if (dropped > 0) Log.d(TAG, "process: receiver behind, dropped $dropped samples")

        // events
        var changed = false
        NativeDab.nativePollEvents(handle)?.let { bytes ->
            state = DabEvents.apply(state, String(bytes, Charsets.UTF_8), now)
            changed = true
        }
        NativeDab.nativeTakeSlide(handle)?.let { bytes ->
            LocalDab.slide = bytes
            state = state.copy(slideSerial = state.slideSerial + 1)
            changed = true
        }

        handleSelection(now)
        pullAudio()
        owedFrames = minOf(owedFrames + n.toDouble() * audioRate / INPUT_RATE, audioRate.toDouble())

        if (now - lastStatusMs >= STATUS_INTERVAL_MS) {
            if (now - lastDescribeMs >= 1000) {
                lastDescribeMs = now
                for (svc in state.services) if (!svc.described) NativeDab.nativeDescribeService(handle, svc.sid.toInt())
            }
            if (LocalDab.constellationWanted) {
                val k = NativeDab.nativePollConstellation(handle, constellation)
                if (k > 0) LocalDab.publishConstellation(constellation.copyOf(2 * k))
            }
            val dt = (now - lastStatusMs).coerceAtLeast(1)
            val rateNow = (inputCount * 1000 / dt).toInt()
            val level = if (inputPowerN > 0) (10 * kotlin.math.log10(inputPower / inputPowerN + 1e-12)).toFloat() else -120f
            if (!state.sync) Log.d(TAG, "status: no sync, input $rateNow S/s, level %.1f dBFS, dropped ${NativeDab.nativeDroppedSamples(handle)}".format(level))
            inputCount = 0; inputPower = 0.0; inputPowerN = 0
            state = state.copy(
                inputRate = rateNow,
                inputLevelDb = level,
                audioRate = audioRate,
                audioBufferedMs = fifoCount * 1000 / audioRate.coerceAtLeast(1),
                droppedSamples = NativeDab.nativeDroppedSamples(handle).toLong() and 0xFFFFFFFFL,
                secondsSinceTune = ((now - tuneTimeMs) / 1000).toInt(),
            )
            lastStatusMs = now
            changed = true
        }
        if (changed) LocalDab.publish(state)
    }

    /** User requests first; else the ensemble's remembered service; else the first audio service after a few seconds. */
    private fun handleSelection(now: Long) {
        if (LocalDab.requestSerial != handledRequest) {
            handledRequest = LocalDab.requestSerial
            pendingSid = LocalDab.requestedSid
            lastSelectAttemptMs = 0
        }
        if (state.services.isNotEmpty() && firstServiceMs == 0L) firstServiceMs = now
        if (pendingSid < 0 && state.selectedSid < 0 && firstServiceMs > 0) {
            val remembered = LocalDab.rememberedSid(state)
            pendingSid = when {
                remembered >= 0 && state.services.any { it.sid == remembered } -> remembered
                now - firstServiceMs > 4000 -> state.services.firstOrNull { it.isAudio == true }?.sid ?: -1
                else -> -1
            }
        }
        if (pendingSid >= 0 && pendingSid != state.selectedSid && now - lastSelectAttemptMs >= 1000) {
            lastSelectAttemptMs = now
            if (NativeDab.nativeSelectServiceBySid(handle, pendingSid.toInt())) {
                Log.i(TAG, "selected service %04X".format(pendingSid))
                clearAudio()
                state = state.copy(selectedSid = pendingSid)
                pendingSid = -1
            }
        } else if (pendingSid == state.selectedSid) {
            pendingSid = -1
        }
    }

    private fun pullAudio() {
        while (true) {
            val k = NativeDab.nativePollAudio(handle, pcm)
            if (k <= 0) break
            val rate = NativeDab.nativeAudioRate(handle)
            if (rate > 0 && rate != audioRate) { audioRate = rate; clearAudio() }
            var i = 0
            while (i + 1 < k) {
                if (fifoCount == FIFO_FRAMES) { fifoRead = (fifoRead + 1) % FIFO_FRAMES; fifoCount-- }
                fifoL[fifoWrite] = pcm[i] / 32768f
                fifoR[fifoWrite] = pcm[i + 1] / 32768f
                fifoWrite = (fifoWrite + 1) % FIFO_FRAMES
                fifoCount++
                i += 2
            }
            if (k < pcm.size) break
        }
    }

    /**
     * Writes the audio due for the input consumed so far (pacing by the sample clock), nudged
     * towards [PREBUFFER_MS] of buffered audio. Returns the number of frames written.
     */
    @Synchronized
    fun pollAudio(outL: FloatArray, outR: FloatArray, capacity: Int): Int {
        val target = audioRate * PREBUFFER_MS / 1000
        if (!playing) {
            if (fifoCount < target) { owedFrames = 0.0; return 0 }
            playing = true
            owedFrames = 0.0
        }
        if (fifoCount == 0) { playing = false; return 0 }
        val nudge = when {
            fifoCount > 2 * target -> 1.02
            fifoCount < target / 2 -> 0.98
            else -> 1.0
        }
        val due = (owedFrames * nudge).toInt()
        val k = minOf(due, fifoCount, capacity)
        if (k <= 0) return 0
        owedFrames -= k / nudge
        for (i in 0 until k) {
            outL[i] = fifoL[fifoRead]; outR[i] = fifoR[fifoRead]
            fifoRead = (fifoRead + 1) % FIFO_FRAMES
        }
        fifoCount -= k
        return k
    }
}
