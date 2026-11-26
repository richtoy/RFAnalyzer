package com.mantz_it.rfanalyzer.addon.dab

/**
 * <h1>RF Analyzer - DAB events</h1>
 *
 * Module:      DabEvents.kt
 * Description: Folds the native receiver's events (flat JSON objects, one per line, see
 *              nativedab/src/main/cpp/dab_bridge.cpp) into a [DabState]. Pure Kotlin, no Android,
 *              so it is unit-tested on the JVM.
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
object DabEvents {

    /** Applies all lines of [text] to [state]. */
    fun apply(state: DabState, text: String, nowMs: Long = System.currentTimeMillis()): DabState {
        var s = state
        for (line in text.split('\n')) {
            if (line.isBlank()) continue
            val o = parseFlatJson(line) ?: continue
            s = applyOne(s, o, nowMs)
        }
        return s
    }

    internal fun applyOne(s: DabState, o: Map<String, Any?>, nowMs: Long): DabState {
        fun int(k: String) = (o[k] as? Number)?.toInt() ?: 0
        fun long(k: String) = (o[k] as? Number)?.toLong() ?: 0L
        fun str(k: String) = (o[k] as? String) ?: ""
        fun bool(k: String) = o[k] as? Boolean ?: false
        return when (o["t"]) {
            "sync" -> s.copy(sync = bool("v"))
            "sys" -> s.copy(sync = bool("sync"), snrDb = int("snr"), frequencyOffsetHz = int("off"))
            "fic" -> s.copy(quality = s.quality.copy(ficPercent = int("q").coerceIn(0, 100)))
            "ens" -> s.copy(ensembleLabel = str("name").trim(), ensembleId = long("id") and 0xFFFFFFFFL)
            "svc" -> {
                val sid = long("sid") and 0xFFFFFFFFL
                if (s.services.any { it.sid == sid }) s
                else s.copy(services = s.services + DabService(sid = sid, label = str("name").trim()))
            }
            "info", "prog" -> {
                val sid = long("sid") and 0xFFFFFFFFL
                val described = DabService(
                    sid = sid,
                    label = str("name").trim(),
                    shortLabel = str("shortName").trim(),
                    isAudio = true,
                    subchId = int("subch"),
                    startCu = int("start"),
                    lengthCu = int("length"),
                    bitrateKbps = int("bitrate"),
                    shortForm = bool("short"),
                    protLevel = int("prot"),
                    ascty = int("ascty"),
                    pty = int("pty"),
                    language = int("lang"),
                )
                val merged = upsert(s.services, described)
                if (o["t"] == "prog") s.copy(services = merged, playing = described, selectedSid = sid, dls = "",
                    quality = s.quality.copy(framePercent = 0, rsPercent = 0, aacPercent = 0))
                else s.copy(services = merged)
            }
            "data" -> {
                val sid = long("sid") and 0xFFFFFFFFL
                s.copy(services = s.services.map { if (it.sid == sid) it.copy(isAudio = false) else it })
            }
            "time" -> s.copy(timeUtc = "%02d:%02d".format(int("h"), int("m")))
            "dls" -> s.copy(dls = str("text").trim())
            "pq" -> s.copy(quality = s.quality.copy(
                framePercent = int("fe").coerceIn(0, 100), rsPercent = int("rs").coerceIn(0, 100), aacPercent = int("aac").coerceIn(0, 100)))
            "tii" -> {
                val tx = DabTransmitter(int("main"), int("sub"), (o["strength"] as? Number)?.toFloat() ?: 0f, nowMs)
                val list = (s.transmitters.filter { !(it.mainId == tx.mainId && it.subId == tx.subId) && nowMs - it.lastSeenMs < 60_000 } + tx)
                    .sortedByDescending { it.strength }.take(12)
                s.copy(transmitters = list)
            }
            else -> s
        }
    }

    private fun upsert(list: List<DabService>, d: DabService): List<DabService> {
        val i = list.indexOfFirst { it.sid == d.sid }
        if (i < 0) return list + d
        val old = list[i]
        return list.toMutableList().also { it[i] = d.copy(label = d.label.ifEmpty { old.label }) }
    }

    /**
     * Parses a flat JSON object (string, number, boolean and null values; no nesting), as the
     * native bridge writes them. Null on malformed input.
     */
    fun parseFlatJson(text: String): Map<String, Any?>? {
        val out = HashMap<String, Any?>()
        var i = 0
        val n = text.length
        fun ws() { while (i < n && text[i].isWhitespace()) i++ }
        fun string(): String? {
            if (i >= n || text[i] != '"') return null
            i++
            val sb = StringBuilder()
            while (i < n) {
                val c = text[i++]
                when (c) {
                    '"' -> return sb.toString()
                    '\\' -> {
                        if (i >= n) return null
                        when (val e = text[i++]) {
                            'n' -> sb.append('\n'); 't' -> sb.append('\t'); 'r' -> sb.append('\r')
                            'b' -> sb.append('\b'); 'f' -> sb.append('\u000C')
                            'u' -> {
                                if (i + 4 > n) return null
                                sb.append(text.substring(i, i + 4).toIntOrNull(16)?.toChar() ?: return null)
                                i += 4
                            }
                            else -> sb.append(e)
                        }
                    }
                    else -> sb.append(c)
                }
            }
            return null
        }
        ws()
        if (i >= n || text[i] != '{') return null
        i++
        ws()
        if (i < n && text[i] == '}') return out
        while (i < n) {
            ws()
            val key = string() ?: return null
            ws()
            if (i >= n || text[i] != ':') return null
            i++
            ws()
            if (i >= n) return null
            val value: Any? = when {
                text[i] == '"' -> string() ?: return null
                text.startsWith("true", i) -> { i += 4; true }
                text.startsWith("false", i) -> { i += 5; false }
                text.startsWith("null", i) -> { i += 4; null }
                else -> {
                    val start = i
                    while (i < n && (text[i].isDigit() || text[i] in "+-.eE")) i++
                    val num = text.substring(start, i)
                    if (num.isEmpty()) return null
                    if (num.any { it == '.' || it == 'e' || it == 'E' }) num.toDoubleOrNull() ?: return null
                    else num.toLongOrNull() ?: return null
                }
            }
            out[key] = value
            ws()
            if (i < n && text[i] == ',') { i++; continue }
            if (i < n && text[i] == '}') return out
            return null
        }
        return null
    }
}
