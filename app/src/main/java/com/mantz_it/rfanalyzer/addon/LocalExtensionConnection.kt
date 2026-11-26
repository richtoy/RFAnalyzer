package com.mantz_it.rfanalyzer.addon

import android.content.Context
import android.util.Log
import com.mantz_it.rfanalyzer.addon.fm.MpxDemodulator
import com.mantz_it.rfanalyzer.addon.rds.RdsBlockSync
import com.mantz_it.rfanalyzer.addon.rds.RdsDemodulator
import com.mantz_it.rfanalyzer.addon.rds.RdsGroupParser
import com.mantz_it.rfanalyzer.digiext.ExtensionConnectionState
import com.mantz_it.rfanalyzer.digiext.ExtensionLink
import com.mantz_it.rfanalyzer_ipc.DigitalDemodParams
import com.mantz_it.rfanalyzer_ipc.DigitalDemodulationConfiguration
import com.mantz_it.rfanalyzer_ipc.SamplePacket
import com.mantz_it.rfanalyzer_ipc.digiext.DemodStatus
import com.mantz_it.rfanalyzer_ipc.digiext.DigitalMode
import com.mantz_it.rfanalyzer_ipc.digiext.DigitalPacket
import com.mantz_it.rfanalyzer_ipc.digiext.FecStats
import com.mantz_it.rfanalyzer_ipc.digiext.RdsPacket
import com.mantz_it.rfanalyzer_ipc.digiext.StreamElement
import com.mantz_it.rfanalyzer_ipc.digiext.toBytes
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * <h1>RF Analyzer - Local extension connection</h1>
 *
 * Module:      LocalExtensionConnection.kt
 * Description: The in-process twin of DigitalExtensionConnection: runs this fork's own decoders
 *              on the Demodulator thread and reports through the same callbacks the remote RFA
 *              Digital extension uses, so the existing RDS screens, Digital tab, packet store and
 *              audio path are reused unchanged.
 *
 *              WFM (Stereo & RDS): MpxDemodulator (stereo audio) + RdsDemodulator / RdsBlockSync /
 *              RdsGroupParser (RdsPacket groups with RdsEvents, DemodStatus once a second).
 *              Signal metrics go to [LocalDiagnostics] for the add-on's technical screens.
 *
 * Push and poll are called back to back by the Demodulator for every packet, on its thread.
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
class LocalExtensionConnection(
    private val context: Context,
    private val onPacket: (DigitalPacket) -> Unit,
    private val onStreamElement: (StreamElement) -> Unit,
    private val onDemodStatus: (DemodStatus) -> Unit,
) : ExtensionLink {

    companion object {
        private const val TAG = "LocalExtensionConn"
        private const val STATUS_INTERVAL_MS = 1000L
        private const val AUDIO_FIFO = 48_000
    }

    override var bound = false
        private set
    private val _connectionState = MutableStateFlow(ExtensionConnectionState.DISCONNECTED)
    override val connectionState: StateFlow<ExtensionConnectionState> = _connectionState.asStateFlow()
    @Volatile override var lastSignOfLifeMs = 0L
        private set
    @Volatile override var lastBindMs = 0L
        private set

    @Volatile private var config = DigitalDemodulationConfiguration()

    // --- FM stereo + RDS ---
    private var mpx: MpxDemodulator? = null
    private var rdsDemod: RdsDemodulator? = null
    private var rdsSync: RdsBlockSync? = null
    private val rdsParser = RdsGroupParser()
    private var currentFrequency = 0L
    private var lastStatusMs = 0L
    private var lastGroupsTotal = 0L

    // audio FIFO (Demodulator thread only)
    private val fifoL = FloatArray(AUDIO_FIFO)
    private val fifoR = FloatArray(AUDIO_FIFO)
    private var fifoRead = 0
    private var fifoWrite = 0
    private var fifoCount = 0
    private var scratchL = FloatArray(8192)
    private var scratchR = FloatArray(8192)

    override fun bind(): Boolean {
        if (bound) return false
        bound = true
        lastBindMs = System.currentTimeMillis()
        lastSignOfLifeMs = lastBindMs
        resetDecoders()
        _connectionState.value = ExtensionConnectionState.CONNECTED
        LocalDiagnostics.setActive(true)
        Log.i(TAG, "bind: local decoders started")
        return true
    }

    override fun unbind() {
        if (!bound) return
        bound = false
        _connectionState.value = ExtensionConnectionState.DISCONNECTED
        LocalDiagnostics.setActive(false)
        Log.i(TAG, "unbind: local decoders stopped")
    }

    override suspend fun unbindAndAwaitStopped(timeoutMs: Long) = unbind()
    override fun dropUnresponsive() = unbind()
    override fun markReconnecting() { _connectionState.value = ExtensionConnectionState.RECONNECTING }
    override fun markUnavailable() { _connectionState.value = ExtensionConnectionState.UNAVAILABLE }
    override fun markIdle() { if (!bound) _connectionState.value = ExtensionConnectionState.DISCONNECTED }
    override fun canResolveDigitalExtensionService(context: Context) = true
    override fun hasIpcPermission(context: Context) = true

    override fun setDigitalDemodulationConfiguration(configuration: DigitalDemodulationConfiguration) {
        config = configuration
    }

    @Synchronized
    private fun resetDecoders() {
        mpx = null; rdsDemod = null; rdsSync = null
        rdsParser.reset()
        fifoRead = 0; fifoWrite = 0; fifoCount = 0
        lastGroupsTotal = 0
    }

    override fun pushInputSamples(inputSamples: SamplePacket): Boolean {
        if (_connectionState.value != ExtensionConnectionState.CONNECTED) return false
        lastSignOfLifeMs = System.currentTimeMillis()
        return when (config.demodulationParams) {
            is DigitalDemodParams.Wfm -> { processWfm(inputSamples); true }
            else -> true
        }
    }

    private fun processWfm(p: SamplePacket) {
        val n = p.size()
        if (n == 0) return
        val rate = p.sampleRate
        var m = mpx
        if (m == null) {
            m = MpxDemodulator(rate, maxOf(n, 1 shl 15))
            mpx = m
            val sync = RdsBlockSync { blocks -> onRdsGroup(blocks) }
            rdsSync = sync
            rdsDemod = RdsDemodulator(rate) { bit -> sync.pushBit(bit) }
        }
        if (kotlin.math.abs(p.frequency - currentFrequency) > 20_000) {
            // retuned: start over (the new station has another PI, phase and timing)
            currentFrequency = p.frequency
            m.reset(); rdsDemod?.reset(); rdsSync?.reset(); rdsSync?.resetStats(); rdsParser.reset()
            LocalDiagnostics.onRetune(p.frequency)
            lastGroupsTotal = 0
        }
        m.stereoEnabled = LocalDiagnostics.stereoEnabled
        m.deemphasisUs = LocalDiagnostics.deemphasisUs
        val outMax = n / 4 + 4
        if (scratchL.size < outMax) { scratchL = FloatArray(outMax); scratchR = FloatArray(outMax) }
        val k = m.process(p.re(), p.im(), n, scratchL, scratchR)
        writeFifo(scratchL, scratchR, k)
        rdsDemod?.process(m.mpx(), m.mpxSize)
        LocalDiagnostics.onMpx(m.mpx(), m.mpxSize, rate)

        val now = System.currentTimeMillis()
        if (now - lastStatusMs >= STATUS_INTERVAL_MS) {
            val dt = (now - lastStatusMs).coerceAtLeast(1)
            lastStatusMs = now
            val sync = rdsSync ?: return
            val groupsPerSec = (sync.groupsTotal - lastGroupsTotal) * 1000f / dt
            lastGroupsTotal = sync.groupsTotal
            val demod = rdsDemod
            LocalDiagnostics.publishFm(
                FmSignalMetrics(
                    timestampMs = now,
                    frequencyHz = p.frequency,
                    pilotLevelDb = m.pilotLevelDb(),
                    pilotLocked = m.pilotLocked,
                    stereo = m.isStereo,
                    peakDeviationKhz = m.takePeakDeviationHz() / 1000f,
                    rdsInjectionKhz = (demod?.carrierLevel ?: 0f) * 75f,
                    rdsSynced = sync.synced,
                    blerRecent = sync.blerRecent(),
                    blerSinceTune = if (sync.blocksTotal > 0) sync.blocksFailed.toFloat() / sync.blocksTotal else 1f,
                    blocksTotal = sync.blocksTotal,
                    blocksOk = sync.blocksOk,
                    blocksCorrected = sync.blocksCorrected,
                    blocksFailed = sync.blocksFailed,
                    groupsPerSecond = groupsPerSec,
                    extras = rdsParser.extras,
                )
            )
            onDemodStatus(
                DemodStatus(
                    mode = DigitalMode.RDS,
                    locked = sync.synced,
                    signalLevelDb = m.pilotLevelDb(),
                    fecStats = FecStats(
                        attempts = sync.blocksTotal.toInt(),
                        passed = (sync.blocksOk + sync.blocksCorrected).toInt(),
                        corrected = sync.blocksCorrected.toInt(),
                        failed = sync.blocksFailed.toInt(),
                    ),
                )
            )
        }
    }

    private fun onRdsGroup(blocks: IntArray) {
        val events = try { rdsParser.parse(blocks) } catch (e: Exception) {
            Log.w(TAG, "onRdsGroup: parser failed: ${e.message}"); emptyList()
        }
        val raw = ByteArray(8)
        for (i in 0..3) { raw[2 * i] = (blocks[i] ushr 8).toByte(); raw[2 * i + 1] = blocks[i].toByte() }
        LocalDiagnostics.onRdsGroup(blocks)
        onPacket(RdsPacket(timestampMs = System.currentTimeMillis(), frequencyHz = currentFrequency, raw = raw.toBytes(), events = events))
    }

    private fun writeFifo(l: FloatArray, r: FloatArray, n: Int) {
        for (i in 0 until n) {
            if (fifoCount == AUDIO_FIFO) { fifoRead = (fifoRead + 1) % AUDIO_FIFO; fifoCount-- }
            fifoL[fifoWrite] = l[i]; fifoR[fifoWrite] = r[i]
            fifoWrite = (fifoWrite + 1) % AUDIO_FIFO
            fifoCount++
        }
    }

    override fun pollOutputSamples(outputSamples: SamplePacket): Boolean {
        if (_connectionState.value != ExtensionConnectionState.CONNECTED) return false
        if (fifoCount == 0) return false
        val outL = outputSamples.re()
        val outR = outputSamples.im()
        val k = minOf(fifoCount, outputSamples.capacity())
        for (i in 0 until k) {
            outL[i] = fifoL[fifoRead]; outR[i] = fifoR[fifoRead]
            fifoRead = (fifoRead + 1) % AUDIO_FIFO
        }
        fifoCount -= k
        outputSamples.setSize(k)
        outputSamples.sampleRate = MpxDemodulator.AUDIO_RATE
        outputSamples.frequency = currentFrequency
        outputSamples.timestamp = System.currentTimeMillis()
        return true
    }
}
