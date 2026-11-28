package com.mantz_it.rfanalyzer.addon.fm

import android.content.Context
import android.content.SharedPreferences
import android.util.Log
import com.mantz_it.rfanalyzer.addon.LocalDiagnostics
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlin.math.abs

/**
 * <h1>RF Analyzer - FM band scan and presets</h1>
 *
 * Module:      LocalFmScan.kt
 * Description: Scans the FM band and keeps the stations found as presets. Process-wide, like the
 *              local decoders. MainViewModel installs [radio] (how to tune and what the source is);
 *              FftProcessor hands spectrum frames to [onFftFrame] while [frameWanted] is set, and
 *              LocalExtensionConnection hands every RDS group to [onRdsGroup].
 *
 *              How a scan runs (a coroutine on the main thread, polling every 100 ms):
 *              1. Survey: the band is wider than the SDR's bandwidth, so the source is stepped
 *                 through overlapping windows ([FmSurvey.windowCenters]). Every frame gives a level
 *                 for each 100 kHz grid channel in its flat part ([levelAt]); the loudest reading per
 *                 channel is kept in [levels]. Channels still unmeasured get windows of their own.
 *              2. Candidates: local maxima at least the threshold above the noise floor ([pickCandidates]).
 *              3. Verify: each candidate is tuned in WFM (with Stereo & RDS) while [FmStationRds]
 *                 collects its RDS and [PsTrack] waits for PS and RadioText to settle; the pilot
 *                 lock gives stereo. A strong carrier without RDS also gets its stronger neighbour
 *                 channel tried for 3 s, since the survey's peak can be one channel off.
 *              4. Finish: duplicates of a PI are marked ([markSamePi]), the user's names are kept,
 *                 the list replaces [presets] (saved in SharedPreferences) and the radio goes back
 *                 to where it was.
 *
 *              Every step is also written to files/fm_scan_log.txt and logcat (tag LocalFmScan).
 *
 * Part of Richard's personal RF Analyzer fork. Ported from the Windows app (windows/src/fmscan.rs).
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
object LocalFmScan {
    private const val TAG = "LocalFmScan"
    private const val PREFS = "local_fm_scan"

    /** What the scan needs from the app; installed by MainViewModel. All calls on the main thread. */
    interface Radio {
        /** The source's sample rate (Hz) and whether it is a file (a file can't be retuned). */
        fun sampleRate(): Long
        fun isFileSource(): Boolean
        fun isRunning(): Boolean
        /** The demodulator's channel frequency now. */
        fun channelFrequency(): Long
        /** WFM with Stereo & RDS on, and the analyzer started if it isn't. */
        fun prepare()
        /** Points the source at [sourceHz] with the channel at [channelHz] (inside the band). */
        fun tuneSurvey(sourceHz: Long, channelHz: Long)
        /** Tunes the WFM channel to [frequencyHz] the way a user's tap would. */
        fun tuneChannel(frequencyHz: Long)
        /** Remembers the current mode/frequency; the returned function goes back to it. */
        fun snapshot(): () -> Unit
    }

    /** Progress for the UI; outlives the scan so the log stays visible. */
    data class Status(
        val running: Boolean = false,
        val stage: String = "",
        val progress: Float = 0f,
        val log: List<String> = emptyList(),
        val finished: Boolean = false,
    )

    @Volatile var radio: Radio? = null

    private val _status = MutableStateFlow(Status())
    val status: StateFlow<Status> = _status.asStateFlow()
    private val _presets = MutableStateFlow<List<FmPreset>>(emptyList())
    val presets: StateFlow<List<FmPreset>> = _presets.asStateFlow()

    /** The scan's settings; tests narrow the band. */
    var config = FmScanConfig()
        internal set

    private var prefs: SharedPreferences? = null
    private var logFile: File? = null
    /** Where the scan coroutine runs: the main thread in the app, a test's own thread in unit tests. */
    internal var dispatcher: CoroutineDispatcher = Dispatchers.Main
    private val scope by lazy { CoroutineScope(SupervisorJob() + dispatcher) }
    private var job: Job? = null

    // --- survey state, written by the FFT thread (under [lock]) ---
    private val lock = Any()
    /** Set while the scan wants spectrum frames (checked by FftProcessor for every frame). */
    @Volatile var frameWanted = false
        private set
    private var grid = LongArray(0)
    /**
     * Level per grid channel (dB), NaN until measured: per window the median over its frames, and
     * over windows the lowest of those (see [finishWindow]).
     */
    private var levels = FloatArray(0)
    /** The current window's frames: per frame the level of each grid channel (NaN = not measured). */
    private val windowFrames = ArrayList<FloatArray>()
    private var dbScratch = FloatArray(0)
    /** Frames are used only after this time (the hardware retunes and the FFT settles). */
    @Volatile private var acceptFramesAfterMs = Long.MAX_VALUE
    private var liveSource = true

    // --- telling stale frames from fresh ones after a retune (see [markRetune]) ---
    /** Coarse spectrum of the newest frame, and of the last frame before the latest retune. */
    private var lastProfile: FloatArray? = null
    private var beforeProfile: FloatArray? = null
    private var retuneAtMs = 0L
    /** The frames show the new frequency's samples (since [freshSinceMs]). */
    private var fresh = true
    private var freshSinceMs = 0L
    /** Frames still to skip after the change: their FFT window can mix old and new samples. */
    private var skipFrames = 0
    /** How long frames may stay stale before they are used anyway (a retune that changes nothing visible). */
    private const val STALE_FALLBACK_MS = 2500L
    /** While verifying: the channel being checked, and its level in each frame. */
    @Volatile private var verifyFrequency = 0L
    private val verifyLevels = ArrayList<Float>()

    // --- RDS of the station being checked (written by the Demodulator thread) ---
    private val stationRds = FmStationRds()

    fun init(context: Context) {
        if (prefs != null) return
        prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        logFile = File(context.applicationContext.filesDir, "fm_scan_log.txt")
        _presets.value = load()
    }

    // ------------------------------------------------------------------ taps

    /**
     * One FFT frame from FftProcessor's thread: [mag] in the FFT's byte scale (dB = minDb + byte *
     * invScale), index 0 at [frequency] - [sampleRate]/2.
     */
    fun onFftFrame(mag: ByteArray, frequency: Long, sampleRate: Long, minDb: Float, invScale: Float) {
        if (!frameWanted) return
        val now = System.currentTimeMillis()
        synchronized(lock) {
            val n = mag.size
            if (dbScratch.size < n) dbScratch = FloatArray(n)
            for (i in 0 until n) dbScratch[i] = minDb + (mag[i].toInt() and 0xFF) * invScale
            val profile = spectrumProfile(dbScratch, n)
            // After a retune FftProcessor labels frames with the new frequency at once, but the
            // samples between the radio and the FFT (USB transfers, the scheduler, the FFT's ring
            // buffer: up to about a second at 2 Msps) are still from the old one. Such frames are
            // the same spectrum as the last frame before the retune; the fresh ones differ.
            if (!fresh) {
                val before = beforeProfile
                if (before == null || profileChanged(before, profile) || now - retuneAtMs > STALE_FALLBACK_MS) {
                    fresh = true
                    freshSinceMs = now
                    skipFrames = 2
                }
            }
            lastProfile = profile
            if (!fresh) return
            if (skipFrames > 0) { skipFrames--; return }
            if (now < acceptFramesAfterMs) return
            val c = frequency.toDouble(); val r = sampleRate.toDouble()
            val f = verifyFrequency
            if (f != 0L) {
                // verifying: the channel's own level, anywhere in the frame
                levelAt(dbScratch, n, c, r, f.toDouble(), 0.0, 0.45 * r)?.let { verifyLevels += it }
                return
            }
            val (dc, edge) = if (liveSource) Pair(FmSurvey.DC * r, FmSurvey.EDGE * r) else Pair(0.0, 0.45 * r)
            val row = FloatArray(grid.size) { k -> levelAt(dbScratch, n, c, r, grid[k].toDouble(), dc, edge) ?: Float.NaN }
            windowFrames += row
        }
    }

    /**
     * Call right after retuning: frames are ignored until they show the new samples (their coarse
     * spectrum differs from the last frame before the retune), or after [STALE_FALLBACK_MS].
     */
    private fun markRetune() = synchronized(lock) {
        beforeProfile = lastProfile?.copyOf()
        retuneAtMs = System.currentTimeMillis()
        fresh = false
        skipFrames = 0
    }

    /** Milliseconds since the frames started showing the new frequency, or -1 while still stale. */
    private fun freshForMs(): Long = synchronized(lock) { if (fresh) System.currentTimeMillis() - freshSinceMs else -1L }

    /**
     * Ends a survey window: each channel's level in it is the median over its frames, and a
     * channel's level over the whole survey the lowest of its windows' medians.
     *
     * Why not the loudest reading: (1) when the source is retuned, FftProcessor takes the new
     * frequency at once, but the frames right after it are still computed from samples received
     * before the retune, so they show every station shifted by one window; (2) a strong station
     * leaves a mirror image (IQ imbalance, seen on the HackRF) at the same distance on the other side
     * of the window centre. Both are in few frames or only in some windows, while a real carrier is
     * in every frame of every window that covers its channel (windows overlap, so most channels are
     * measured twice). Keeping the loudest reading turned them into "stations" that were only noise.
     */
    private fun finishWindow() = synchronized(lock) {
        val frames = windowFrames.size
        for (k in grid.indices) {
            val v = FloatArray(frames)
            var m = 0
            for (row in windowFrames) if (!row[k].isNaN()) v[m++] = row[k]
            if (m == 0) continue
            v.sort(0, m)
            val med = if (m % 2 == 1) v[m / 2] else (v[m / 2 - 1] + v[m / 2]) / 2
            levels[k] = if (levels[k].isNaN()) med else minOf(levels[k], med)
        }
        windowFrames.clear()
    }

    /** Every RDS group from the local decoder (Demodulator thread), with the tuned frequency. */
    fun onRdsGroup(blocks: IntArray, frequencyHz: Long) {
        val f = verifyFrequency
        if (f != 0L && abs(frequencyHz - f) < 20_000) stationRds.onGroup(blocks)
    }

    // ------------------------------------------------------------------ presets

    fun tunePreset(frequencyHz: Long) {
        if (_status.value.running) stopScan()
        radio?.tuneChannel(frequencyHz) ?: Log.w(TAG, "tunePreset: no radio installed")
    }

    /** Tunes the next ([dir] > 0) or previous preset from the current channel, wrapping around. */
    fun stepPreset(dir: Int) {
        val list = _presets.value
        val r = radio ?: return
        if (list.isEmpty()) return
        val ch = r.channelFrequency()
        val next = if (dir > 0) list.firstOrNull { it.frequencyHz > ch + 25_000 } ?: list.first()
        else list.lastOrNull { it.frequencyHz + 25_000 < ch } ?: list.last()
        tunePreset(next.frequencyHz)
    }

    /** The preset at the current channel frequency, if any. */
    fun presetAt(frequencyHz: Long): FmPreset? = _presets.value.firstOrNull { abs(it.frequencyHz - frequencyHz) < 50_000 }

    fun rename(frequencyHz: Long, name: String) = update(_presets.value.map { if (it.frequencyHz == frequencyHz) it.copy(name = name.trim()) else it })
    fun delete(frequencyHz: Long) = update(_presets.value.filter { it.frequencyHz != frequencyHz })
    fun clear() = update(emptyList())
    fun csv(): String = presetsCsv(_presets.value)

    private fun update(list: List<FmPreset>) {
        _presets.value = list.sortedBy { it.frequencyHz }
        save(_presets.value)
    }

    // ------------------------------------------------------------------ scan

    fun startScan() {
        if (job?.isActive == true) return
        val r = radio ?: run { Log.w(TAG, "startScan: no radio installed"); return }
        job = scope.launch {
            val restore = r.snapshot()
            var completed = false
            try {
                completed = runScan(r)
            } catch (e: kotlinx.coroutines.CancellationException) {
                note("=== FM scan stopped")
                throw e
            } finally {
                frameWanted = false
                verifyFrequency = 0L
                acceptFramesAfterMs = Long.MAX_VALUE
                _status.value = _status.value.copy(running = false, finished = completed)
                restore()
            }
        }
    }

    fun stopScan() {
        job?.cancel()
    }

    /** Waits until the running scan (if any) has ended; for tests. */
    internal suspend fun awaitScan() { job?.join() }

    private fun note(line: String) {
        Log.i(TAG, line)
        _status.value = _status.value.copy(log = _status.value.log + line)
        try {
            logFile?.appendText(SimpleDateFormat("HH:mm:ss", Locale.US).format(Date()) + " " + line + "\n")
        } catch (e: Exception) {
            Log.w(TAG, "note: can't write the log: ${e.message}")
        }
    }

    private fun stage(text: String, progress: Float) {
        _status.value = _status.value.copy(stage = text, progress = progress)
    }

    /** Thrown inside the scan when the source stops (unplugged SDR, end of a file). */
    private class SourceStopped : Exception()

    private fun checkRunning(r: Radio) { if (!r.isRunning()) throw SourceStopped() }

    /** The whole scan; returns true when it ran to the end. */
    private suspend fun runScan(r: Radio): Boolean = try {
        scan(r)
    } catch (e: SourceStopped) {
        note("=== FM scan stopped: the source stopped (SDR unplugged, or the end of a file?)")
        false
    }

    private suspend fun scan(r: Radio): Boolean {
        val cfg = config
        _status.value = Status(running = true, stage = "Starting")
        r.prepare()
        // wait (up to 10 s) for the analyzer to run
        var waited = 0
        while (!r.isRunning() && waited < 10_000) { delay(200); waited += 200 }
        if (!r.isRunning()) { note("=== FM scan: the analyzer isn't running (is the SDR connected?)"); return false }

        val rate = r.sampleRate()
        val file = r.isFileSource()
        val g = cfg.grid()
        synchronized(lock) {
            grid = g
            levels = FloatArray(g.size) { Float.NaN }
            liveSource = !file
        }
        val centers = if (file) longArrayOf(0) else FmSurvey.windowCenters(cfg, rate)
        note(String.format(Locale.US, "=== FM scan %.1f-%.1f MHz, step %d kHz, %d sps, %d windows",
            cfg.startHz / 1e6, cfg.endHz / 1e6, cfg.stepHz / 1000, rate, centers.size))
        val started = System.currentTimeMillis()

        // 1. survey
        frameWanted = true
        delay(500) // a frame of the current tuning, for the first retune to compare against
        suspend fun surveyWindow(c: Long) {
            acceptFramesAfterMs = Long.MAX_VALUE
            synchronized(lock) { windowFrames.clear() }
            if (!file) { r.tuneSurvey(c, c + (0.2 * rate).toLong()); markRetune() }
            val t0 = System.currentTimeMillis()
            acceptFramesAfterMs = t0 + 150
            // frames count once they show the new frequency (markRetune); move on after >= 8 such
            // frames over >= 0.8 s, or after 7 s regardless
            while (true) {
                delay(100)
                checkRunning(r)
                val dt = System.currentTimeMillis() - t0
                val frames = synchronized(lock) { windowFrames.size }
                if ((frames >= 8 && freshForMs() > 800) || dt > 7000) break
            }
            acceptFramesAfterMs = Long.MAX_VALUE
            finishWindow()
        }
        for ((i, c) in centers.withIndex()) {
            stage("Surveying the band", 0.3f * i / centers.size)
            surveyWindow(c)
        }
        // channels the planned windows missed (the source may have ended up elsewhere): their own window
        if (!file) {
            var extra = 0
            while (extra < 8) {
                val k = synchronized(lock) { levels.indexOfFirst { it.isNaN() } }
                if (k < 0) break
                extra++
                surveyWindow(g[k] - (0.2 * rate).toLong())
            }
            if (extra > 0) note("Survey: $extra extra windows for channels not measured")
        }
        val lv = synchronized(lock) { levels.copyOf() }
        val (floor, cands) = pickCandidates(lv, cfg.thresholdDb)
        note(String.format(Locale.US, "Survey: noise floor %.1f dB, %d carriers above %.0f dB: %s",
            floor, cands.size, cfg.thresholdDb, cands.joinToString(" ") { String.format(Locale.US, "%.1f", g[it] / 1e6) }))
        if (cands.isEmpty()) {
            note("=== FM scan finished: no stations found. Check the antenna and gain.")
            return true
        }

        // 2. verify each candidate
        val found = ArrayList<FmPreset>()
        for ((i, k) in cands.withIndex()) {
            val f0 = g[k]
            val snr = lv[k] - floor
            stage(String.format(Locale.US, "Checking %.1f MHz (%d of %d)", f0 / 1e6, i + 1, cands.size), 0.3f + 0.7f * i / cands.size)
            var p = listen(r, f0, f0, lv[k], snr, cfg, neighbour = false)
            if (!p.rds && snr >= 12f) {
                // the survey's peak may be one channel off: try the stronger neighbour, unless it is
                // a candidate itself or already found (94.0 and 94.2 both moved to 94.1 once)
                val taken = cands.map { g[it] } + found.map { it.frequencyHz }
                val below = k - 1; val above = k + 1
                fun lvl(j: Int) = if (j in lv.indices && !lv[j].isNaN()) lv[j] else Float.NEGATIVE_INFINITY
                val side = if (lvl(above) >= lvl(below)) above else below
                if (side in g.indices && g[side] !in taken) {
                    stage(String.format(Locale.US, "Trying %.1f MHz next to %.1f", g[side] / 1e6, f0 / 1e6), 0.3f + 0.7f * i / cands.size)
                    val q = listen(r, g[side], f0, lv[k], snr, cfg, neighbour = true)
                    if (q.rds) {
                        note(String.format(Locale.US, "%.1f MHz showed no RDS; found it on %.1f MHz", f0 / 1e6, g[side] / 1e6))
                        p = q
                    }
                }
            }
            // No RDS, no pilot and no carrier when actually tuned: something the survey saw that isn't
            // there (a leftover frame or a mirror image the median/minimum didn't catch). Drop it.
            if (!p.rds && !p.stereo && p.levelDb < floor + cfg.thresholdDb / 2) {
                note(String.format(Locale.US, "%6.1f MHz  dropped: no carrier when tuned (level %.1f dB, floor %.1f dB)", p.frequencyMhz, p.levelDb, floor))
                continue
            }
            note(presetLogLine(p))
            found += p
        }

        // 3. finish
        val marked = markSamePi(found)
        for (p in marked) p.samePiAs?.let {
            note(String.format(Locale.US, "%.1f MHz carries the PI of %.1f MHz (%s): likely another transmitter of it", p.frequencyMhz, it / 1e6, p.ps))
        }
        val old = _presets.value
        update(marked.map { p -> old.firstOrNull { it.frequencyHz == p.frequencyHz && it.name.isNotBlank() }?.let { p.copy(name = it.name) } ?: p })
        val secs = (System.currentTimeMillis() - started) / 1000
        note("=== FM scan finished: ${marked.size} stations, ${marked.count { it.rds }} with RDS ($secs s)")
        stage("Finished", 1f)
        return true
    }

    /**
     * Listens to [freq] (the candidate [candidate] itself, or its [neighbour]) and returns what it
     * showed. Ends when no PI came within the no-RDS wait, 4.5 s after the first stable PS (time for
     * RT, AF, ECC...), or after the configured maximum.
     */
    private suspend fun listen(r: Radio, freq: Long, candidate: Long, surveyLevel: Float, snr: Float, cfg: FmScanConfig, neighbour: Boolean): FmPreset {
        stationRds.reset()
        synchronized(lock) { verifyLevels.clear() }
        verifyFrequency = freq
        r.tuneChannel(freq)
        markRetune() // the tuned level counts only from frames that show the new frequency
        val t0 = System.currentTimeMillis()
        acceptFramesAfterMs = t0 + 300
        val track = PsTrack()
        var pilotSeen = 0
        var lastMetricsTs = 0L
        var psAt: Double? = null
        var blocksAtPi: Triple<Long, Long, Long>? = null
        val max = cfg.maxSecs.toDouble()
        // weak carriers get longer to show RDS: they are often distant transmitters
        val noRdsWait = when {
            neighbour -> 3.0 // a quick look is enough (a PI shows within ~1 s)
            snr >= 15f -> 7.0
            snr >= 11f -> 5.0
            else -> 3.5
        }.coerceAtMost(max)
        var snap = FmStationRds.Snapshot()
        while (true) {
            delay(100)
            checkRunning(r)
            val t = (System.currentTimeMillis() - t0) / 1000.0
            if (t < 0.8) continue // the engine retunes and the previous station's RDS drains
            val m = LocalDiagnostics.fm.value
            // metrics come once a second; count each fresh one for this frequency with the pilot locked
            if (m.timestampMs != lastMetricsTs && abs(m.frequencyHz - freq) < 20_000) {
                lastMetricsTs = m.timestampMs
                if (t >= 1.2 && m.pilotLocked && m.pilotLevelDb > -45f) pilotSeen++
            }
            snap = stationRds.snapshot()
            val rdsSeen = snap.pi != null
            if (rdsSeen && blocksAtPi == null && abs(m.frequencyHz - freq) < 20_000) blocksAtPi = Triple(m.blocksOk, m.blocksCorrected, m.blocksFailed)
            if (track.observe(t, if (rdsSeen) snap.ps else "", snap.rt) && psAt == null) psAt = t
            val done = (!rdsSeen && t >= noRdsWait) || (psAt != null && t >= psAt!! + 4.5) || t >= max
            if (done) break
        }
        verifyFrequency = 0L
        val m = LocalDiagnostics.fm.value
        // median over the frames: the first ones after the retune can still show the previous frequency
        val level = synchronized(lock) { if (verifyLevels.isEmpty()) surveyLevel else verifyLevels.sorted()[verifyLevels.size / 2] }
        val rds = snap.pi != null
        val bler: Float? = if (!rds) null else blocksAtPi?.let { (o, c, f) ->
            val ok = (m.blocksOk - o).coerceAtLeast(0) + (m.blocksCorrected - c).coerceAtLeast(0)
            val bad = (m.blocksFailed - f).coerceAtLeast(0)
            if (ok + bad > 0) bad.toFloat() / (ok + bad) else m.blerSinceTune
        } ?: m.blerSinceTune
        return FmPreset(
            frequencyHz = freq,
            levelDb = level,
            snrDb = snr,
            // stereo: the pilot locked on 2 of the once-a-second reports (the Windows app polls faster and needs 3)
            stereo = pilotSeen >= 2,
            pilotDb = m.pilotLevelDb,
            rds = rds,
            pi = snap.pi,
            ps = if (rds) track.name() else "",
            longPs = snap.longPs,
            pty = snap.pty,
            ptyn = snap.ptyn,
            tp = rds && snap.tp,
            ta = rds && snap.ta,
            music = snap.music,
            ecc = snap.ecc,
            country = rdsCountry(snap.pi, snap.ecc),
            af = snap.af,
            rt = if (rds) track.rt else "",
            bler = bler,
            groupsPerSec = if (rds) m.groupsPerSecond else 0f,
            rtPlus = snap.rtPlus,
            eon = snap.eon,
            tmc = snap.tmc,
            clock = snap.clock,
            oda = snap.oda,
            psSecs = psAt?.toFloat(),
            scanned = SimpleDateFormat("yyyy-MM-dd HH:mm", Locale.US).format(Date()),
        ).also { if (candidate != freq) Log.d(TAG, "listen: neighbour $freq of $candidate: rds=${it.rds}") }
    }

    // ------------------------------------------------------------------ storage

    private fun save(list: List<FmPreset>) {
        if (prefs == null) return // not initialised (unit tests)
        val a = JSONArray()
        for (p in list) a.put(toJson(p))
        prefs?.edit()?.putString("presets", a.toString())?.apply()
    }

    private fun load(): List<FmPreset> = try {
        val a = JSONArray(prefs?.getString("presets", "[]") ?: "[]")
        (0 until a.length()).map { fromJson(a.getJSONObject(it)) }.sortedBy { it.frequencyHz }
    } catch (e: Exception) {
        Log.w(TAG, "load: ${e.message}")
        emptyList()
    }

    fun toJson(p: FmPreset): JSONObject = JSONObject()
        .put("f", p.frequencyHz).put("name", p.name).put("level", p.levelDb.toDouble()).put("snr", p.snrDb.toDouble())
        .put("stereo", p.stereo).put("pilot", p.pilotDb.toDouble()).put("rds", p.rds).put("pi", p.pi ?: -1).put("ps", p.ps)
        .put("lps", p.longPs ?: "").put("pty", p.pty ?: -1).put("ptyn", p.ptyn ?: "").put("tp", p.tp).put("ta", p.ta)
        .put("music", p.music).put("ecc", p.ecc ?: -1).put("country", p.country ?: "").put("af", JSONArray(p.af))
        .put("rt", p.rt).put("bler", p.bler?.toDouble() ?: -1.0).put("gps", p.groupsPerSec.toDouble()).put("rtplus", p.rtPlus)
        .put("eon", p.eon).put("tmc", p.tmc).put("clock", p.clock).put("oda", JSONArray(p.oda))
        .put("psSecs", p.psSecs?.toDouble() ?: -1.0).put("samePi", p.samePiAs ?: -1L).put("scanned", p.scanned)

    fun fromJson(o: JSONObject): FmPreset {
        fun optInt(k: String) = o.optInt(k, -1).takeIf { it >= 0 }
        fun optStr(k: String) = o.optString(k, "").takeIf { it.isNotEmpty() }
        val af = o.optJSONArray("af")
        val oda = o.optJSONArray("oda")
        return FmPreset(
            frequencyHz = o.getLong("f"), name = o.optString("name", ""),
            levelDb = o.optDouble("level", 0.0).toFloat(), snrDb = o.optDouble("snr", 0.0).toFloat(),
            stereo = o.optBoolean("stereo"), pilotDb = o.optDouble("pilot", -120.0).toFloat(), rds = o.optBoolean("rds"),
            pi = optInt("pi"), ps = o.optString("ps", ""), longPs = optStr("lps"), pty = optInt("pty"), ptyn = optStr("ptyn"),
            tp = o.optBoolean("tp"), ta = o.optBoolean("ta"), music = o.optBoolean("music"), ecc = optInt("ecc"),
            country = optStr("country"), af = af?.let { a -> (0 until a.length()).map { a.getDouble(it) } } ?: emptyList(),
            rt = o.optString("rt", ""), bler = o.optDouble("bler", -1.0).takeIf { it >= 0 }?.toFloat(),
            groupsPerSec = o.optDouble("gps", 0.0).toFloat(), rtPlus = o.optBoolean("rtplus"), eon = o.optInt("eon", 0),
            tmc = o.optBoolean("tmc"), clock = o.optBoolean("clock"),
            oda = oda?.let { a -> (0 until a.length()).map { a.getString(it) } } ?: emptyList(),
            psSecs = o.optDouble("psSecs", -1.0).takeIf { it >= 0 }?.toFloat(),
            samePiAs = o.optLong("samePi", -1L).takeIf { it >= 0 }, scanned = o.optString("scanned", ""),
        )
    }
}
