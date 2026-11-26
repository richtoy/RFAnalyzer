package com.mantz_it.rfanalyzer.addon

import com.mantz_it.rfanalyzer.addon.fm.DspUtil
import com.mantz_it.rfanalyzer.addon.fm.MpxDemodulator
import com.mantz_it.rfanalyzer.addon.rds.RdsBlockSync
import com.mantz_it.rfanalyzer.addon.rds.RdsDemodulator
import com.mantz_it.rfanalyzer.addon.rds.RdsGroupParser
import com.mantz_it.rfanalyzer_ipc.digiext.RdsEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Random
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * End-to-end test of the local FM stereo + RDS decoder on a synthetic FM broadcast signal
 * (192 kHz complex baseband): stereo tones, pilot, RDS groups with PS/RT/AF/CT, a clock error
 * between transmitter and receiver, a tuning offset and noise.
 */
class LocalFmRdsDecoderTest {
    private val offsets = intArrayOf(0x0FC, 0x198, 0x168, 0x1B4)

    private fun groups(): List<IntArray> {
        val pi = 0x8204
        val ps = "TESTFM  "
        val rt = "Hello from a synthetic RDS signal 1234\r"
        val list = ArrayList<IntArray>()
        for (seg in 0..3) list += intArrayOf(pi, (1 shl 10) or (10 shl 5) or (1 shl 3) or seg, (225 shl 8) or 23, (ps[seg * 2].code shl 8) or ps[seg * 2 + 1].code)
        for (seg in 0 until 10) {
            fun ch(i: Int) = if (i < rt.length) rt[i].code else 0x20
            list += intArrayOf(pi, (2 shl 12) or (10 shl 5) or seg, (ch(seg * 4) shl 8) or ch(seg * 4 + 1), (ch(seg * 4 + 2) shl 8) or ch(seg * 4 + 3))
        }
        val mjd = 61005 // 2025-11-26
        list += intArrayOf(pi, (4 shl 12) or (10 shl 5) or ((mjd shr 15) and 3), ((mjd and 0x7FFF) shl 1), (9 shl 12) or (30 shl 6) or 2)
        return list
    }

    private class Result(val events: List<RdsEvent>, val bler: Float, val tones: DoubleArray, val stereo: Boolean)

    private fun run(clockErr: Double, snrDb: Double, seconds: Double, pilot: Boolean = true): Result {
        val fs = 192000.0
        val bits = ArrayList<Int>()
        repeat(40) {
            for (g in groups()) for (i in 0..3) {
                val w = (g[i] shl 10) or (RdsBlockSync.checkword(g[i]) xor offsets[i])
                for (b in 25 downTo 0) bits += (w shr b) and 1
            }
        }
        val enc = IntArray(bits.size); var prev = 0
        for (i in bits.indices) { prev = prev xor bits[i]; enc[i] = prev }
        val n = (fs * seconds).toInt()
        val txFs = fs * (1 + clockErr)
        val chip = FloatArray(n) { i ->
            val t = i / txFs * 1187.5
            val e = if (enc[t.toInt()] == 1) 1f else -1f
            if (t - t.toInt() < 0.5) e else -e
        }
        val shape = DspUtil.lowPassTaps(255, fs, 2400.0)
        val shaped = FloatArray(n) { i -> var a = 0f; for (k in shape.indices) { val j = i - k; if (j >= 0) a += chip[j] * shape[k] }; a }
        val rnd = Random(1)
        val re = FloatArray(n); val im = FloatArray(n)
        var ph = 0.0
        val noise = 10.0.pow(-snrDb / 20) / sqrt(2.0)
        for (i in 0 until n) {
            val t = i / txFs
            val l = sin(2 * PI * 1000 * t); val r = 0.5 * sin(2 * PI * 3000 * t)
            val w = 2 * PI * 19000 * t
            val mpx = 0.8 * ((l + r) / 2 + (l - r) / 2 * sin(2 * w)) + (if (pilot) 0.09 else 0.0) * sin(w) + 0.12 * shaped[i] * sin(3 * w + 0.7)
            ph += 2 * PI * 75000 * mpx / fs + 2 * PI * 1500 / fs
            re[i] = (cos(ph) + noise * rnd.nextGaussian()).toFloat()
            im[i] = (sin(ph) + noise * rnd.nextGaussian()).toFloat()
        }
        val parser = RdsGroupParser()
        val events = ArrayList<RdsEvent>()
        val sync = RdsBlockSync { events += parser.parse(it) }
        val rds = RdsDemodulator(192000) { sync.pushBit(it) }
        val dem = MpxDemodulator(192000)
        val outL = ArrayList<Float>(); val outR = ArrayList<Float>()
        val oL = FloatArray(1024); val oR = FloatArray(1024)
        var off = 0
        while (off < n) {
            val m = minOf(4096, n - off)
            val k = dem.process(re.copyOfRange(off, off + m), im.copyOfRange(off, off + m), m, oL, oR)
            for (i in 0 until k) { outL += oL[i]; outR += oR[i] }
            rds.process(dem.mpx(), dem.mpxSize)
            off += m
        }
        fun tone(x: List<Float>, f: Double): Double {
            var a = 0.0; var b = 0.0; val s = x.size - 48000
            for (i in s until x.size) { a += x[i] * cos(2 * PI * f * i / 48000); b += x[i] * sin(2 * PI * f * i / 48000) }
            return 2 * sqrt(a * a + b * b) / 48000
        }
        return Result(events, sync.blerRecent(), doubleArrayOf(tone(outL, 1000.0), tone(outL, 3000.0), tone(outR, 1000.0), tone(outR, 3000.0)), dem.isStereo)
    }

    @Test
    fun decodesStereoAndRds() {
        val r = run(clockErr = 40e-6, snrDb = 20.0, seconds = 5.0)
        assertTrue(r.stereo)
        assertEquals("TESTFM  ", r.events.filterIsInstance<RdsEvent.ProgramService>().last().name)
        assertEquals("Hello from a synthetic RDS signal 1234", r.events.filterIsInstance<RdsEvent.RadioText>().last().text)
        assertEquals(listOf(89.8), r.events.filterIsInstance<RdsEvent.AlternativeFrequencies>().last().list)
        val ct = r.events.filterIsInstance<RdsEvent.ClockTime>().last()
        assertEquals(2025, ct.year); assertEquals(11, ct.month); assertEquals(26, ct.day); assertEquals(9, ct.hour); assertEquals(30, ct.min)
        assertTrue("BLER ${r.bler}", r.bler < 0.05f)
        // separation: L has the 1 kHz tone only, R the 3 kHz tone only
        assertTrue(r.tones[0] > 0.6 && r.tones[1] < 0.02 && r.tones[2] < 0.02 && r.tones[3] > 0.2)
    }

    @Test
    fun decodesRdsOnMonoStation() {
        val r = run(clockErr = -80e-6, snrDb = 15.0, seconds = 4.0, pilot = false)
        assertTrue(!r.stereo)
        assertEquals("TESTFM  ", r.events.filterIsInstance<RdsEvent.ProgramService>().last().name)
    }
}
