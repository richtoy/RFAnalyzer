package com.mantz_it.rfanalyzer.addon.rds

/**
 * <h1>RF Analyzer - RDS block synchronisation</h1>
 *
 * Module:      RdsBlockSync.kt
 * Description: RDS data bits to groups (IEC 62106): 26-bit blocks with a 10-bit check word and
 *              offset words A/B/C/C'/D, block sync found from two valid blocks the right distance
 *              apart, error correction of bursts up to [MAX_BURST] bits (only once synced),
 *              sync loss after too many bad blocks. Complete groups (all four blocks valid or
 *              corrected) go to [onGroup]; block statistics feed the BLER.
 *
 * Reference: the approach of redsea (windytan, MIT) - re-implemented, not copied.
 *
 * Part of Richard's personal RF Analyzer fork (local decoders replacing the closed extension).
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
class RdsBlockSync(private val onGroup: (blocks: IntArray) -> Unit) {
    companion object {
        private const val POLY = 0x5B9      // x^10+x^8+x^7+x^5+x^4+x^3+1 (without x^10 = 0x1B9 | 0x400)
        private const val MAX_BURST = 2
        // offset words: A, B, C, C', D
        private val OFFSETS = intArrayOf(0x0FC, 0x198, 0x168, 0x350, 0x1B4)
        private const val OFS_A = 0
        private const val OFS_B = 1
        private const val OFS_C = 2
        private const val OFS_CP = 3
        private const val OFS_D = 4

        /** Check word of 16 data bits (without offset). */
        fun checkword(data: Int): Int {
            var reg = 0
            for (i in 15 downTo 0) {
                val bit = (data shr i) and 1
                val fb = ((reg shr 9) and 1) xor bit
                reg = (reg shl 1) and 0x3FF
                if (fb != 0) reg = reg xor (POLY and 0x3FF)
            }
            return reg
        }

        /** Syndrome of a 26-bit block: 0 if valid for no offset... = checkword(data) ^ check. */
        private fun syndrome(block26: Int): Int = checkword(block26 ushr 10) xor (block26 and 0x3FF)

        // syndrome (with offset removed) -> 26-bit error pattern, for bursts up to MAX_BURST bits
        private val correctionTable: Map<Int, Int> = buildMap {
            for (len in 1..MAX_BURST) {
                // burst patterns starting and ending with 1
                val patterns = if (len == 1) listOf(1) else (0 until (1 shl (len - 2))).map { mid -> (1 shl (len - 1)) or (mid shl 1) or 1 }
                for (pat in patterns) for (shift in 0..(26 - len)) {
                    val e = pat shl shift
                    val s = syndrome(e)
                    if (!containsKey(s)) put(s, e)
                }
            }
        }
    }

    // statistics (read and reset by the owner)
    @Volatile var blocksTotal = 0L; private set
    @Volatile var blocksOk = 0L; private set
    @Volatile var blocksCorrected = 0L; private set
    @Volatile var blocksFailed = 0L; private set
    @Volatile var groupsTotal = 0L; private set
    @Volatile var synced = false; private set
    /** Recent block results for the BLER window (true = bad). */
    private val recent = BooleanArray(400)   // last 100 groups
    private var recentPos = 0
    private var recentCount = 0

    /** Block error rate over the last 100 groups (0..1). */
    fun blerRecent(): Float {
        if (recentCount == 0) return 1f
        var bad = 0
        for (i in 0 until recentCount) if (recent[i]) bad++
        return bad.toFloat() / recentCount
    }

    private var shiftReg = 0                 // last 26 bits
    private var bitCount = 0L
    // acquisition
    private var lastFoundBit = -1L
    private var lastFoundOffset = -1
    // synced state
    private var expectedOffset = 0           // 0..3 = A,B,C,D position in group
    private var bitsInBlock = 0
    private var badInWindow = 0
    private var sinceSyncChecked = true
    private var blocksInWindow = 0
    private val group = IntArray(4)
    private val groupValid = BooleanArray(4)

    fun reset() {
        shiftReg = 0; bitCount = 0; lastFoundBit = -1; lastFoundOffset = -1
        synced = false; expectedOffset = 0; bitsInBlock = 0; badInWindow = 0; blocksInWindow = 0
        groupValid.fill(false)
    }

    fun resetStats() {
        blocksTotal = 0; blocksOk = 0; blocksCorrected = 0; blocksFailed = 0; groupsTotal = 0
        recentPos = 0; recentCount = 0
    }

    private fun positionOf(offsetIndex: Int): Int = when (offsetIndex) {
        OFS_A -> 0; OFS_B -> 1; OFS_C, OFS_CP -> 2; else -> 3
    }

    fun pushBit(bit: Int) {
        shiftReg = ((shiftReg shl 1) or (bit and 1)) and 0x3FFFFFF
        bitCount++
        if (!synced) {
            if (bitCount < 26) return
            val s = syndrome(shiftReg)
            for (o in OFFSETS.indices) {
                if (s == OFFSETS[o]) {
                    val pos = positionOf(o)
                    if (lastFoundBit >= 0) {
                        val dist = bitCount - lastFoundBit
                        val lastPos = positionOf(lastFoundOffset)
                        if (dist % 26 == 0L && dist <= 26 * 6 && (lastPos + dist / 26) % 4 == pos.toLong()) {
                            synced = true
                            sinceSyncChecked = false
                            badInWindow = 0; blocksInWindow = 0
                            groupValid.fill(false)
                            acceptBlock(pos, shiftReg ushr 10, true, false)
                            expectedOffset = (pos + 1) % 4
                            bitsInBlock = 0
                            return
                        }
                    }
                    lastFoundBit = bitCount
                    lastFoundOffset = o
                    break
                }
            }
            return
        }
        bitsInBlock++
        if (bitsInBlock < 26) return
        bitsInBlock = 0
        val pos = expectedOffset
        expectedOffset = (expectedOffset + 1) % 4
        var s = syndrome(shiftReg)
        // which offsets are acceptable at this position
        val candidates = if (pos == 2) intArrayOf(OFS_C, OFS_CP) else intArrayOf(when (pos) { 0 -> OFS_A; 1 -> OFS_B; else -> OFS_D })
        var data = -1
        var corrected = false
        for (o in candidates) if (s == OFFSETS[o]) { data = shiftReg ushr 10; break }
        if (data < 0) {
            for (o in candidates) {
                val e = correctionTable[s xor OFFSETS[o]] ?: continue
                data = (shiftReg xor e) ushr 10
                corrected = true
                break
            }
        }
        val ok = data >= 0
        blocksInWindow++
        if (!ok) badInWindow++
        acceptBlock(pos, if (ok) data else 0, ok, corrected)
        // a sync found on noise (or while the demodulator still converges) shows at once
        if (!sinceSyncChecked && blocksInWindow >= 10) {
            sinceSyncChecked = true
            if (badInWindow >= 4) { synced = false; lastFoundBit = -1; blocksInWindow = 0; badInWindow = 0; return }
        }
        if (blocksInWindow >= 50) {
            if (badInWindow > 35) {
                synced = false
                lastFoundBit = -1
            }
            blocksInWindow = 0; badInWindow = 0
        }
    }

    private fun acceptBlock(pos: Int, data: Int, ok: Boolean, corrected: Boolean) {
        blocksTotal++
        when {
            !ok -> blocksFailed++
            corrected -> blocksCorrected++
            else -> blocksOk++
        }
        recent[recentPos] = !ok
        recentPos = (recentPos + 1) % recent.size
        if (recentCount < recent.size) recentCount++

        if (pos == 0) groupValid.fill(false)
        group[pos] = data
        groupValid[pos] = ok
        if (pos == 3) {
            if (groupValid[0] && groupValid[1] && groupValid[2] && groupValid[3]) {
                groupsTotal++
                onGroup(group.copyOf())
            }
            groupValid.fill(false)
        }
    }
}
