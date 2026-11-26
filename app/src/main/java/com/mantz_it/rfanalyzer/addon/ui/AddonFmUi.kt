package com.mantz_it.rfanalyzer.addon.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mantz_it.rfanalyzer.addon.FmSignalMetrics
import com.mantz_it.rfanalyzer.addon.LocalDiagnostics
import java.util.Locale

/**
 * <h1>RF Analyzer - Add-on FM / RDS screens</h1>
 *
 * Module:      AddonFmUi.kt
 * Description: The technical-info screens of this fork's local FM stereo + RDS decoder:
 *              - [LocalFmControls]: Stereo and de-emphasis, under "Stereo & RDS" in the Demodulation tab.
 *              - [RdsSignalPanel]: pinned under the RDS ticker: BLER now / since tune with a 60 s
 *                sparkline, groups per second, corrected/uncorrected blocks, pilot level and lock,
 *                RDS injection, peak deviation, stereo/mono.
 *              - [RdsAddonDetails]: on the station's Details page: ECC, language, PTYN, long PS,
 *                RT+ title/artist, PIN, a groups histogram and the MPX spectrum view (0-96 kHz
 *                with 19/38/57 kHz markers).
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */

internal val Good = Color(0xFF2E9D57)
internal val Warn = Color(0xFFD08A00)
internal val Bad = Color(0xFFC0392B)

@Composable
fun LocalFmControls(modifier: Modifier = Modifier) {
    val settings by LocalDiagnostics.settings.collectAsState()
    Column(modifier.fillMaxWidth().padding(vertical = 4.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.weight(1f)) {
                Text("Stereo", style = MaterialTheme.typography.bodyMedium)
                Text("Off keeps weak stations in mono while RDS still decodes.", style = MaterialTheme.typography.bodySmall)
            }
            Switch(checked = settings.first, onCheckedChange = { LocalDiagnostics.setStereo(it) })
        }
        Spacer(Modifier.height(6.dp))
        Text("De-emphasis", style = MaterialTheme.typography.bodyMedium)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for ((label, us) in listOf("50 µs (Europe)" to 50.0, "75 µs (Americas)" to 75.0, "Off" to 0.0)) {
                FilterChip(selected = settings.second == us, onClick = { LocalDiagnostics.setDeemphasis(us) }, label = { Text(label, fontSize = 12.sp) })
            }
        }
    }
}

private fun blerColor(bler: Float) = when {
    bler < 0.05f -> Good
    bler < 0.25f -> Warn
    else -> Bad
}

@Composable
internal fun Dot(color: Color) {
    Box(Modifier.size(8.dp).background(color, CircleShape))
}

@Composable
internal fun Metric(label: String, value: String, modifier: Modifier = Modifier) {
    Column(modifier.padding(end = 10.dp, bottom = 4.dp)) {
        Text(label, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.bodyMedium, fontFamily = FontFamily.Monospace)
    }
}

@Composable
fun RdsSignalPanel(modifier: Modifier = Modifier) {
    val active by LocalDiagnostics.active.collectAsState()
    val m by LocalDiagnostics.fm.collectAsState()
    val history by LocalDiagnostics.blerHistory.collectAsState()
    if (!active || m.timestampMs == 0L) return
    Column(
        modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp, vertical = 4.dp)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(8.dp))
            .padding(8.dp)
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Dot(if (m.rdsSynced) blerColor(m.blerRecent) else Bad)
            Spacer(Modifier.width(6.dp))
            Text(
                "RDS signal · ${if (m.rdsSynced) "synced" else "searching"} · ${if (m.stereo) "stereo" else "mono"}",
                style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold,
            )
        }
        Spacer(Modifier.height(6.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("BLER (last 100 groups)", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Text(pct(m.blerRecent), style = MaterialTheme.typography.headlineSmall, color = blerColor(m.blerRecent), fontFamily = FontFamily.Monospace)
                Text("since tune ${pct(m.blerSinceTune)}", style = MaterialTheme.typography.bodySmall)
            }
            Sparkline(history, Modifier.width(140.dp).height(44.dp))
        }
        Spacer(Modifier.height(6.dp))
        FlowRowCompat {
            Metric("Groups/s", String.format(Locale.US, "%.1f / 11.4", m.groupsPerSecond))
            Metric("Blocks ok", "${m.blocksOk}")
            Metric("Corrected", "${m.blocksCorrected}")
            Metric("Uncorrected", "${m.blocksFailed}")
            Metric("Pilot", String.format(Locale.US, "%.1f dB %s", m.pilotLevelDb, if (m.pilotLocked) "lock" else "—"))
            Metric("RDS injection", String.format(Locale.US, "%.1f kHz", m.rdsInjectionKhz))
            Metric("Peak dev.", String.format(Locale.US, "%.0f kHz", m.peakDeviationKhz))
        }
    }
}

private fun pct(v: Float) = String.format(Locale.US, "%.1f %%", v * 100)

/** Simple wrapping row without depending on the experimental FlowRow API. */
@Composable
internal fun FlowRowCompat(content: @Composable () -> Unit) {
    androidx.compose.ui.layout.Layout(content = content) { measurables, constraints ->
        val placeables = measurables.map { it.measure(constraints.copy(minWidth = 0, minHeight = 0)) }
        var x = 0; var y = 0; var rowH = 0
        val positions = placeables.map { p ->
            if (x + p.width > constraints.maxWidth && x > 0) { x = 0; y += rowH; rowH = 0 }
            val pos = x to y
            x += p.width; rowH = maxOf(rowH, p.height)
            pos
        }
        layout(constraints.maxWidth, y + rowH) {
            placeables.forEachIndexed { i, p -> p.placeRelative(positions[i].first, positions[i].second) }
        }
    }
}

@Composable
internal fun Sparkline(values: List<Float>, modifier: Modifier) {
    val line = MaterialTheme.colorScheme.primary
    val grid = MaterialTheme.colorScheme.outlineVariant
    Canvas(modifier) {
        drawLine(grid, Offset(0f, size.height), Offset(size.width, size.height))
        if (values.size < 2) return@Canvas
        val path = Path()
        values.forEachIndexed { i, v ->
            val x = i * size.width / 59f
            val y = size.height * (1 - v.coerceIn(0f, 1f))
            if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
        }
        drawPath(path, line, style = Stroke(width = 2f))
    }
}

@Composable
fun RdsAddonDetails() {
    val m by LocalDiagnostics.fm.collectAsState()
    val counts by LocalDiagnostics.groupCounts.collectAsState()
    Column(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
        val e = m.extras
        val facts = listOfNotNull(
            e.ecc?.let { "ECC" to String.format(Locale.US, "%02X%s", it, eccCountry(it)) },
            e.languageCode?.let { "Language" to String.format(Locale.US, "code %02X", it) },
            e.ptyn?.takeIf { it.isNotBlank() }?.let { "PTYN" to it },
            e.longPs?.takeIf { it.isNotBlank() }?.let { "Long PS" to it },
            e.rtPlusTitle?.let { "RT+ title" to it },
            e.rtPlusArtist?.let { "RT+ artist" to it },
            e.pin?.let { "PIN" to it },
            e.di?.let { "Decoder ID" to listOfNotNull("stereo".takeIf { _ -> it and 8 != 0 }, "artificial head".takeIf { _ -> it and 4 != 0 }, "compressed".takeIf { _ -> it and 2 != 0 }, "dynamic PTY".takeIf { _ -> it and 1 != 0 }).joinToString(", ").ifEmpty { "mono" } },
        )
        for ((k, v) in facts) Row(Modifier.padding(vertical = 2.dp)) {
            Text(k, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.width(96.dp))
            Text(v, style = MaterialTheme.typography.bodySmall)
        }
        Spacer(Modifier.height(8.dp))
        Text("Groups by type (since tune)", style = MaterialTheme.typography.titleSmall)
        GroupsHistogram(counts)
        Spacer(Modifier.height(8.dp))
        MpxView()
    }
}

private fun eccCountry(ecc: Int): String = when (ecc) {
    0xE3 -> " (NL/DE/IE/… by PI)"
    0xE0 -> " (DE/…)"
    0xE1 -> " (GR/IT/…)"
    0xE2 -> " (FR/CH/…)"
    0xE4 -> " (ES/…)"
    else -> ""
}

@Composable
private fun GroupsHistogram(counts: Map<String, Int>) {
    if (counts.isEmpty()) { Text("No groups yet.", style = MaterialTheme.typography.bodySmall); return }
    val total = counts.values.sum().coerceAtLeast(1)
    val max = counts.values.max().coerceAtLeast(1)
    val types = counts.keys.sortedWith(compareBy({ it.dropLast(1).toIntOrNull() ?: 99 }, { it.last() }))
    val bar = MaterialTheme.colorScheme.primary
    for (t in types) {
        val c = counts[t] ?: 0
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(vertical = 1.dp)) {
            Text(t, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace, modifier = Modifier.width(36.dp))
            Box(Modifier.weight(1f).height(12.dp)) {
                Box(Modifier.fillMaxWidth(c.toFloat() / max).height(12.dp).background(bar, RoundedCornerShape(2.dp)))
            }
            Text(String.format(Locale.US, " %d · %.0f %%", c, c * 100f / total), style = MaterialTheme.typography.bodySmall, modifier = Modifier.width(96.dp))
        }
    }
}

@Composable
private fun MpxView() {
    var show by remember { mutableStateOf(false) }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text("MPX spectrum (0-96 kHz)", style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
        Switch(checked = show, onCheckedChange = { show = it })
    }
    if (!show) return
    DisposableEffect(Unit) {
        LocalDiagnostics.mpxSpectrumWanted = true
        onDispose { LocalDiagnostics.mpxSpectrumWanted = false }
    }
    val spectrum by LocalDiagnostics.mpxSpectrum.collectAsState()
    val line = MaterialTheme.colorScheme.primary
    val marker = MaterialTheme.colorScheme.tertiary
    val grid = MaterialTheme.colorScheme.outlineVariant
    Canvas(Modifier.fillMaxWidth().height(160.dp).padding(top = 4.dp)) {
        val (bins, rate) = spectrum ?: return@Canvas
        val nyq = rate / 2f
        val minDb = -90f; val maxDb = 0f
        for (db in listOf(-20f, -40f, -60f, -80f)) {
            val y = size.height * (maxDb - db) / (maxDb - minDb)
            drawLine(grid, Offset(0f, y), Offset(size.width, y))
        }
        for (f in listOf(19_000f, 38_000f, 57_000f)) {
            val x = size.width * f / nyq
            drawLine(marker, Offset(x, 0f), Offset(x, size.height), strokeWidth = 1.5f)
        }
        val path = Path()
        bins.forEachIndexed { i, db ->
            val x = size.width * i / bins.size
            val y = size.height * (maxDb - db.coerceIn(minDb, maxDb)) / (maxDb - minDb)
            if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
        }
        drawPath(path, line, style = Stroke(width = 1.5f))
    }
    Text("Markers: 19 kHz pilot · 38 kHz stereo subcarrier · 57 kHz RDS. Grid every 20 dB.", style = MaterialTheme.typography.bodySmall)
}

/** Current FM metrics snapshot (for other screens, e.g. a waterfall overlay). */
fun fmStatusLine(m: FmSignalMetrics, ps: String?): String = listOfNotNull(
    ps?.trim()?.takeIf { it.isNotEmpty() },
    if (m.stereo) "ST" else "MONO",
    if (m.rdsSynced) String.format(Locale.US, "BLER %.0f%%", m.blerRecent * 100) else "no RDS",
).joinToString("  ·  ")
