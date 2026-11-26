package com.mantz_it.rfanalyzer.addon.rds

import com.mantz_it.rfanalyzer_ipc.digiext.RdsEvent
import com.mantz_it.rfanalyzer_ipc.digiext.TmcRawOptionalField
import java.util.Locale

/**
 * <h1>RF Analyzer - RDS group parser</h1>
 *
 * Module:      RdsGroupParser.kt
 * Description: Turns RDS groups (four 16-bit blocks) into the [RdsEvent]s the app's RDS screens
 *              already understand - the same contract as the closed RFA Digital extension:
 *              flags (0A/0B/15B), PS (0A/0B), AF method A (0A), RadioText (2A/2B), clock time
 *              (4A), ODA announcements and TMC system info (3A), TMC/ALERT-C messages incl.
 *              multi-group reassembly (8A or the announced group), EON (14A/14B). Also collects
 *              Phase-2 extras (ECC, PTYN, long PS, RT+, PIN) in [extras] for the signal panel.
 *
 * Stateful: PS/RT/AF/EON/TMC assembly depends on earlier groups. Everything resets when the PI changes.
 *
 * Part of Richard's personal RF Analyzer fork (local decoders replacing the closed extension).
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
class RdsGroupParser {

    /** Decoded data that the ipc RdsEvent contract has no event for yet (shown by the add-on panel). */
    data class Extras(
        val ecc: Int? = null,
        val languageCode: Int? = null,
        val pin: String? = null,
        val ptyn: String? = null,
        val longPs: String? = null,
        val rtPlusTitle: String? = null,
        val rtPlusArtist: String? = null,
        val di: Int? = null,
    )

    @Volatile var extras = Extras()
        private set

    private var currentPi = -1

    // PS
    private val ps = CharArray(8) { ' ' }
    private var psMask = 0
    private var lastPs: String? = null

    // RT
    private val rt = CharArray(64) { ' ' }
    private var rtMask = 0L
    private var rtAb = -1
    private var rtEnd = -1
    private var rtIsB = false
    private var lastRt: String? = null

    // AF (method A)
    private var afExpected = 0
    private val afList = LinkedHashSet<Double>()
    private var lastAf: List<Double>? = null

    // TMC
    private var tmcGroupType = 8
    private var tmcGroupA = true
    private var odaRtPlusGroup = -1
    private var odaRtPlusA = true

    private class TmcAssembly(var event: Int, var location: Int, var extent: Int, var direction: Boolean, var diversion: Boolean, var ci: Int) {
        val bits = ArrayList<Int>()        // free-format 28-bit chunks in order
        var expectedRemaining = -1
        var groupsSeen = 1
    }
    private val tmcAssemblies = HashMap<Int, TmcAssembly>()

    // EON
    private class EonState(val pi: Int) {
        val ps = CharArray(8) { ' ' }
        var psMask = 0
        val af = LinkedHashSet<Double>()
        var afExpected = 0
        var tp: Boolean? = null
        var ta: Boolean? = null
        var pty: Int? = null
        var lastEmitted: RdsEvent.Eon? = null
    }
    private val eon = HashMap<Int, EonState>()

    // PTYN / long PS / RT+
    private val ptyn = CharArray(8) { ' ' }
    private var ptynMask = 0
    private val longPs = ByteArray(32) { 0x20 }
    private var longPsMask = 0
    private var rtPlusTags: IntArray? = null

    fun reset() {
        currentPi = -1
        ps.fill(' '); psMask = 0; lastPs = null
        rt.fill(' '); rtMask = 0; rtAb = -1; rtEnd = -1; lastRt = null
        afExpected = 0; afList.clear(); lastAf = null
        tmcGroupType = 8; tmcGroupA = true; odaRtPlusGroup = -1
        tmcAssemblies.clear(); eon.clear()
        ptyn.fill(' '); ptynMask = 0; longPs.fill(0x20); longPsMask = 0; rtPlusTags = null
        extras = Extras()
    }

    fun parse(b: IntArray): List<RdsEvent> {
        val a = b[0]; val bb = b[1]; val c = b[2]; val d = b[3]
        if (a != currentPi) { reset(); currentPi = a }
        val type = bb ushr 12
        val versionA = (bb ushr 11) and 1 == 0
        val tp = (bb ushr 10) and 1 == 1
        val events = ArrayList<RdsEvent>(4)

        when {
            type == 0 -> group0(bb, c, d, versionA, tp, events)
            type == 1 && versionA -> group1A(bb, c, d)
            type == 2 -> group2(bb, c, d, versionA, events)
            type == 3 && versionA -> group3A(bb, c, d, events)
            type == 4 && versionA -> group4A(bb, c, d, events)
            type == 10 && versionA -> group10A(bb, c, d)
            type == 14 -> group14(bb, c, d, versionA, events)
            type == 15 && !versionA -> {
                val ta = (bb ushr 4) and 1 == 1
                val ms = (bb ushr 3) and 1 == 1
                events += RdsEvent.Flags(tp, ta, ms)
            }
            type == 15 && versionA -> group15A(bb, c, d)
        }
        if (type == tmcGroupType && versionA == tmcGroupA && type != 0 && type != 2 && type != 14) {
            groupTmc(bb, c, d, events)
        }
        if (type == odaRtPlusGroup && versionA == odaRtPlusA && type != 0 && type != 2) groupRtPlus(bb, c, d)
        return events
    }

    private fun char(code: Int): Char = RdsCharset.decode(code and 0xFF)

    private fun group0(bb: Int, c: Int, d: Int, versionA: Boolean, tp: Boolean, events: MutableList<RdsEvent>) {
        val ta = (bb ushr 4) and 1 == 1
        val ms = (bb ushr 3) and 1 == 1
        events += RdsEvent.Flags(tp, ta, ms)
        val seg = bb and 3
        // decoder identification: one bit per segment (d3..d0 sent in segments 0..3)
        val diBit = (bb ushr 2) and 1
        val di = (extras.di ?: 0) and (1 shl (3 - seg)).inv() or (diBit shl (3 - seg))
        ps[seg * 2] = char(d ushr 8)
        ps[seg * 2 + 1] = char(d)
        psMask = psMask or (1 shl seg)
        if (psMask == 0xF) {
            val name = String(ps)
            if (name != lastPs) { events += RdsEvent.ProgramService(name); lastPs = name }
            psMask = 0
        }
        if (di != extras.di) extras = extras.copy(di = di)
        if (versionA) {
            afCode(c ushr 8, events)
            afCode(c and 0xFF, events)
        }
    }

    private fun afFreq(code: Int): Double? = if (code in 1..204) Math.round((87.5 + code * 0.1) * 10) / 10.0 else null

    private fun afCode(code: Int, events: MutableList<RdsEvent>) {
        when (code) {
            in 225..249 -> {
                val n = code - 224
                if (n != afExpected || afList.size > n) { afList.clear() }
                afExpected = n
            }
            in 1..204 -> {
                if (afExpected == 0) return
                afList += afFreq(code)!!
                if (afList.size > afExpected) { afList.clear() }
                val list = afList.toList()
                val complete = list.size >= afExpected
                if (list != lastAf && (complete || list.size % 3 == 0)) {
                    lastAf = list
                    events += RdsEvent.AlternativeFrequencies(list, complete)
                }
            }
        }
    }

    private fun group1A(bb: Int, c: Int, d: Int) {
        val variant = (c ushr 12) and 7
        when (variant) {
            0 -> extras = extras.copy(ecc = c and 0xFF)
            3 -> extras = extras.copy(languageCode = c and 0xFF)
        }
        if (d != 0) {
            val day = d ushr 11; val hour = (d ushr 6) and 0x1F; val min = d and 0x3F
            extras = extras.copy(pin = String.format(Locale.US, "day %d, %02d:%02d", day, hour, min))
        }
    }

    private fun group2(bb: Int, c: Int, d: Int, versionA: Boolean, events: MutableList<RdsEvent>) {
        val ab = (bb ushr 4) and 1
        val seg = bb and 0xF
        if (ab != rtAb || versionA == rtIsB) {
            if (rtAb != -1) {
                // A/B flag toggled: a new text. Emit what we had if nothing was emitted yet.
                flushRt(events, force = true)
            }
            rt.fill(' '); rtMask = 0; rtEnd = -1
            rtAb = ab; rtIsB = !versionA
        }
        val chars = if (versionA) intArrayOf(c ushr 8, c and 0xFF, d ushr 8, d and 0xFF) else intArrayOf(d ushr 8, d and 0xFF)
        val base = seg * chars.size
        for ((k, code) in chars.withIndex()) {
            val idx = base + k
            if (code == 0x0D) { if (rtEnd < 0 || idx < rtEnd) rtEnd = idx; continue }
            if (idx < rt.size) rt[idx] = char(code)
        }
        rtMask = rtMask or (1L shl seg)
        flushRt(events, force = false)
    }

    private fun flushRt(events: MutableList<RdsEvent>, force: Boolean) {
        val segLen = if (rtIsB) 2 else 4
        val maxLen = 16 * segLen
        val len = if (rtEnd in 0 until maxLen) rtEnd else maxLen
        val neededSegs = (len + segLen - 1) / segLen
        val needMask = if (neededSegs >= 64) -1L else (1L shl neededSegs) - 1
        val complete = (rtMask and needMask) == needMask && neededSegs > 0
        if (!complete && !force) return
        if (!complete && rtMask == 0L) return
        val text = String(rt, 0, len).trimEnd()
        if (text.isNotEmpty() && text != lastRt && complete) {
            lastRt = text
            events += RdsEvent.RadioText(text)
        }
    }

    private fun group3A(bb: Int, c: Int, d: Int, events: MutableList<RdsEvent>) {
        val appGroup = (bb ushr 1) and 0xF
        val appA = bb and 1 == 0
        val aid = d
        events += RdsEvent.AidAnnouncement(appGroup, appA, aid)
        when (aid) {
            0xCD46, 0xCD47 -> {
                if (appGroup != 0) { tmcGroupType = appGroup; tmcGroupA = appA }
                when ((c ushr 14) and 3) {
                    0 -> events += RdsEvent.AidVariant0(
                        locationTable = (c ushr 6) and 0x3F,
                        afi = (c ushr 5) and 1 == 1,
                        enhancedMode = (c ushr 4) and 1 == 1,
                        international = (c ushr 3) and 1 == 1,
                        national = (c ushr 2) and 1 == 1,
                        regional = (c ushr 1) and 1 == 1,
                        urban = c and 1 == 1,
                        aid = aid,
                    )
                    1 -> events += RdsEvent.AidVariant1(gapGroups = (c ushr 12) and 3, serviceId = (c ushr 6) and 0x3F, aid = aid)
                }
            }
            0x4BD7 -> { odaRtPlusGroup = appGroup; odaRtPlusA = appA }
        }
    }

    private fun group4A(bb: Int, c: Int, d: Int, events: MutableList<RdsEvent>) {
        val mjd = ((bb and 3) shl 15) or (c ushr 1)
        val hour = ((c and 1) shl 4) or (d ushr 12)
        val minute = (d ushr 6) and 0x3F
        val offsetSign = if ((d ushr 5) and 1 == 1) -1 else 1
        val offsetHours = offsetSign * (d and 0x1F) / 2.0
        if (mjd == 0 || hour > 23 || minute > 59) return
        // MJD to date (IEC 62106 Annex G)
        val yp = ((mjd - 15078.2) / 365.25).toInt()
        val mp = ((mjd - 14956.1 - (yp * 365.25).toInt()) / 30.6001).toInt()
        val day = mjd - 14956 - (yp * 365.25).toInt() - (mp * 30.6001).toInt()
        val k = if (mp == 14 || mp == 15) 1 else 0
        val year = 1900 + yp + k
        val month = mp - 1 - k * 12
        if (month !in 1..12 || day !in 1..31) return
        // local time for the text
        var localMin = hour * 60 + minute + (offsetHours * 60).toInt()
        localMin = ((localMin % 1440) + 1440) % 1440
        val text = String.format(Locale.US, "%04d-%02d-%02d %02d:%02d UTC (local %02d:%02d, UTC%+.1f)",
            year, month, day, hour, minute, localMin / 60, localMin % 60, offsetHours)
        events += RdsEvent.ClockTime(text, year, month, day, hour, minute, offsetHours)
    }

    private fun group10A(bb: Int, c: Int, d: Int) {
        val seg = bb and 1
        val chars = intArrayOf(c ushr 8, c and 0xFF, d ushr 8, d and 0xFF)
        for (k in 0..3) ptyn[seg * 4 + k] = char(chars[k])
        ptynMask = ptynMask or (1 shl seg)
        if (ptynMask == 3) extras = extras.copy(ptyn = String(ptyn).trim())
    }

    private fun group15A(bb: Int, c: Int, d: Int) {
        // long PS: 8 segments of 4 bytes, UTF-8
        val seg = bb and 7
        val bytes = intArrayOf(c ushr 8, c and 0xFF, d ushr 8, d and 0xFF)
        for (k in 0..3) longPs[seg * 4 + k] = bytes[k].toByte()
        longPsMask = longPsMask or (1 shl seg)
        val endIdx = longPs.indexOf(0x0D.toByte()).let { if (it < 0) 32 else it }
        val segsNeeded = (endIdx + 3) / 4
        if (segsNeeded > 0 && (longPsMask and ((1 shl segsNeeded) - 1)) == (1 shl segsNeeded) - 1) {
            extras = extras.copy(longPs = String(longPs, 0, endIdx, Charsets.UTF_8).trim())
        }
    }

    private fun groupRtPlus(bb: Int, c: Int, d: Int) {
        // RT+ (ODA 4BD7): two tags (content type, start, length) over the current RadioText
        val bits = ((bb.toLong() and 0x1F) shl 32) or ((c.toLong() and 0xFFFF) shl 16) or (d.toLong() and 0xFFFF)
        // B: ... item toggle(1) item running(1) | type1(6) start1(6) len1(6) | type2(6) start2(6) len2(5)
        val type1 = ((bits ushr 29) and 0x3F).toInt()
        val start1 = ((bits ushr 23) and 0x3F).toInt()
        val len1 = ((bits ushr 17) and 0x3F).toInt()
        val type2 = ((bits ushr 11) and 0x3F).toInt()
        val start2 = ((bits ushr 5) and 0x3F).toInt()
        val len2 = (bits and 0x1F).toInt()
        val text = lastRt ?: return
        fun tag(type: Int, start: Int, len: Int) {
            if (type == 0) return
            val end = (start + len + 1).coerceAtMost(text.length)
            if (start >= end) return
            val s = text.substring(start, end).trim()
            when (type) {
                1 -> extras = extras.copy(rtPlusTitle = s)
                4 -> extras = extras.copy(rtPlusArtist = s)
            }
        }
        tag(type1, start1, len1)
        tag(type2, start2, len2)
    }

    // --- TMC (ALERT-C) ---

    private val labelLengths = intArrayOf(3, 3, 5, 5, 5, 8, 8, 8, 8, 11, 16, 16, 16, 16, 0, 0)

    private fun groupTmc(bb: Int, c: Int, d: Int, events: MutableList<RdsEvent>) {
        val tFlag = (bb ushr 4) and 1
        val fFlag = (bb ushr 3) and 1
        val x = bb and 7
        if (tFlag == 1) {
            events += RdsEvent.TmcTuningInfo(variant = bb and 0xF, data2 = c, data3 = d)
            return
        }
        if (fFlag == 1) {
            // single-group user message
            val diversion = (c ushr 15) and 1 == 1
            val direction = (c ushr 14) and 1 == 1
            val extent = (c ushr 11) and 7
            val event = c and 0x7FF
            events += RdsEvent.TmcUserMessage(true, diversion, x, direction, extent, event, d)
            events += RdsEvent.TmcAssembledMessage(
                messageKey = tmcKey(d, event, direction, extent),
                continuityIndex = -1, singleGroup = true, complete = true,
                eventCode = event, extent = extent, direction = direction,
                diversionRecommended = diversion, locationCode = d, durationOrCi = x,
            )
            return
        }
        val ci = x
        val first = (c ushr 15) and 1 == 1
        if (first) {
            val direction = (c ushr 14) and 1 == 1
            val extent = (c ushr 11) and 7
            val event = c and 0x7FF
            events += RdsEvent.TmcUserMessage(false, true, ci, direction, extent, event, d)
            tmcAssemblies[ci] = TmcAssembly(event, d, extent, direction, false, ci)
            return
        }
        val second = (c ushr 14) and 1 == 1
        val gsi = (c ushr 12) and 3
        val free = ((c and 0xFFF) shl 16) or d
        events += RdsEvent.TmcContinuation(ci, second, gsi, free)
        val asm = tmcAssemblies[ci] ?: return
        if (second) {
            asm.bits.clear(); asm.expectedRemaining = gsi
        } else if (asm.expectedRemaining < 0 || gsi != asm.expectedRemaining - 1) {
            // a group is missing: pass on what we have as incomplete
            emitAssembled(asm, complete = false, events)
            tmcAssemblies.remove(ci)
            return
        } else {
            asm.expectedRemaining = gsi
        }
        asm.bits += free
        asm.groupsSeen++
        if (gsi == 0) {
            emitAssembled(asm, complete = true, events)
            tmcAssemblies.remove(ci)
        }
    }

    private fun tmcKey(location: Int, event: Int, direction: Boolean, extent: Int) =
        "L${location}E${event}D${if (direction) 1 else 0}X$extent"

    private fun emitAssembled(asm: TmcAssembly, complete: Boolean, events: MutableList<RdsEvent>) {
        // free-format bit stream, 28 bits per group, MSB first
        val stream = ArrayList<Int>(asm.bits.size * 28)
        for (chunk in asm.bits) for (i in 27 downTo 0) stream += (chunk ushr i) and 1
        val fields = ArrayList<TmcRawOptionalField>()
        var pos = 0
        fun read(n: Int): Int { var v = 0; repeat(n) { v = (v shl 1) or stream[pos++] }; return v }
        while (stream.size - pos >= 4) {
            if ((pos until stream.size).all { stream[it] == 0 }) break
            val label = read(4)
            val len = labelLengths[label]
            if (stream.size - pos < len) break
            fields += TmcRawOptionalField(label, read(len))
        }
        events += RdsEvent.TmcAssembledMessage(
            messageKey = tmcKey(asm.location, asm.event, asm.direction, asm.extent),
            continuityIndex = asm.ci, singleGroup = false, complete = complete,
            eventCode = asm.event, extent = asm.extent, direction = asm.direction,
            diversionRecommended = false, locationCode = asm.location, durationOrCi = asm.ci,
            optionalFields = fields,
        )
    }

    // --- EON ---

    private fun group14(bb: Int, c: Int, d: Int, versionA: Boolean, events: MutableList<RdsEvent>) {
        val piOn = d
        if (piOn == 0 || piOn == currentPi) return
        val st = eon.getOrPut(piOn) { EonState(piOn) }
        val tpOn = (bb ushr 4) and 1 == 1
        st.tp = tpOn
        if (!versionA) {
            st.ta = (bb ushr 3) and 1 == 1
        } else {
            when (val variant = bb and 0xF) {
                in 0..3 -> {
                    st.ps[variant * 2] = char(c ushr 8)
                    st.ps[variant * 2 + 1] = char(c)
                    st.psMask = st.psMask or (1 shl variant)
                }
                4 -> {
                    for (code in intArrayOf(c ushr 8, c and 0xFF)) {
                        when (code) {
                            in 225..249 -> { val n = code - 224; if (n != st.afExpected) st.af.clear(); st.afExpected = n }
                            in 1..204 -> if (st.afExpected > 0) { st.af += afFreq(code)!!; if (st.af.size > st.afExpected) st.af.clear() }
                        }
                    }
                }
                13 -> { st.pty = c ushr 11; st.ta = c and 1 == 1 }
                else -> events += RdsEvent.EonVariantRaw(piOn, variant, c, d)
            }
        }
        if (st.psMask == 0xF) {
            val ev = RdsEvent.Eon(
                pi = piOn, ps = String(st.ps),
                alternativeFrequencies = st.af.toList(),
                afComplete = st.afExpected > 0 && st.af.size >= st.afExpected,
                tp = st.tp, pty = st.pty, ta = st.ta,
            )
            if (ev != st.lastEmitted) { st.lastEmitted = ev; events += ev }
        }
    }
}

/** The RDS basic character set (IEC 62106 Annex E) to Unicode, for the bytes PS/RT use. */
internal object RdsCharset {
    private val high = charArrayOf(
        // 0x80
        'á','à','é','è','í','ì','ó','ò','ú','ù','Ñ','Ç','Ş','β','¡','Ĳ',
        // 0x90
        'â','ä','ê','ë','î','ï','ô','ö','û','ü','ñ','ç','ş','ğ','ı','ĳ',
        // 0xA0
        'ª','α','©','‰','Ğ','ě','ň','ő','π','€','£','$','←','↑','→','↓',
        // 0xB0
        'º','¹','²','³','±','İ','ń','ű','µ','¿','÷','°','¼','½','¾','§',
        // 0xC0
        'Á','À','É','È','Í','Ì','Ó','Ò','Ú','Ù','Ř','Č','Š','Ž','Đ','Ŀ',
        // 0xD0
        'Â','Ä','Ê','Ë','Î','Ï','Ô','Ö','Û','Ü','ř','č','š','ž','đ','ŀ',
        // 0xE0
        'Ã','Å','Æ','Œ','ŷ','Ý','Õ','Ø','Þ','Ŋ','Ŕ','Ć','Ś','Ź','Ŧ','ð',
        // 0xF0
        'ã','å','æ','œ','ŵ','ý','õ','ø','þ','ŋ','ŕ','ć','ś','ź','ŧ',' ',
    )

    fun decode(code: Int): Char = when {
        code in 0x20..0x7D -> code.toChar()
        code == 0x7E -> '¯'
        code >= 0x80 -> high[code - 0x80]
        else -> ' '
    }
}
