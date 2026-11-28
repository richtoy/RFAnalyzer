package com.mantz_it.rfanalyzer.addon

import com.mantz_it.rfanalyzer.addon.fm.FmScanConfig
import com.mantz_it.rfanalyzer.addon.fm.LocalFmScan
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Random
import java.util.concurrent.Executors
import kotlin.math.abs

/**
 * End-to-end run of the FM band scan driver ([LocalFmScan]) against a simulated radio: spectrum
 * frames with three carriers (plus stale frames after each retune and mirror images) for the survey, then per station the decoder's metrics (pilot) and RDS
 * groups. Covers survey -> candidates -> verify -> neighbour retry -> same-PI marking -> presets.
 * Runs in real time over a narrow band (about 30 s).
 */
class LocalFmScanTest {
    private class Station(val hz: Long, val levelDb: Float, val pi: Int?, val ps: String, val stereo: Boolean)

    /** The simulated radio: frames from a thread at ~30/s, RDS groups and metrics for the tuned station. */
    private class FakeRadio(val stations: List<Station>) : LocalFmScan.Radio {
        @Volatile var source = 99_000_000L
        @Volatile var channel = 99_000_000L
        val rate = 2_048_000L
        @Volatile var running = true
        val tuned = ArrayList<Long>()
        private val rnd = Random(1)

        /** When the source was last retuned, and where it was before. */
        @Volatile var retunedAt = 0L
        @Volatile var previousSource = source

        /**
         * One spectrum frame like FftProcessor's, including its two flaws: for 700 ms after a retune
         * the frames still show the old frequency's signal (labelled with the new one), and every
         * station leaves a mirror image 30 dB down on the other side of the centre (IQ imbalance).
         */
        fun frame(): ByteArray {
            val n = 2048
            val mag = ByteArray(n)
            val shown = if (System.currentTimeMillis() - retunedAt < 700) previousSource else source
            for (i in 0 until n) {
                val off = -rate / 2 + i * rate / n
                val f = shown + off
                val mirror = shown - off
                var db = -100f + rnd.nextFloat()
                for (s in stations) {
                    if (abs(f - s.hz) < 80_000) db = maxOf(db, s.levelDb)
                    if (abs(mirror - s.hz) < 80_000) db = maxOf(db, s.levelDb - 30f)
                }
                mag[i] = ((db + 150f) / (160f / 255f)).toInt().coerceIn(0, 255).toByte()
            }
            return mag
        }

        private fun retune(to: Long) {
            if (to == source) return
            previousSource = source
            retunedAt = System.currentTimeMillis()
            source = to
        }

        val thread = Thread {
            var seg = 0
            var lastMetrics = 0L
            var blocks = 0L
            while (running) {
                LocalFmScan.onFftFrame(frame(), source, rate, -150f, 160f / 255f)
                val st = stations.firstOrNull { it.hz == channel }
                if (st?.pi != null) {
                    // one PS segment (group 0A) per ~30 ms
                    val ps = st.ps.padEnd(8)
                    LocalFmScan.onRdsGroup(intArrayOf(st.pi, (10 shl 5) or (seg and 3), 0, (ps[(seg and 3) * 2].code shl 8) or ps[(seg and 3) * 2 + 1].code), channel)
                    seg++; blocks += 4
                }
                val now = System.currentTimeMillis()
                if (now - lastMetrics >= 1000) {
                    lastMetrics = now
                    LocalDiagnostics.publishFm(FmSignalMetrics(
                        timestampMs = now, frequencyHz = channel,
                        pilotLocked = st?.stereo == true, pilotLevelDb = if (st?.stereo == true) -20f else -60f,
                        blocksTotal = blocks, blocksOk = blocks, groupsPerSecond = if (st?.pi != null) 11f else 0f,
                    ))
                }
                Thread.sleep(30)
            }
        }

        override fun sampleRate() = rate
        override fun isFileSource() = false
        override fun isRunning() = true
        override fun channelFrequency() = channel
        override fun prepare() {}
        override fun tuneSurvey(sourceHz: Long, channelHz: Long) { retune(sourceHz); channel = channelHz }
        override fun tuneChannel(frequencyHz: Long) {
            tuned += frequencyHz
            channel = frequencyHz
            // keep the channel inside the band the source receives
            if (abs(frequencyHz - source) > rate / 3) retune(frequencyHz + 300_000)
        }
        override fun snapshot(): () -> Unit = {}
    }

    @Test
    fun scanFindsStationsEndToEnd() = runBlocking {
        val radio = FakeRadio(listOf(
            Station(99_300_000, -45f, 0x8201, "TESTFM", stereo = true),
            Station(99_900_000, -50f, null, "", stereo = true),     // strong, no RDS: neighbour retry
            Station(100_200_000, -70f, 0x8201, "", stereo = false), // weak, same PI as 99.3
        ))
        val exec = Executors.newSingleThreadExecutor()
        LocalFmScan.dispatcher = exec.asCoroutineDispatcher()
        LocalFmScan.config = FmScanConfig(startHz = 99_000_000, endHz = 100_500_000)
        LocalFmScan.radio = radio
        radio.thread.start()
        try {
            LocalFmScan.startScan()
            Thread.sleep(200)
            LocalFmScan.awaitScan()
        } finally {
            radio.running = false
            radio.thread.join()
            exec.shutdown()
        }
        val log = LocalFmScan.status.value.log
        val p = LocalFmScan.presets.value
        assertEquals(log.joinToString("\n"), listOf(99_300_000L, 99_900_000L, 100_200_000L), p.map { it.frequencyHz })
        val a = p[0]
        assertTrue(a.rds); assertEquals(0x8201, a.pi); assertEquals("TESTFM", a.ps); assertTrue(a.stereo)
        assertEquals("Netherlands?", a.country)
        val b = p[1]
        assertFalse(b.rds); assertTrue(b.stereo)
        // the strong carrier without RDS got its neighbour tried
        assertTrue(radio.tuned.toString(), radio.tuned.any { it == 99_800_000L || it == 100_000_000L })
        val c = p[2]
        assertEquals(99_300_000L, c.samePiAs)
        assertEquals("TESTFM", c.ps)
        assertFalse(c.stereo)
        assertTrue(log.last(), log.last().startsWith("=== FM scan finished: 3 stations, 2 with RDS"))
        // no ghosts from the stale frames or the mirror images
        assertEquals(log.joinToString("\n"), 3L, p.size.toLong())
    }
}
