package com.mantz_it.rfanalyzer.addon.dab

/**
 * <h1>RF Analyzer - DAB model</h1>
 *
 * Module:      DabModel.kt
 * Description: What the local DAB/DAB+ receiver knows about the ensemble it is tuned to, as the
 *              DAB screens show it: Band III channels, services with their subchannel parameters,
 *              signal quality. Plain data, built by [DabEvents] from the native receiver's events.
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */

data class DabChannel(val name: String, val frequencyHz: Long) {
    val frequencyMhz: String get() = "%.3f".format(frequencyHz / 1e6)
}

object DabChannels {
    /** Band III (ETSI EN 300 401 Table B.1), 5A..13F including the N channels. */
    val bandIII: List<DabChannel> = listOf(
        "5A" to 174_928, "5B" to 176_640, "5C" to 178_352, "5D" to 180_064,
        "6A" to 181_936, "6B" to 183_648, "6C" to 185_360, "6D" to 187_072,
        "7A" to 188_928, "7B" to 190_640, "7C" to 192_352, "7D" to 194_064,
        "8A" to 195_936, "8B" to 197_648, "8C" to 199_360, "8D" to 201_072,
        "9A" to 202_928, "9B" to 204_640, "9C" to 206_352, "9D" to 208_064,
        "10A" to 209_936, "10N" to 210_096, "10B" to 211_648, "10C" to 213_360, "10D" to 215_072,
        "11A" to 216_928, "11N" to 217_088, "11B" to 218_640, "11C" to 220_352, "11D" to 222_064,
        "12A" to 223_936, "12N" to 224_096, "12B" to 225_648, "12C" to 227_360, "12D" to 229_072,
        "13A" to 230_784, "13B" to 232_496, "13C" to 234_208, "13D" to 235_776, "13E" to 237_488, "13F" to 239_200,
    ).map { (name, khz) -> DabChannel(name, khz * 1000L) }

    /** The N channels overlap their neighbours and are rarely used: a scan skips them. */
    val scanList: List<DabChannel> = bandIII.filter { !it.name.endsWith("N") }

    fun forFrequency(frequencyHz: Long, toleranceHz: Long = 20_000): DabChannel? =
        bandIII.minByOrNull { kotlin.math.abs(it.frequencyHz - frequencyHz) }
            ?.takeIf { kotlin.math.abs(it.frequencyHz - frequencyHz) <= toleranceHz }

    fun byName(name: String): DabChannel? = bandIII.firstOrNull { it.name.equals(name, ignoreCase = true) }
}

/** One service of the ensemble. Subchannel fields are -1/0 until the receiver has described it. */
data class DabService(
    val sid: Long,
    val label: String,
    val shortLabel: String = "",
    /** null: not known yet. */
    val isAudio: Boolean? = null,
    val subchId: Int = -1,
    val startCu: Int = -1,
    val lengthCu: Int = 0,
    val bitrateKbps: Int = 0,
    val shortForm: Boolean = false,
    val protLevel: Int = 0,
    /** Audio service component type: 63 = DAB+ (HE-AAC v2), 0 = DAB (MPEG-1 Layer II). */
    val ascty: Int = -1,
    val pty: Int = 0,
    val language: Int = 0,
) {
    val described: Boolean get() = isAudio == false || subchId >= 0
    val sidHex: String get() = "%04X".format(sid)
    val codec: String get() = when {
        isAudio == false -> "Data"
        ascty == 63 -> "DAB+ (HE-AAC)"
        ascty == 0 -> "DAB (MP2)"
        else -> "-"
    }
    val codecShort: String get() = when {
        isAudio == false -> "data"
        ascty == 63 -> "DAB+"
        ascty == 0 -> "DAB"
        else -> ""
    }
    /** EEP long form (dab-cmdline: 0..3 = 1A..4A, 4..7 = 1B..4B) or UEP short form (table level 1..5). */
    val protection: String get() = when {
        subchId < 0 -> "-"
        shortForm -> "UEP-$protLevel"
        protLevel in 0..3 -> "EEP-${protLevel + 1}A"
        protLevel in 4..7 -> "EEP-${protLevel - 3}B"
        else -> "?"
    }
    val ptyName: String get() = DabNames.pty(pty)
}

/** A transmitter identified by its TII (main/sub id), strongest first. */
data class DabTransmitter(val mainId: Int, val subId: Int, val strength: Float, val lastSeenMs: Long)

data class DabQuality(
    /** Percentage of FIBs passing the CRC (FIC quality). */
    val ficPercent: Int = 0,
    /** DAB+: valid superframes %; DAB: valid frames %. */
    val framePercent: Int = 0,
    /** DAB+: Reed-Solomon passes %. */
    val rsPercent: Int = 0,
    /** DAB+: valid AAC frames %. */
    val aacPercent: Int = 0,
)

data class DabState(
    val active: Boolean = false,
    val frequencyHz: Long = 0,
    val sync: Boolean = false,
    /** Time sync was reached at least once since the last retune (sync itself can flicker on a weak signal). */
    val syncSeen: Boolean = false,
    val snrDb: Int = 0,
    val frequencyOffsetHz: Int = 0,
    val quality: DabQuality = DabQuality(),
    val ensembleLabel: String = "",
    val ensembleId: Long = -1,
    val timeUtc: String = "",
    /** In first-heard order. */
    val services: List<DabService> = emptyList(),
    val selectedSid: Long = -1,
    /** The program data of the playing service (from the receiver on selection). */
    val playing: DabService? = null,
    val dls: String = "",
    val slideName: String = "",
    val slideType: Int = 0,
    /** Bumped whenever a new slide arrives (the bytes are in [DabEvents.slide]). */
    val slideSerial: Int = 0,
    val audioRate: Int = 48_000,
    val audioBufferedMs: Int = 0,
    val droppedSamples: Long = 0,
    val transmitters: List<DabTransmitter> = emptyList(),
    /** Seconds since the last retune (for "searching..." messages). */
    val secondsSinceTune: Int = 0,
) {
    val channel: DabChannel? get() = DabChannels.forFrequency(frequencyHz)
    val audioServices: List<DabService> get() = services.filter { it.isAudio != false }
    val selectedService: DabService? get() = services.firstOrNull { it.sid == selectedSid }
    val ensembleIdHex: String get() = if (ensembleId < 0) "-" else "%04X".format(ensembleId and 0xFFFF)
    /** Country id (first nibble of the EId). */
    val countryId: Int get() = if (ensembleId < 0) -1 else ((ensembleId shr 12) and 0xF).toInt()
    val usedCu: Int get() = services.filter { it.subchId >= 0 }.distinctBy { it.subchId }.sumOf { it.lengthCu }
}

/** A scan's finding. */
data class DabEnsembleEntry(val channel: String, val frequencyHz: Long, val label: String, val ensembleId: Long, val services: List<String>)

data class DabScanState(
    val running: Boolean = false,
    val channelIndex: Int = 0,
    val channelCount: Int = 0,
    val currentChannel: String = "",
    val found: List<DabEnsembleEntry> = emptyList(),
    /** What the scan saw on each channel, newest last ("11C: no signal", "12B: NPO, 8 services"). */
    val log: List<String> = emptyList(),
    val finished: Boolean = false,
)

object DabNames {
    private val PTY = arrayOf(
        "None", "News", "Current Affairs", "Information", "Sport", "Education", "Drama", "Culture",
        "Science", "Varied", "Pop Music", "Rock Music", "Easy Listening", "Light Classical",
        "Serious Classical", "Other Music", "Weather", "Finance", "Children's", "Social Affairs",
        "Religion", "Phone In", "Travel", "Leisure", "Jazz Music", "Country Music", "National Music",
        "Oldies Music", "Folk Music", "Documentary", "Alarm Test", "Alarm",
    )
    fun pty(code: Int): String = PTY.getOrNull(code) ?: "PTY $code"

    /** ETSI TS 101 756 Table 9 (the common ones). */
    fun language(code: Int): String = when (code) {
        0 -> "-"; 0x01 -> "Albanian"; 0x05 -> "Czech"; 0x06 -> "Danish"; 0x07 -> "German"; 0x09 -> "English"
        0x0A -> "Spanish"; 0x0F -> "French"; 0x13 -> "Croatian"; 0x15 -> "Italian"; 0x1D -> "Dutch"
        0x1E -> "Norwegian"; 0x20 -> "Polish"; 0x21 -> "Portuguese"; 0x24 -> "Slovak"; 0x27 -> "Finnish"
        0x28 -> "Swedish"; 0x29 -> "Turkish"; 0x41 -> "Frisian"; 0x56 -> "Russian"
        else -> "0x%02X".format(code)
    }
}
