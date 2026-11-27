package com.mantz_it.rfanalyzer.digiext

import android.content.Context
import com.mantz_it.rfanalyzer.BuildConfig
import com.mantz_it.rfanalyzer.ui.tabs.DemodulationMode
import com.mantz_it.rfanalyzer_ipc.DigitalDemodulationConfiguration
import com.mantz_it.rfanalyzer_ipc.SamplePacket
import kotlinx.coroutines.flow.StateFlow

/**
 * <h1>RF Analyzer - Extension link</h1>
 *
 * Module:      ExtensionLink.kt
 * Description: What AnalyzerService needs from a digital decoder: the remote RFA Digital
 *              extension ([DigitalExtensionConnection], over Binder) or the in-process local
 *              decoders of this fork ([com.mantz_it.rfanalyzer.addon.LocalExtensionConnection]).
 *              Both deliver to the same onPacket / onStreamElement / onDemodStatus callbacks, so
 *              the Digital tab, packet store and presenters work unchanged.
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
interface ExtensionLink {
    val bound: Boolean
    val connectionState: StateFlow<ExtensionConnectionState>
    val lastSignOfLifeMs: Long
    val lastBindMs: Long
    fun bind(): Boolean
    fun unbind()
    suspend fun unbindAndAwaitStopped(timeoutMs: Long = 5_000L)
    fun dropUnresponsive()
    fun markReconnecting()
    fun markUnavailable()
    fun markIdle()
    fun pushInputSamples(inputSamples: SamplePacket): Boolean
    fun pollOutputSamples(outputSamples: SamplePacket): Boolean
    fun setDigitalDemodulationConfiguration(configuration: DigitalDemodulationConfiguration)
    fun canResolveDigitalExtensionService(context: Context): Boolean
    fun hasIpcPermission(context: Context): Boolean
}

/**
 * The local (in-process) decoders of this fork. In the FOSS build they replace the closed
 * extension: FM stereo + RDS (WFM with "Stereo & RDS") and DAB/DAB+.
 */
object LocalDecoders {
    val enabled: Boolean = BuildConfig.IS_FOSS

    /** Digital demodulation modes the local decoders handle (WFM stereo/RDS is not a digital mode). */
    fun decodes(mode: DemodulationMode): Boolean = when (mode.name) {
        "DAB" -> true
        else -> false
    }
}
