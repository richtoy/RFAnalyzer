package com.mantz_it.rfanalyzer.addon

import com.mantz_it.rfanalyzer.addon.dab.DabChannels
import com.mantz_it.rfanalyzer.addon.dab.DabEvents
import com.mantz_it.rfanalyzer.addon.dab.DabState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The native DAB receiver's events (dab_bridge.cpp) folded into the DAB screens' state. */
class DabEventsTest {
    @Test
    fun parsesFlatJson() {
        val o = DabEvents.parseFlatJson("""{"t":"dls","text":"Now: \"Hit\" é\\x","n":-12,"f":1.5,"b":true,"z":null}""")!!
        assertEquals("dls", o["t"])
        assertEquals("Now: \"Hit\" é\\x", o["text"])
        assertEquals(-12L, o["n"])
        assertEquals(1.5, o["f"])
        assertEquals(true, o["b"])
        assertTrue(o.containsKey("z"))
        assertNull(DabEvents.parseFlatJson("{\"t\":"))
        assertNull(DabEvents.parseFlatJson("garbage"))
    }

    @Test
    fun buildsEnsembleAndSelection() {
        val events = """
            {"t":"sync","v":true}
            {"t":"sys","sync":true,"snr":14,"off":-312}
            {"t":"fic","q":97}
            {"t":"ens","name":"NPO           ","id":-31234}
            {"t":"svc","name":"NPO Radio 1     ","sid":33537,"sub":0}
            {"t":"svc","name":"NPO Radio 2","sid":33538,"sub":0}
            {"t":"svc","name":"NPO Radio 1","sid":33537,"sub":0}
            {"t":"info","name":"NPO Radio 2","shortName":"NPO 2","sid":33538,"pty":10,"subch":2,"start":84,"length":72,"short":false,"prot":2,"bitrate":96,"ascty":63,"lang":29}
            {"t":"prog","name":"NPO Radio 1","shortName":"NPO 1","sid":33537,"pty":1,"subch":1,"start":0,"length":84,"short":false,"prot":2,"bitrate":112,"ascty":63,"lang":29}
            {"t":"dls","text":"Het nieuws"}
            {"t":"pq","fe":100,"rs":98,"aac":100}
            {"t":"time","h":9,"m":5}
        """.trimIndent()
        val s = DabEvents.apply(DabState(active = true, frequencyHz = 225_648_000), events, 1000)
        assertTrue(s.sync)
        assertEquals(14, s.snrDb)
        assertEquals(-312, s.frequencyOffsetHz)
        assertEquals(97, s.quality.ficPercent)
        assertEquals("NPO", s.ensembleLabel)
        assertEquals(2, s.services.size)
        assertEquals(33537L, s.selectedSid)
        assertEquals("NPO Radio 1", s.playing?.label)
        assertEquals("EEP-3A", s.playing?.protection)
        assertEquals("DAB+", s.playing?.codecShort)
        assertEquals(96, s.services.first { it.sid == 33538L }.bitrateKbps)
        assertEquals("Het nieuws", s.dls)
        assertEquals(98, s.quality.rsPercent)
        assertEquals("09:05", s.timeUtc)
        assertEquals(156, s.usedCu)
        assertEquals("12B", s.channel?.name)
        assertEquals("Dutch", com.mantz_it.rfanalyzer.addon.dab.DabNames.language(s.playing!!.language))
    }

    @Test
    fun channelTable() {
        assertEquals(41, DabChannels.bandIII.size)
        assertEquals(174_928_000L, DabChannels.byName("5a")!!.frequencyHz)
        assertEquals("13F", DabChannels.forFrequency(239_205_000)!!.name)
        assertNull(DabChannels.forFrequency(200_000_000))
        assertTrue(DabChannels.scanList.none { it.name.endsWith("N") })
    }
}
