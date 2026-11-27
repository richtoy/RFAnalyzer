package com.mantz_it.rfanalyzer.addon

import com.mantz_it.rfanalyzer.addon.rds.RdsGroupParser
import com.mantz_it.rfanalyzer_ipc.digiext.RdsEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** A new PI is only taken once two groups in a row carry it (one miscorrected block A is ignored). */
class RdsPiConfirmTest {
    /** The four group 0A groups that carry an 8-character PS. */
    private fun psGroups(pi: Int, ps: String): List<IntArray> = (0 until 4).map { seg ->
        intArrayOf(pi, (10 shl 5) or seg, 0xE0CD, (ps[seg * 2].code shl 8) or ps[seg * 2 + 1].code)
    }

    private fun lastPs(events: List<RdsEvent>) = events.filterIsInstance<RdsEvent.ProgramService>().lastOrNull()?.name

    @Test
    fun singleWrongPiIsIgnored() {
        val p = RdsGroupParser()
        val ev = ArrayList<RdsEvent>()
        (psGroups(0x8201, "NPO R1  ") + psGroups(0x8201, "NPO R1  ")).forEach { ev += p.parse(it) }
        assertEquals("NPO R1  ", lastPs(ev))
        // one group with a miscorrected PI is held back and does not wipe the PS
        val bad = psGroups(0x8E07, "NPO R1  ")[0]
        assertTrue(p.parse(bad).isEmpty())
        assertTrue(p.lastGroupHeld)
        // the station's state survived: the same PS again is not re-announced (a reset would
        // have cleared it, and the PS would be emitted anew)
        ev.clear()
        psGroups(0x8201, "NPO R1  ").forEach { ev += p.parse(it) }
        assertFalse(p.lastGroupHeld)
        assertEquals(null, lastPs(ev))
    }

    @Test
    fun realStationChangeIsTaken() {
        val p = RdsGroupParser()
        val ev = ArrayList<RdsEvent>()
        (psGroups(0x8201, "NPO R1  ") + psGroups(0x8201, "NPO R1  ")).forEach { ev += p.parse(it) }
        ev.clear()
        // the same new PI in consecutive groups: the first is held, the rest are taken
        (psGroups(0x8E07, "OTHER   ") + psGroups(0x8E07, "OTHER   ")).forEach { ev += p.parse(it) }
        assertFalse(p.lastGroupHeld)
        assertEquals("OTHER   ", lastPs(ev))
    }
}
