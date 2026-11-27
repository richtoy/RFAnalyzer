package com.mantz_it.rfanalyzer.addon.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.widget.Toast
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mantz_it.rfanalyzer.addon.fm.FmPreset
import com.mantz_it.rfanalyzer.addon.fm.LocalFmScan
import com.mantz_it.rfanalyzer.digiext.rds.LocalRdsPtyUs
import com.mantz_it.rfanalyzer.digiext.rds.rdsPtyName
import java.util.Locale

/**
 * <h1>RF Analyzer - FM stations panel</h1>
 *
 * Module:      FmStationsUi.kt
 * Description: The Demodulation tab's FM stations panel (under "Stereo & RDS" in WFM): the band scan
 *              button with its progress and log, previous/next preset buttons, and the presets list
 *              with each station's RDS attributes. Tapping a station tunes it; the pencil opens a
 *              dialog to name or delete it and to see everything the scan recorded.
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */
@Composable
fun FmStationsPanel(channelFrequency: Long, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    LaunchedEffect(Unit) { LocalFmScan.init(context) }
    val status by LocalFmScan.status.collectAsState()
    val presets by LocalFmScan.presets.collectAsState()
    var showLog by remember { mutableStateOf(false) }
    var editing by remember { mutableStateOf<FmPreset?>(null) }
    var confirmClear by remember { mutableStateOf(false) }
    val current = presets.firstOrNull { kotlin.math.abs(it.frequencyHz - channelFrequency) < 50_000 }

    Column(
        modifier
            .fillMaxWidth()
            .padding(vertical = 6.dp)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(8.dp))
            .padding(8.dp)
    ) {
        Text("FM stations", style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            if (status.running) OutlinedButton(onClick = { LocalFmScan.stopScan() }) { Text("Stop scan") }
            else Button(onClick = { LocalFmScan.startScan() }) { Text("Scan FM band") }
            OutlinedButton(onClick = { LocalFmScan.stepPreset(-1) }, enabled = presets.isNotEmpty(), modifier = Modifier.widthIn(min = 44.dp)) { Text("◀") }
            OutlinedButton(onClick = { LocalFmScan.stepPreset(1) }, enabled = presets.isNotEmpty(), modifier = Modifier.widthIn(min = 44.dp)) { Text("▶") }
        }
        if (current != null) Text(
            "Now: ${String.format(Locale.US, "%.1f", current.frequencyMhz)} ${current.label()}",
            style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(top = 2.dp),
        )
        if (status.running) {
            Text(status.stage, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(top = 4.dp))
            LinearProgressIndicator(progress = { status.progress }, modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp))
        }
        if (status.log.isNotEmpty()) {
            Text(
                (if (showLog) "▾ " else "▸ ") + "Scan log (${status.log.size} lines)",
                style = MaterialTheme.typography.labelMedium,
                modifier = Modifier.padding(top = 4.dp).clickable { showLog = !showLog },
            )
            if (showLog) status.log.forEach {
                Text(it, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace, fontSize = 10.sp)
            }
        }
        if (!status.running && presets.isEmpty()) Text(
            "No stations yet. \"Scan FM band\" surveys 87.5-108 MHz, then checks each carrier for stereo and RDS (a few minutes). Stereo & RDS is switched on for it.",
            style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(top = 4.dp),
        )
        if (presets.isNotEmpty()) {
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 6.dp)) {
                Text(
                    "${presets.size} stations · ${presets.count { it.stereo }} stereo · ${presets.count { it.rds }} with RDS",
                    style = MaterialTheme.typography.labelMedium, modifier = Modifier.weight(1f),
                )
                TextButton(onClick = { copyCsv(context) }) { Text("Copy CSV", fontSize = 12.sp) }
                TextButton(onClick = { confirmClear = true }) { Text("Clear", fontSize = 12.sp) }
            }
            HorizontalDivider()
            presets.forEach { p -> PresetRow(p, p == current, onTune = { LocalFmScan.tunePreset(p.frequencyHz) }, onEdit = { editing = p }) }
        }
    }
    editing?.let { p -> PresetDialog(p, onDismiss = { editing = null }) }
    if (confirmClear) AlertDialog(
        onDismissRequest = { confirmClear = false },
        title = { Text("Clear all FM stations?") },
        text = { Text("The list and the names you gave stations are removed. A new scan finds the stations again.") },
        confirmButton = { TextButton(onClick = { LocalFmScan.clear(); confirmClear = false }) { Text("Clear") } },
        dismissButton = { TextButton(onClick = { confirmClear = false }) { Text("Cancel") } },
    )
}

private fun copyCsv(context: Context) {
    val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return
    cm.setPrimaryClip(ClipData.newPlainText("FM stations", LocalFmScan.csv()))
    Toast.makeText(context, "FM stations copied as CSV", Toast.LENGTH_SHORT).show()
}

/** The short attribute line under a station: stereo, RDS, PI, programme type, country, signal. */
@Composable
private fun attributes(p: FmPreset): String {
    val us = LocalRdsPtyUs.current
    val parts = ArrayList<String>()
    parts += if (p.stereo) "stereo" else "mono"
    if (p.rds) {
        parts += "RDS " + (p.pi?.let { String.format("%04X", it) } ?: "")
        p.pty?.takeIf { it > 0 }?.let { parts += rdsPtyName(it, us) }
        p.country?.let { parts += it }
        if (p.tp) parts += "TP"
        p.bler?.let { parts += String.format(Locale.US, "BLER %.0f%%", it * 100) }
    }
    parts += String.format(Locale.US, "SNR %.0f dB", p.snrDb)
    p.samePiAs?.let { parts += String.format(Locale.US, "= %.1f", it / 1e6) }
    return parts.joinToString(" · ")
}

@Composable
private fun PresetRow(p: FmPreset, isCurrent: Boolean, onTune: () -> Unit, onEdit: () -> Unit) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.fillMaxWidth().clickable(onClick = onTune).padding(vertical = 4.dp),
    ) {
        Text(
            String.format(Locale.US, "%5.1f", p.frequencyMhz),
            fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold,
            color = if (isCurrent) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.width(52.dp),
        )
        Column(Modifier.weight(1f)) {
            Text(
                p.label(), fontWeight = if (p.rds || p.name.isNotBlank()) FontWeight.SemiBold else FontWeight.Normal,
                maxLines = 1, overflow = TextOverflow.Ellipsis,
                color = if (isCurrent) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
            )
            Text(attributes(p), style = MaterialTheme.typography.bodySmall, color = if (p.rds) Good else MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (p.rt.isNotEmpty()) Text(p.rt, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        TextButton(onClick = onEdit) { Text("✎") }
    }
}

@Composable
private fun PresetDialog(p: FmPreset, onDismiss: () -> Unit) {
    var name by remember(p.frequencyHz) { mutableStateOf(p.name) }
    val us = LocalRdsPtyUs.current
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(String.format(Locale.US, "%.1f MHz  %s", p.frequencyMhz, p.ps.ifEmpty { "" })) },
        text = {
            Column {
                OutlinedTextField(value = name, onValueChange = { name = it }, label = { Text("Your name (optional)") }, singleLine = true)
                Spacer(Modifier.height(8.dp))
                fun line(k: String, v: String?) = if (v.isNullOrEmpty()) null else "$k: $v"
                listOfNotNull(
                    line("PI", p.pi?.let { String.format("%04X", it) }),
                    line("Programme type", p.pty?.let { rdsPtyName(it, us) }),
                    line("PTYN", p.ptyn),
                    line("Long PS", p.longPs),
                    line("Country", p.country),
                    line("ECC", p.ecc?.let { String.format("%02X", it) }),
                    line("Stereo", if (p.stereo) "yes" else "no"),
                    line("TP / TA", "${if (p.tp) "TP" else "-"} / ${if (p.ta) "TA" else "-"}"),
                    line("Level / SNR", String.format(Locale.US, "%.1f dB / %.1f dB", p.levelDb, p.snrDb)),
                    line("BLER", p.bler?.let { String.format(Locale.US, "%.0f %%", it * 100) }),
                    line("Groups/s", if (p.rds) String.format(Locale.US, "%.1f", p.groupsPerSec) else null),
                    line("AF", p.af.joinToString(", ") { String.format(Locale.US, "%.1f", it) }),
                    line("RadioText", p.rt),
                    line("Extras", listOfNotNull(
                        "RT+".takeIf { p.rtPlus }, if (p.eon > 0) "EON ${p.eon}" else null, "TMC".takeIf { p.tmc },
                        "CT".takeIf { p.clock },
                    ).joinToString(" ")),
                    line("ODA", p.oda.joinToString(", ")),
                    line("Same station as", p.samePiAs?.let { String.format(Locale.US, "%.1f MHz", it / 1e6) }),
                    line("PS after", p.psSecs?.let { String.format(Locale.US, "%.1f s", it) }),
                    line("Scanned", p.scanned),
                ).forEach { Text(it, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = { TextButton(onClick = { LocalFmScan.rename(p.frequencyHz, name); onDismiss() }) { Text("Save") } },
        dismissButton = {
            Row {
                TextButton(onClick = { LocalFmScan.delete(p.frequencyHz); onDismiss() }) { Text("Delete") }
                TextButton(onClick = onDismiss) { Text("Cancel") }
            }
        },
    )
}
