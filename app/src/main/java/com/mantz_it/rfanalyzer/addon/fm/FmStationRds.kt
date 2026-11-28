package com.mantz_it.rfanalyzer.addon.fm

import com.mantz_it.rfanalyzer.addon.rds.RdsGroupParser
import com.mantz_it.rfanalyzer_ipc.digiext.RdsEvent

/**
 * <h1>RF Analyzer - FM band scan: RDS of the station being checked</h1>
 *
 * Module:      FmStationRds.kt
 * Description: Collects what one station sends while the band scan listens to it. Gets the raw RDS
 *              groups (four 16-bit blocks, already error-checked by RdsBlockSync) and runs its own
 *              [RdsGroupParser], so the scan doesn't depend on the RDS screens' state.
 *
 *              A PI is accepted only once two groups carried it ([confirmedPi]): a block A that the
 *              error correction got wrong can otherwise show a wrong PI (seen: 8E07 instead of 8201)
 *              and, since the parser resets on every PI change, wipe the station's PS. Groups with
 *              a PI that isn't (yet) confirmed are not parsed.
 *
 *              Written on the Demodulator thread ([onGroup]), read by the scan on the main thread
 *              ([snapshot]), hence the synchronization.
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
class FmStationRds {
    /** What the scan reads on each poll; a copy, safe to keep. */
    data class Snapshot(
        val pi: Int? = null,
        /** The newest complete PS (8 characters, untrimmed). */
        val ps: String = "",
        val rt: String = "",
        val pty: Int? = null,
        val tp: Boolean = false,
        val ta: Boolean = false,
        val music: Boolean = false,
        val af: List<Double> = emptyList(),
        val ecc: Int? = null,
        val ptyn: String? = null,
        val longPs: String? = null,
        val rtPlus: Boolean = false,
        val eon: Int = 0,
        val tmc: Boolean = false,
        val clock: Boolean = false,
        val oda: List<String> = emptyList(),
        /** Groups parsed (with the confirmed PI). */
        val groups: Int = 0,
    )

    private val parser = RdsGroupParser()
    /** How many groups carried each PI that is not the confirmed one (cleared on confirmation). */
    private val piCounts = HashMap<Int, Int>()
    private var confirmedPi = -1
    private var ps = ""
    private var rt = ""
    private var pty: Int? = null
    private var tp = false
    private var ta = false
    private var music = false
    private var af: List<Double> = emptyList()
    private val eonPis = HashSet<Int>()
    private var tmc = false
    private var clock = false
    private val oda = LinkedHashSet<String>()
    private var groups = 0

    /** Starts over for a new frequency. */
    @Synchronized
    fun reset() {
        parser.reset(); piCounts.clear(); confirmedPi = -1
        ps = ""; rt = ""; pty = null; tp = false; ta = false; music = false; af = emptyList()
        eonPis.clear(); tmc = false; clock = false; oda.clear(); groups = 0
    }

    /** One RDS group ([blocks] = A, B, C, D). */
    @Synchronized
    fun onGroup(blocks: IntArray) {
        val pi = blocks[0]
        if (pi != confirmedPi) {
            val n = (piCounts[pi] ?: 0) + 1
            if (n < 2) { piCounts[pi] = n; return }
            // a second group with this PI: it is the station's (the parser resets on the change)
            piCounts.clear()
            confirmedPi = pi
            ps = ""; rt = ""; af = emptyList()
        }
        groups++
        val b = blocks[1]
        pty = (b ushr 5) and 0x1F
        tp = (b ushr 10) and 1 == 1
        val events = try { parser.parse(blocks) } catch (e: Exception) { emptyList() }
        for (e in events) when (e) {
            is RdsEvent.ProgramService -> ps = e.name
            is RdsEvent.RadioText -> rt = e.text.trim()
            is RdsEvent.Flags -> { ta = e.ta; music = e.music }
            is RdsEvent.AlternativeFrequencies -> af = e.list.sorted()
            is RdsEvent.Eon -> eonPis += e.pi
            is RdsEvent.ClockTime -> clock = true
            is RdsEvent.TmcUserMessage, is RdsEvent.TmcAssembledMessage, is RdsEvent.TmcTuningInfo -> tmc = true
            is RdsEvent.AidAnnouncement -> oda += odaName(e.aid, e.applicationGroup, e.groupTypeA)
            else -> Unit
        }
    }

    @Synchronized
    fun snapshot(): Snapshot {
        val x = parser.extras
        return Snapshot(
            pi = confirmedPi.takeIf { it >= 0 },
            ps = ps, rt = rt, pty = if (confirmedPi >= 0) pty else null, tp = tp, ta = ta, music = music, af = af,
            ecc = x.ecc, ptyn = x.ptyn?.takeIf { it.isNotBlank() }, longPs = x.longPs?.takeIf { it.isNotBlank() },
            rtPlus = x.rtPlusTitle != null || x.rtPlusArtist != null,
            eon = eonPis.size, tmc = tmc, clock = clock, oda = oda.toList(), groups = groups,
        )
    }

    private fun odaName(aid: Int, group: Int, a: Boolean): String {
        val what = when (aid) {
            0x4BD7 -> "RT+"
            0xCD46, 0xCD47 -> "TMC"
            0x6552 -> "eRT"
            0x0D45, 0xCD45 -> "TMC (ALERT-C)"
            else -> "ODA"
        }
        return String.format("%s (%04X) in %d%s", what, aid, group, if (a) "A" else "B")
    }
}
