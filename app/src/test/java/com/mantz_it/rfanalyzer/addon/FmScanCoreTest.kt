package com.mantz_it.rfanalyzer.addon

import com.mantz_it.rfanalyzer.addon.fm.FmPreset
import com.mantz_it.rfanalyzer.addon.fm.FmScanConfig
import com.mantz_it.rfanalyzer.addon.fm.FmStationRds
import com.mantz_it.rfanalyzer.addon.fm.FmSurvey
import com.mantz_it.rfanalyzer.addon.fm.PsTrack
import com.mantz_it.rfanalyzer.addon.fm.isMixedPage
import com.mantz_it.rfanalyzer.addon.fm.levelAt
import com.mantz_it.rfanalyzer.addon.fm.markSamePi
import com.mantz_it.rfanalyzer.addon.fm.pickCandidates
import com.mantz_it.rfanalyzer.addon.fm.profileChanged
import com.mantz_it.rfanalyzer.addon.fm.rejectMirrors
import com.mantz_it.rfanalyzer.addon.fm.windowMedians
import com.mantz_it.rfanalyzer.addon.fm.spectrumProfile
import com.mantz_it.rfanalyzer.addon.fm.psDistance
import com.mantz_it.rfanalyzer.addon.fm.rdsCountry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The FM band scan's pure logic (FmScanCore.kt, FmStationRds.kt), same cases as the Windows app's tests. */
class FmScanCoreTest {

    @Test
    fun candidatesAreLocalMaximaAboveFloor() {
        val l = FloatArray(20) { -90f }
        l[4] = -75f; l[5] = -60f; l[6] = -74f
        l[12] = -70f; l[13] = -70f // flat top: only the last is kept
        val (floor, c) = pickCandidates(l, 10f)
        assertEquals(-90f, floor, 0.01f)
        assertEquals(listOf(5, 13), c)
    }

    @Test
    fun unmeasuredChannelsAreIgnored() {
        val l = FloatArray(10) { Float.NaN }
        l[3] = -60f; l[4] = -90f; l[5] = -90f; l[6] = -90f; l[7] = -90f
        val (_, c) = pickCandidates(l, 10f)
        assertEquals(listOf(3), c)
    }

    @Test
    fun levelInFrame() {
        val n = 1024
        val rate = 2_048_000.0
        val db = FloatArray(n) { -100f }
        val bin = ((300_000.0 / rate + 0.5) * n).toInt()
        for (b in bin - 20..bin + 20) db[b] = -40f
        assertTrue(levelAt(db, n, 100e6, rate, 100.3e6, 0.0, 0.45 * rate)!! > -42f)
        assertTrue(levelAt(db, n, 100e6, rate, 100.6e6, 0.0, 0.45 * rate)!! < -95f)
        assertNull(levelAt(db, n, 100e6, rate, 100.05e6, 0.06 * rate, 0.45 * rate))
    }

    @Test
    fun surveyWindowsCoverEveryChannel() {
        val cfg = FmScanConfig()
        val rate = 2_048_000L
        val centers = FmSurvey.windowCenters(cfg, rate)
        for (f in cfg.grid()) {
            assertTrue("$f not covered", centers.any { c ->
                val off = kotlin.math.abs(f - c).toDouble()
                off >= FmSurvey.DC * rate && off <= FmSurvey.EDGE * rate
            })
        }
    }

    @Test
    fun scrollingPsIsNotMixed() {
        val tr = PsTrack()
        for (i in 0 until 200) {
            val t = i * 0.05
            var ps = if ((t.toInt() / 2) % 2 == 0) "NPO" else "PASSIEK"
            if (t % 2.0 < 0.1 && t > 1.0) ps = "NPO   EK"
            tr.observe(t, ps, "")
        }
        assertEquals("NPO PASSIEK", tr.name())
    }

    @Test
    fun stitchedPagesAreDropped() {
        val tr = PsTrack()
        tr.pages += PsTrack.Page("RADIO 2 ", 6.0, 3)
        tr.pages += PsTrack.Page("RADIR2  ", 2.0, 2)
        tr.pages += PsTrack.Page("NPO R2  ", 4.0, 3)
        tr.pages += PsTrack.Page("RAO O 2 ", 2.5, 2)
        assertEquals("RADIO 2 NPO R2", tr.name())
        assertTrue(isMixedPage("KLO     ", listOf("NPO     ", "KLASSIEK")))
        assertFalse(isMixedPage("KLASSIEK", listOf("NPO     ")))
        assertEquals(2, psDistance("RAO O 2", "RADIO 2"))
    }

    @Test
    fun garbledPageIsDropped() {
        val tr = PsTrack()
        for (i in 0 until 300) {
            val t = i * 0.05
            val ps = if (t >= 5.0 && t < 6.2) "RADIR1" else if ((t.toInt() / 3) % 2 == 0) "NPO R1" else "RADIO 1"
            tr.observe(t, ps, "")
        }
        assertEquals("NPO R1 RADIO 1", tr.name())
    }

    @Test
    fun stitchAndOddCharactersAreDropped() {
        val tr = PsTrack()
        tr.pages += PsTrack.Page("NPO     ", 8.0, 4)
        tr.pages += PsTrack.Page("KLAS  EK", 3.0, 1)
        tr.pages += PsTrack.Page("KLASSIEK", 2.0, 4)
        tr.pages += PsTrack.Page("KLASSI  ", 1.5, 1)
        assertEquals("NPO KLASSIEK", tr.name())
        val tr2 = PsTrack()
        tr2.pages += PsTrack.Page("RADIO 1 ", 3.0, 1)
        tr2.pages += PsTrack.Page("RADIO å@", 5.0, 2)
        assertEquals("RADIO 1", tr2.name())
    }

    @Test
    fun shortRecurringPageIsKeptAndNameStartsAtBrand() {
        val tr = PsTrack()
        for (i in 0 until 400) {
            val t = i * 0.05
            val ps = if ((t + 5.0) % 6.6 < 5.0) "NPO     " else "KLASSIEK"
            tr.observe(t, ps, "")
        }
        assertEquals("NPO KLASSIEK", tr.name())
    }

    @Test
    fun radioTextNeedsToBeStable() {
        val tr = PsTrack()
        tr.observe(0.0, "", "EO")         // too short
        tr.observe(0.5, "", "Hello there")
        tr.observe(1.0, "", "Hello there")
        assertEquals("", tr.rt)
        tr.observe(2.1, "", "Hello there")
        assertEquals("Hello there", tr.rt)
    }

    @Test
    fun samePiMarksWeaker() {
        val v = markSamePi(listOf(
            FmPreset(105_300_000, pi = 0x8201, levelDb = 1f, ps = "NPO R1"),
            FmPreset(106_700_000, pi = 0x8201, levelDb = -14f),
        ))
        assertNull(v[0].samePiAs)
        assertEquals(105_300_000L, v[1].samePiAs)
        assertEquals("NPO R1", v[1].ps)
    }

    @Test
    fun countryFromEccOrGuess() {
        assertEquals("Netherlands", rdsCountry(0x8201, 0xE3))
        assertEquals("Belgium?", rdsCountry(0x6303, null))
        assertNull(rdsCountry(null, 0xE3))
    }

    @Test
    fun piNeedsTwoGroups() {
        val r = FmStationRds()
        fun ps(pi: Int, seg: Int, a: Char, b: Char) = intArrayOf(pi, (10 shl 5) or seg, 0, (a.code shl 8) or b.code)
        // one garbled group with another PI must not count
        r.onGroup(ps(0x8E07, 0, 'X', 'X'))
        assertNull(r.snapshot().pi)
        val name = "NPO R1  "
        for (round in 0..1) for (seg in 0..3) r.onGroup(ps(0x8201, seg, name[seg * 2], name[seg * 2 + 1]))
        assertEquals(0x8201, r.snapshot().pi)
        assertEquals(name, r.snapshot().ps)
        // a single stray PI afterwards neither switches the PI nor wipes the PS
        r.onGroup(ps(0x8E07, 0, 'Z', 'Z'))
        assertEquals(0x8201, r.snapshot().pi)
        assertEquals(name, r.snapshot().ps)
        assertEquals(10, r.snapshot().pty)
    }

    @Test
    fun profileTellsStaleFromFreshFrames() {
        val rnd = java.util.Random(5)
        fun frame(stationBin: Int) = FloatArray(2048) { i -> -100f + 3f * rnd.nextFloat() + if (kotlin.math.abs(i - stationBin) < 80) 50f else 0f }
        val before = spectrumProfile(frame(500), 2048)
        // the same signal again (a stale frame, other noise): no change
        assertFalse(profileChanged(before, spectrumProfile(frame(500), 2048)))
        // the station moved by a survey step (about a quarter of the span): changed
        assertTrue(profileChanged(before, spectrumProfile(frame(1050), 2048)))
    }

    @Test
    fun medianIgnoresAStrayFrame() {
        val frames = listOf(floatArrayOf(-90f, -40f), floatArrayOf(-90f, -41f), floatArrayOf(-50f, -40f))
        val m = windowMedians(frames, 2)
        assertEquals(-90f, m[0], 0.01f)
        assertEquals(-40f, m[1], 0.01f)
    }

    @Test
    fun mirrorImagesAreRejected() {
        // grid 99.0..100.0 MHz, window centre 99.5: a strong station at 99.8 and its image at 99.2
        val grid = LongArray(11) { 99_000_000L + it * 100_000 }
        val med = FloatArray(11) { -95f }
        med[8] = -40f           // 99.8, the station
        med[2] = -70f           // 99.2, its image 30 dB down
        med[4] = -60f           // 99.4, a real weak station; its mirror 99.6 is empty
        val out = rejectMirrors(med, grid, 99_500_000L)
        assertTrue(out[2].isNaN())
        assertEquals(-40f, out[8], 0.01f)
        assertEquals(-60f, out[4], 0.01f)
    }
}
