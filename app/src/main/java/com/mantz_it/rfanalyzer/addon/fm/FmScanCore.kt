package com.mantz_it.rfanalyzer.addon.fm

import java.util.Locale
import kotlin.math.abs
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.log10
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow

/**
 * <h1>RF Analyzer - FM band scan: data and pure logic</h1>
 *
 * Module:      FmScanCore.kt
 * Description: The parts of the FM band scan that need no Android: the preset record ([FmPreset]),
 *              the scan settings, the spectrum survey maths ([levelAt], [pickCandidates]), the
 *              tracker that turns the changing PS/RadioText of a station into a clean name
 *              ([PsTrack]), same-PI marking and the country lookup. [LocalFmScan] drives the radio
 *              and uses these; the unit tests (FmScanCoreTest) exercise them directly.
 *
 *              Ported from the Windows app (windows/src/fmscan.rs and rds.rs), keeping its rules:
 *              - RDS counts only once a PI was decoded (block sync alone happens on noise);
 *              - a PS or RadioText counts only after it stayed the same for a while, so a scrolling
 *                PS can't produce a mixed name like "NPO   EK";
 *              - pages of a scrolling PS are joined ("NPO KLASSIEK"), stitched/garbled pages dropped.
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */

/**
 * One station found by the scan (or kept from an earlier one). Presets are identified by
 * [frequencyHz]; the RDS fields are a snapshot taken at the end of the station's check.
 */
data class FmPreset(
    val frequencyHz: Long,
    /** The user's own name; empty means "use the PS". Survives rescans. */
    val name: String = "",
    /** Mean FFT level of the channel while tuned (per-bin dB of the plot). */
    val levelDb: Float = 0f,
    /** Survey level above the band's noise floor. */
    val snrDb: Float = 0f,
    /** Pilot locked on at least 3 polls. */
    val stereo: Boolean = false,
    val pilotDb: Float = -120f,
    /** True only if a (confirmed) PI was decoded on this frequency. */
    val rds: Boolean = false,
    val pi: Int? = null,
    /** Stable PS, possibly the pages of a scrolling PS joined by spaces. */
    val ps: String = "",
    val longPs: String? = null,
    val pty: Int? = null,
    val ptyn: String? = null,
    val tp: Boolean = false,
    val ta: Boolean = false,
    val music: Boolean = false,
    val ecc: Int? = null,
    /** From ECC + PI, or a guess from the PI alone ending in "?". */
    val country: String? = null,
    /** Alternative frequencies in MHz. */
    val af: List<Double> = emptyList(),
    /** A RadioText that stayed unchanged for at least 1.5 s. */
    val rt: String = "",
    /** Block error rate (0..1) from the first PI to the end of the check. */
    val bler: Float? = null,
    val groupsPerSec: Float = 0f,
    val rtPlus: Boolean = false,
    /** Number of other networks announced (EON). */
    val eon: Int = 0,
    val tmc: Boolean = false,
    val clock: Boolean = false,
    /** Announced Open Data Applications ("RT+ (4BD7)", ...). */
    val oda: List<String> = emptyList(),
    /** Seconds until the first stable PS. */
    val psSecs: Float? = null,
    /** A stronger preset carries the same PI: this one is likely a distant transmitter of it. */
    val samePiAs: Long? = null,
    /** Local time of the scan, "yyyy-MM-dd HH:mm". */
    val scanned: String = "",
) {
    val frequencyMhz: Double get() = frequencyHz / 1e6

    /** Display name: the user's name, else the PS, else the frequency. */
    fun label(): String = when {
        name.isNotBlank() -> name.trim()
        ps.isNotBlank() -> ps.trim()
        else -> String.format(Locale.US, "%.1f MHz", frequencyMhz)
    }
}

/** Scan settings (kept with the presets). */
data class FmScanConfig(
    val startHz: Long = 87_500_000,
    val endHz: Long = 108_000_000,
    val stepHz: Long = 100_000,
    /** A channel counts as a carrier when it is this far above the noise floor. */
    val thresholdDb: Float = 10f,
    /** Longest time spent on one station waiting for RDS. */
    val maxSecs: Float = 10f,
) {
    /** Channel frequencies start..end in [stepHz] steps. */
    fun grid(): LongArray {
        val step = stepHz.coerceAtLeast(10_000)
        val n = ((endHz - startHz) / step + 1).toInt().coerceAtLeast(0)
        return LongArray(n) { startHz + it * step }
    }
}

object FmSurvey {
    /**
     * Survey geometry for live sources, as fractions of the sample rate. A channel is measured
     * only between [DC] and [EDGE] from the window centre: nearer the centre is the DC spike,
     * further out the analogue anti-alias filter whose slope tilts the levels (on the HackRF it
     * moved peaks one 100 kHz channel over). Windows are [STEP] apart; STEP + DC <= EDGE
     * guarantees every channel lands in the flat ring of some window.
     */
    const val DC = 0.06
    const val EDGE = 0.34
    const val STEP = 0.27

    /** Source centre frequencies for the survey windows of [cfg] at [rate] samples/s. */
    fun windowCenters(cfg: FmScanConfig, rate: Long): LongArray {
        val w = STEP * rate
        var c = cfg.startHz - 0.2 * rate
        val v = ArrayList<Long>()
        while (c < cfg.endHz + EDGE * rate) {
            v += Math.round(c / 1000.0) * 1000
            c += w
        }
        return v.toLongArray()
    }
}

/**
 * Mean power (dB) in +-50 kHz around [f] in a spectrum frame [db] (index 0 = centre - rate/2),
 * or null if [f] lies outside the usable part of the frame: nearer than [dcExcl] Hz to the centre
 * (DC spike) or further than [edge] Hz from it. The mean is taken in linear power.
 */
fun levelAt(db: FloatArray, n: Int, center: Double, rate: Double, f: Double, dcExcl: Double, edge: Double): Float? {
    val off = f - center
    if (abs(off) < dcExcl || abs(off) > edge || n < 2) return null
    // centre bin of f, and +-50 kHz in bins (at least 1)
    val bin = (off / rate + 0.5) * n
    val hw = max(50_000.0 / rate * n, 1.0)
    val a = max(floor(bin - hw), 0.0).toInt()
    val b = min(ceil(bin + hw).toInt(), n - 1)
    if (a >= b) return null
    var p = 0.0
    for (i in a..b) p += 10.0.pow(db[i] / 10.0)
    p /= (b - a + 1)
    return (10.0 * log10(max(p, 1e-20))).toFloat()
}

/**
 * The noise floor (20th percentile of the measured channels) and the indices of the channels that
 * are local maxima at least [thresholdDb] above it. Unmeasured channels are NaN.
 */
fun pickCandidates(levels: FloatArray, thresholdDb: Float): Pair<Float, List<Int>> {
    val v = levels.filter { !it.isNaN() }.sorted()
    if (v.isEmpty()) return Pair(Float.NaN, emptyList())
    val floor = v[v.size / 5]
    val out = ArrayList<Int>()
    for (i in levels.indices) {
        val l = levels[i]
        if (l.isNaN() || l < floor + thresholdDb) continue
        val left = if (i > 0 && !levels[i - 1].isNaN()) levels[i - 1] else Float.NEGATIVE_INFINITY
        val right = if (i + 1 < levels.size && !levels[i + 1].isNaN()) levels[i + 1] else Float.NEGATIVE_INFINITY
        // >= on the left, > on the right: of a flat top only the last channel is kept
        if (l >= left && l > right) out += i
    }
    return Pair(floor, out)
}

/** Pads/cuts a PS to its 8 character positions. */
private fun pad8(s: String): CharArray = CharArray(8) { if (it < s.length) s[it] else ' ' }

/**
 * True if [p] looks like two pages of a scrolling PS stitched together: every 2-character segment
 * equals the same segment of one of [others] (PS segments arrive separately, so a page change
 * mid-cycle can publish half old, half new text).
 */
fun isMixedPage(p: String, others: List<String>): Boolean {
    if (others.isEmpty()) return false
    val pc = pad8(p)
    val oc = others.map { pad8(it) }
    if (oc.any { it.contentEquals(pc) }) return false
    return (0 until 4).all { i -> oc.any { o -> o[2 * i] == pc[2 * i] && o[2 * i + 1] == pc[2 * i + 1] } }
}

/** Number of character positions (of the 8) where two PS values differ. */
fun psDistance(a: String, b: String): Int {
    val x = pad8(a); val y = pad8(b)
    return (0 until 8).count { x[it] != y[it] }
}

/**
 * What one station showed while it was being checked. PS and RadioText only count once they have
 * stayed the same for a while ([observe]); [name] then builds the station's name from the stable
 * PS pages.
 */
class PsTrack {
    /** One stable PS page: its text (8 characters), seconds seen stable, and episodes (separate
     *  stretches in which it became stable: the pages of a scrolling PS come back every cycle,
     *  a garbled read normally only once). */
    data class Page(val text: String, var secs: Double, var episodes: Int)

    private var lastPs = ""
    private var lastPsSince = 0.0
    private var psCounted = false
    val pages = ArrayList<Page>()
    private var lastRt = ""
    private var lastRtSince = 0.0
    /** The last RadioText that stayed unchanged for >= 1.5 s. */
    var rt = ""
        private set
    private var lastT = 0.0

    /**
     * Called on every poll at time [t] (seconds) with the current raw PS (8 characters, untrimmed
     * so segment positions are preserved) and the trimmed RadioText. A PS becomes stable once
     * unchanged for 1 s; from then on its page accumulates time. Returns true when a page becomes
     * stable for the first time.
     */
    fun observe(t: Double, ps: String, rtNow: String): Boolean {
        // time since the last poll, capped so a stall doesn't count much
        val dt = (t - lastT).coerceIn(0.0, 0.5)
        lastT = t
        var newStable = false
        if (ps.isNotBlank()) {
            if (ps == lastPs) {
                if (t - lastPsSince >= 1.0) {
                    val first = !psCounted
                    psCounted = true
                    val e = pages.find { it.text == ps }
                    if (e != null) {
                        e.secs += dt
                        if (first) e.episodes++
                    } else {
                        pages += Page(ps, dt, 1)
                        newStable = true
                    }
                }
            } else {
                lastPs = ps
                lastPsSince = t
                psCounted = false
            }
        }
        if (rtNow.length >= 4) {
            if (rtNow == lastRt) {
                if (t - lastRtSince >= 1.5) rt = rtNow
            } else {
                lastRt = rtNow
                lastRtSince = t
            }
        }
        return newStable
    }

    /** The station's name: the steadiest PS, or for a scrolling PS its pages joined ("NPO KLASSIEK"). */
    fun name(): String {
        when (pages.size) {
            0 -> return ""
            1 -> return pages[0].text.trim()
        }
        // Several stable pages: a scrolling PS plus reads caught mid-change or garbled. Three passes:
        // 1. drop stitches: pages made entirely of segments of other, at least as established pages
        val unmixed = pages.filter { p ->
            val others = pages.filter { q -> q.text != p.text && (q.secs >= p.secs || q.episodes >= p.episodes) }.map { it.text }
            !isMixedPage(p.text, others)
        }
        // 2. drop one-off reads: keep pages that came back at least twice or were seen for at least
        //    30 % of the longest page's time
        val best = unmixed.maxOfOrNull { it.secs } ?: 0.0
        val kept = unmixed.filter { it.secs >= 0.3 * best || it.episodes >= 2 }
        // 3. of pages within 2 characters of each other (a corrupted copy) keep the better one:
        //    plain ASCII first, then the one seen longer; exact ties keep both
        fun plain(s: String) = s.all { it in ' '..'~' }
        fun better(q: Page, p: Page): Boolean = when {
            plain(q.text) && !plain(p.text) -> true
            !plain(q.text) && plain(p.text) -> false
            else -> q.secs > p.secs
        }
        val chosen = kept.filter { p -> kept.none { q -> q.text != p.text && psDistance(p.text, q.text) <= 2 && better(q, p) } }
            .map { it.text.trim() to it.secs }.toMutableList()
        // The pages repeat in a cycle and the scan can start anywhere in it: start the name at the
        // page shown longest, usually the brand ("NPO" in "NPO KLASSIEK").
        val i = chosen.indices.maxByOrNull { chosen[it].second }
        if (i != null && i > 0) {
            val head = chosen.subList(0, i).toList()
            repeat(i) { chosen.removeAt(0) }
            chosen.addAll(head)
        }
        val names = chosen.map { it.first }
        if (names.size == 1) return names[0]
        // up to 3 pages' worth joins nicely; longer (song titles in the PS) -> the longest-seen page
        val joined = names.joinToString(" ")
        return if (names.isNotEmpty() && joined.length <= 24) joined
        else pages.maxByOrNull { it.secs }?.text?.trim() ?: ""
    }
}

/**
 * Flags presets whose PI also belongs to a stronger preset (a distant transmitter of the same
 * programme, or an image of it) and lets them take that station's PS.
 */
fun markSamePi(found: List<FmPreset>): List<FmPreset> = found.map { p ->
    val pi = p.pi ?: return@map p
    val best = found.filter { it.frequencyHz != p.frequencyHz && it.pi == pi && it.levelDb > p.levelDb }.maxByOrNull { it.levelDb }
    if (best == null) p else p.copy(samePiAs = best.frequencyHz, ps = best.ps.ifEmpty { p.ps })
}

/** Country from ECC + PI country nibble, or a Western-European guess from the PI alone ("Belgium?"). */
fun rdsCountry(pi: Int?, ecc: Int?): String? {
    if (pi == null) return null
    val cc = (pi ushr 12) and 0xF
    if (ecc != null) {
        val c = when (ecc to cc) {
            0xE3 to 0x8 -> "Netherlands"
            0xE0 to 0xD, 0xE0 to 0x1 -> "Germany"
            0xE0 to 0x6 -> "Belgium"
            0xE1 to 0xC -> "United Kingdom"
            0xE1 to 0xF -> "France"
            0xE4 to 0x7 -> "Luxembourg"
            0xE2 to 0xC -> "Ireland"
            0xE1 to 0x4 -> "Switzerland"
            0xE0 to 0xA -> "Austria"
            0xE2 to 0x5 -> "Italy"
            0xE2 to 0xE -> "Spain"
            0xE1 to 0x9 -> "Denmark"
            0xE3 to 0xF -> "Norway"
            0xE3 to 0xE -> "Sweden"
            0xE1 to 0x6 -> "Finland"
            0xE4 to 0x8 -> "Portugal"
            0xE2 to 0x3 -> "Poland"
            0xE2 to 0x2 -> "Czechia"
            0xA0 to 0x1, 0xA0 to 0xB, 0xA0 to 0xD, 0xA0 to 0xE -> "USA"
            0xA1 to 0xB, 0xA1 to 0xC, 0xA1 to 0xD, 0xA1 to 0xE -> "Canada"
            else -> null
        }
        if (c != null) return c
    }
    val g = when (cc) {
        0x8 -> "Netherlands"
        0x6 -> "Belgium"
        0xD, 0x1 -> "Germany"
        0x7 -> "Luxembourg"
        0xF -> "France"
        0xC -> "United Kingdom"
        0xA -> "Austria"
        0x4 -> "Switzerland"
        0x9 -> "Denmark"
        0xE -> "Spain"
        0x5 -> "Italy"
        else -> return null
    }
    return "$g?"
}

/** The presets as CSV, one row per station. */
fun presetsCsv(list: List<FmPreset>): String {
    fun q(s: String) = "\"" + s.replace("\"", "\"\"") + "\""
    val sb = StringBuilder("freq_mhz,name,ps,same_pi_as_mhz,pi,pty,stereo,rds,tp,ta,country,ecc,level_db,snr_db,bler_pct,groups_per_s,af_mhz,rt,ptyn,long_ps,rtplus,eon,tmc,clock,oda,scanned\n")
    for (p in list) {
        sb.append(String.format(Locale.US, "%.2f,", p.frequencyMhz))
            .append(q(p.name)).append(',').append(q(p.ps)).append(',')
            .append(p.samePiAs?.let { String.format(Locale.US, "%.1f", it / 1e6) } ?: "").append(',')
            .append(p.pi?.let { String.format("%04X", it) } ?: "").append(',')
            .append(p.pty?.toString() ?: "").append(',')
            .append(p.stereo).append(',').append(p.rds).append(',').append(p.tp).append(',').append(p.ta).append(',')
            .append(q(p.country ?: "")).append(',')
            .append(p.ecc?.let { String.format("%02X", it) } ?: "").append(',')
            .append(String.format(Locale.US, "%.1f,%.1f,", p.levelDb, p.snrDb))
            .append(p.bler?.let { String.format(Locale.US, "%.1f", it * 100) } ?: "").append(',')
            .append(String.format(Locale.US, "%.1f,", p.groupsPerSec))
            .append(q(p.af.joinToString(" ") { String.format(Locale.US, "%.1f", it) })).append(',')
            .append(q(p.rt)).append(',').append(q(p.ptyn ?: "")).append(',').append(q(p.longPs ?: "")).append(',')
            .append(p.rtPlus).append(',').append(p.eon).append(',').append(p.tmc).append(',').append(p.clock).append(',')
            .append(q(p.oda.joinToString(" "))).append(',').append(p.scanned).append('\n')
    }
    return sb.toString()
}

/** One line of the scan log for a station. */
fun presetLogLine(p: FmPreset): String = String.format(
    Locale.US, "%6.1f MHz  %-8s  PI %-4s  %s  %s  level %.1f dB  SNR %.1f dB  %s%s%s",
    p.frequencyMhz,
    p.ps.ifEmpty { "-" },
    p.pi?.let { String.format("%04X", it) } ?: "-",
    if (p.stereo) "stereo" else "mono  ",
    if (p.rds) String.format(Locale.US, "RDS BLER %.0f%%", (p.bler ?: 1f) * 100) else "no RDS",
    p.levelDb, p.snrDb,
    p.pty?.let { "PTY $it " } ?: "",
    if (p.tp) "TP " else "",
    if (p.rt.isNotEmpty()) "RT \"${p.rt}\"" else "",
).trimEnd()
