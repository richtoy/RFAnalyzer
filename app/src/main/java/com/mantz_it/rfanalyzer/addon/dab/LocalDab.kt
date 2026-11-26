package com.mantz_it.rfanalyzer.addon.dab

import android.content.Context
import android.content.SharedPreferences
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

/**
 * <h1>RF Analyzer - local DAB receiver state</h1>
 *
 * Module:      LocalDab.kt
 * Description: The bridge between the DAB screens and [DabReceiver] (which runs on the
 *              Demodulator thread): the live [DabState], the newest slide, the constellation, the
 *              user's requests (service selection) and the Band III scan. Process-wide, like the
 *              receiver. MainViewModel installs [tuner] so the screens can tune to a channel.
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
object LocalDab {
    private const val TAG = "LocalDab"
    private const val PREFS = "local_dab"

    private val _state = MutableStateFlow(DabState())
    val state: StateFlow<DabState> = _state.asStateFlow()

    /** The newest MOT slide's bytes (JPEG/PNG); the state's slideSerial says when it changed. */
    @Volatile var slide: ByteArray? = null
        internal set

    /** Constellation points (interleaved I/Q, unit magnitude) of the newest OFDM symbol batch. */
    private val _constellation = MutableStateFlow(FloatArray(0))
    val constellation: StateFlow<FloatArray> = _constellation.asStateFlow()
    @Volatile var constellationWanted = false

    private val _scan = MutableStateFlow(DabScanState())
    val scan: StateFlow<DabScanState> = _scan.asStateFlow()

    /** Tunes the receiver to a DAB channel frequency (installed by MainViewModel). */
    @Volatile var tuner: ((label: String, frequencyHz: Long) -> Unit)? = null

    // --- requests to the receiver (read on the Demodulator thread) ---
    @Volatile internal var requestedSid: Long = -1
    @Volatile internal var requestSerial = 0

    private var prefs: SharedPreferences? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var scanJob: Job? = null

    fun init(context: Context) {
        if (prefs == null) prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        if (_scan.value.found.isEmpty()) _scan.value = _scan.value.copy(found = loadEnsembles())
    }

    internal fun publish(s: DabState) { _state.value = s }
    internal fun publishConstellation(points: FloatArray) { _constellation.value = points }

    /** Plays [sid]; remembered as this ensemble's choice. */
    fun selectService(sid: Long) {
        requestedSid = sid
        requestSerial++
        val s = _state.value
        prefs?.edit()?.putLong(prefKey(s), sid)?.apply()
    }

    fun tuneToChannel(channel: DabChannel) {
        tuner?.invoke("DAB ${channel.name}", channel.frequencyHz) ?: Log.w(TAG, "tuneToChannel: no tuner installed")
    }

    /** The service last chosen in the ensemble ([s]'s EId, else its channel), or -1. */
    internal fun rememberedSid(s: DabState): Long = prefs?.getLong(prefKey(s), -1L) ?: -1L
    private fun prefKey(s: DabState) = if (s.ensembleId >= 0) "sid_e${s.ensembleId}" else "sid_f${s.frequencyHz / 1000}"

    // ------------------------------------------------------------------ scan

    /** Tunes through Band III and lists the ensembles found; ends on the first one found. */
    fun startScan() {
        if (scanJob?.isActive == true) return
        val channels = DabChannels.scanList
        scanJob = scope.launch {
            val found = ArrayList<DabEnsembleEntry>()
            _scan.value = DabScanState(running = true, channelCount = channels.size, found = emptyList())
            for ((i, ch) in channels.withIndex()) {
                _scan.value = _scan.value.copy(channelIndex = i, currentChannel = ch.name)
                tuneToChannel(ch)
                // wait for the receiver to be on the channel
                var waited = 0
                while (waited < 3000 && _state.value.frequencyHz != ch.frequencyHz) { delay(100); waited += 100 }
                if (_state.value.frequencyHz != ch.frequencyHz) continue // analyzer not running?
                // time sync within ~2.5 s, else nothing here
                waited = 0
                while (waited < 2500 && !_state.value.sync) { delay(100); waited += 100 }
                if (!_state.value.sync) continue
                // the ensemble label and the service list (FIG 1) take a few seconds
                waited = 0
                while (waited < 8000 && (_state.value.ensembleLabel.isEmpty() || _state.value.services.isEmpty())) { delay(200); waited += 200 }
                if (_state.value.ensembleLabel.isEmpty() && _state.value.services.isEmpty()) continue
                delay(2500) // collect the rest of the services
                val s = _state.value
                found += DabEnsembleEntry(ch.name, ch.frequencyHz, s.ensembleLabel.ifEmpty { "(unnamed)" }, s.ensembleId,
                    s.services.map { it.label })
                _scan.value = _scan.value.copy(found = found.toList())
            }
            _scan.value = _scan.value.copy(running = false, found = found.toList().ifEmpty { _scan.value.found })
            if (found.isNotEmpty()) {
                saveEnsembles(found)
                DabChannels.byName(found.first().channel)?.let { tuneToChannel(it) }
            }
        }
    }

    fun stopScan() {
        scanJob?.cancel()
        _scan.value = _scan.value.copy(running = false)
    }

    private fun saveEnsembles(list: List<DabEnsembleEntry>) {
        val a = JSONArray()
        list.forEach { e ->
            a.put(JSONObject().put("ch", e.channel).put("f", e.frequencyHz).put("l", e.label).put("id", e.ensembleId)
                .put("s", JSONArray(e.services)))
        }
        prefs?.edit()?.putString("ensembles", a.toString())?.apply()
    }

    private fun loadEnsembles(): List<DabEnsembleEntry> = try {
        val a = JSONArray(prefs?.getString("ensembles", "[]") ?: "[]")
        (0 until a.length()).map { i ->
            val o = a.getJSONObject(i)
            val s = o.optJSONArray("s") ?: JSONArray()
            DabEnsembleEntry(o.getString("ch"), o.getLong("f"), o.getString("l"), o.optLong("id", -1),
                (0 until s.length()).map { s.getString(it) })
        }
    } catch (e: Exception) {
        emptyList()
    }
}
