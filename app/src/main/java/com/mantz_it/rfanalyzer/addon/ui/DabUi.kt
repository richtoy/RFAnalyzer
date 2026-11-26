package com.mantz_it.rfanalyzer.addon.ui

import android.graphics.BitmapFactory
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.mantz_it.rfanalyzer.addon.dab.DabChannels
import com.mantz_it.rfanalyzer.addon.dab.DabNames
import com.mantz_it.rfanalyzer.addon.dab.DabService
import com.mantz_it.rfanalyzer.addon.dab.DabState
import com.mantz_it.rfanalyzer.addon.dab.LocalDab
import java.util.Locale

/**
 * <h1>RF Analyzer - DAB screens</h1>
 *
 * Module:      DabUi.kt
 * Description: The DAB/DAB+ panel of the Demodulation tab (channel, scan, now playing, services)
 *              and the full-screen DAB view with its technical pages: Now playing, Services,
 *              Ensemble (subchannel/CU map) and Signal (SNR, frequency offset, FIC and audio
 *              decoder quality, constellation, transmitters).
 *
 * Part of Richard's personal RF Analyzer fork.
 * License: http://www.gnu.org/licenses/gpl.html GPL version 2 or higher
 */

private const val TOTAL_CU = 864

private fun qualityColor(percent: Int) = when {
    percent >= 90 -> Good
    percent >= 60 -> Warn
    else -> Bad
}

private fun snrColor(snr: Int) = when {
    snr >= 10 -> Good
    snr >= 5 -> Warn
    else -> Bad
}

private fun DabState.statusText(): String = when {
    !active -> "Start the analyzer to receive DAB"
    !sync && secondsSinceTune < 3 -> "Searching for an ensemble…"
    !sync -> "No DAB signal on ${channel?.name ?: "this frequency"}"
    ensembleLabel.isEmpty() -> "Synced · reading the ensemble…"
    else -> ensembleLabel
}

@Composable
private fun rememberSlide(serial: Int): ImageBitmap? = remember(serial) {
    val bytes = LocalDab.slide ?: return@remember null
    try { BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap() } catch (e: Exception) { null }
}

/** The Demodulation tab's DAB panel. */
@Composable
fun DabControls(modifier: Modifier = Modifier) {
    val s by LocalDab.state.collectAsState()
    val scan by LocalDab.scan.collectAsState()
    var showDetails by remember { mutableStateOf(false) }

    Column(modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 4.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            ChannelPicker(s)
            if (scan.running) OutlinedButton(onClick = { LocalDab.stopScan() }) { Text("Stop scan") }
            else OutlinedButton(onClick = { LocalDab.startScan() }) { Text("Scan") }
            Spacer(Modifier.weight(1f))
            TextButton(onClick = { showDetails = true }) { Text("Details") }
        }
        if (scan.running) {
            Text("Scanning ${scan.currentChannel} (${scan.channelIndex + 1}/${scan.channelCount}) · ${scan.found.size} found",
                style = MaterialTheme.typography.bodySmall)
            LinearProgressIndicator(
                progress = { (scan.channelIndex + 1f) / scan.channelCount.coerceAtLeast(1) },
                modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
            )
        }
        StatusRow(s)
        Spacer(Modifier.height(6.dp))
        NowPlayingCard(s, compact = true)
        Spacer(Modifier.height(6.dp))
        ServiceList(s)
    }
    if (showDetails) DabDetailsDialog(onDismiss = { showDetails = false })
}

@Composable
private fun ChannelPicker(s: DabState) {
    val scan by LocalDab.scan.collectAsState()
    var open by remember { mutableStateOf(false) }
    Box {
        Button(onClick = { open = true }) {
            Text(s.channel?.let { "${it.name} · ${it.frequencyMhz} MHz" } ?: "Channel ▾")
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            if (scan.found.isNotEmpty()) {
                Text("Found ensembles", style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp))
                scan.found.forEach { e ->
                    DropdownMenuItem(
                        text = { Text("${e.channel} · ${e.label} (${e.services.size})") },
                        onClick = { open = false; DabChannels.byName(e.channel)?.let { LocalDab.tuneToChannel(it) } },
                    )
                }
                HorizontalDivider()
            }
            DabChannels.bandIII.forEach { ch ->
                DropdownMenuItem(
                    text = { Text("${ch.name}  ${ch.frequencyMhz} MHz", fontFamily = FontFamily.Monospace) },
                    onClick = { open = false; LocalDab.tuneToChannel(ch) },
                )
            }
        }
    }
}

@Composable
private fun StatusRow(s: DabState) {
    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 6.dp)) {
        Dot(if (!s.sync) Bad else if (s.quality.ficPercent >= 90) Good else Warn)
        Spacer(Modifier.width(6.dp))
        Text(s.statusText(), style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold,
            maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        if (s.sync) Text("SNR ${s.snrDb} dB · FIC ${s.quality.ficPercent}%", style = MaterialTheme.typography.bodySmall,
            fontFamily = FontFamily.Monospace)
    }
}

@Composable
private fun NowPlayingCard(s: DabState, compact: Boolean) {
    val svc = s.playing ?: s.selectedService
    val slide = rememberSlide(s.slideSerial)
    Column(
        Modifier.fillMaxWidth()
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(8.dp))
            .padding(8.dp)
    ) {
        if (svc == null) {
            Text(if (s.services.isEmpty()) "No services yet" else "Pick a service below",
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            return@Column
        }
        Row(verticalAlignment = Alignment.Top) {
            if (slide != null) {
                Image(slide, contentDescription = "Slideshow", contentScale = ContentScale.Fit,
                    modifier = Modifier.width(if (compact) 112.dp else 320.dp).aspectRatio(4f / 3f).background(Color.Black))
                Spacer(Modifier.width(8.dp))
            }
            Column(Modifier.weight(1f)) {
                Text(svc.label, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                Text(listOfNotNull(
                    svc.codecShort.ifEmpty { null },
                    svc.bitrateKbps.takeIf { it > 0 }?.let { "$it kbit/s" },
                    svc.pty.takeIf { it > 0 }?.let { DabNames.pty(it) },
                    "${s.audioRate / 1000} kHz".takeIf { s.audioBufferedMs > 0 },
                ).joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                if (s.dls.isNotEmpty()) {
                    Spacer(Modifier.height(4.dp))
                    Text(s.dls, style = MaterialTheme.typography.bodyMedium, maxLines = if (compact) 3 else 6, overflow = TextOverflow.Ellipsis)
                }
                if (svc.ascty == 63 && s.sync) {
                    Spacer(Modifier.height(4.dp))
                    Text("Audio: frames ${s.quality.framePercent}% · RS ${s.quality.rsPercent}% · AAC ${s.quality.aacPercent}%",
                        style = MaterialTheme.typography.labelSmall, color = qualityColor(minOf(s.quality.framePercent, s.quality.aacPercent)))
                }
            }
        }
    }
}

@Composable
private fun ServiceList(s: DabState) {
    val services = s.audioServices.sortedBy { it.label.lowercase(Locale.ROOT) }
    if (services.isEmpty()) return
    Text("Services (${services.size})", style = MaterialTheme.typography.labelMedium, modifier = Modifier.padding(vertical = 4.dp))
    Column(Modifier.border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(8.dp))) {
        services.forEachIndexed { i, svc ->
            if (i > 0) HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
            val selected = svc.sid == s.selectedSid
            Row(
                Modifier.fillMaxWidth()
                    .background(if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent)
                    .clickable { LocalDab.selectService(svc.sid) }
                    .padding(horizontal = 10.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(svc.label, style = MaterialTheme.typography.bodyMedium,
                    fontWeight = if (selected) FontWeight.Bold else FontWeight.Normal, modifier = Modifier.weight(1f))
                Text(listOfNotNull(svc.codecShort.ifEmpty { null }, svc.bitrateKbps.takeIf { it > 0 }?.let { "$it k" }).joinToString(" "),
                    style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

// ------------------------------------------------------------------ full-screen view

@Composable
fun DabDetailsDialog(onDismiss: () -> Unit) {
    var page by remember { mutableIntStateOf(0) }
    val s by LocalDab.state.collectAsState()
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) {
            Column {
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(start = 16.dp, end = 4.dp, top = 8.dp)) {
                    Column(Modifier.weight(1f)) {
                        Text("DAB+ · ${s.channel?.name ?: "-"}", style = MaterialTheme.typography.titleLarge)
                        Text(s.statusText(), style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    TextButton(onClick = onDismiss) { Text("Close") }
                }
                TabRow(selectedTabIndex = page) {
                    listOf("Playing", "Services", "Ensemble", "Signal").forEachIndexed { i, t ->
                        Tab(selected = page == i, onClick = { page = i }, text = { Text(t) })
                    }
                }
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp)) {
                    when (page) {
                        0 -> { NowPlayingCard(s, compact = false); Spacer(Modifier.height(8.dp)); ServiceList(s) }
                        1 -> ServicesPage(s)
                        2 -> EnsemblePage(s)
                        else -> SignalPage(s)
                    }
                }
            }
        }
    }
}

@Composable
private fun KeyValue(key: String, value: String) {
    Row(Modifier.fillMaxWidth().padding(vertical = 2.dp)) {
        Text(key, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.width(150.dp))
        Text(value, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
    }
}

@Composable
private fun Cell(text: String, width: Int, bold: Boolean = false) {
    Text(text, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace, maxLines = 1,
        overflow = TextOverflow.Ellipsis, fontWeight = if (bold) FontWeight.Bold else FontWeight.Normal,
        modifier = Modifier.width(width.dp).padding(end = 6.dp))
}

@Composable
private fun ServicesPage(s: DabState) {
    if (s.services.isEmpty()) { Text("No services yet."); return }
    Column(Modifier.horizontalScroll(rememberScrollState())) {
        Row { listOf("SId" to 52, "Label" to 140, "Short" to 80, "Codec" to 96, "kbit/s" to 52, "Prot." to 64, "SubCh" to 48, "PTY" to 120, "Language" to 90)
            .forEach { (t, w) -> Cell(t, w, bold = true) } }
        HorizontalDivider()
        s.services.sortedBy { it.label.lowercase(Locale.ROOT) }.forEach { svc ->
            Row(Modifier.clickable(enabled = svc.isAudio != false) { LocalDab.selectService(svc.sid) }.padding(vertical = 3.dp)) {
                Cell(svc.sidHex, 52, bold = svc.sid == s.selectedSid)
                Cell(svc.label, 140, bold = svc.sid == s.selectedSid)
                Cell(svc.shortLabel, 80)
                Cell(svc.codec, 96)
                Cell(if (svc.bitrateKbps > 0) "${svc.bitrateKbps}" else "-", 52)
                Cell(svc.protection, 64)
                Cell(if (svc.subchId >= 0) "${svc.subchId}" else "-", 48)
                Cell(if (svc.described) svc.ptyName else "-", 120)
                Cell(if (svc.described) DabNames.language(svc.language) else "-", 90)
            }
        }
    }
    Spacer(Modifier.height(8.dp))
    Text("Tap a service to play it. Subchannel details appear once the receiver has read the ensemble's configuration (FIG 0/1, 0/2).",
        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

private val subchannelColors = listOf(
    Color(0xFF4E79A7), Color(0xFFF28E2B), Color(0xFF59A14F), Color(0xFFB07AA1), Color(0xFF76B7B2),
    Color(0xFFEDC948), Color(0xFFE15759), Color(0xFF9C755F), Color(0xFFBAB0AC), Color(0xFFFF9DA7),
)

@Composable
private fun EnsemblePage(s: DabState) {
    KeyValue("Ensemble", s.ensembleLabel.ifEmpty { "-" })
    KeyValue("Ensemble ID", s.ensembleIdHex + if (s.countryId >= 0) "  (country id ${"%X".format(s.countryId)})" else "")
    KeyValue("Channel", s.channel?.let { "${it.name} · ${it.frequencyMhz} MHz" } ?: "%.3f MHz".format(s.frequencyHz / 1e6))
    KeyValue("Time (from FIG 0/10)", s.timeUtc.ifEmpty { "-" })
    KeyValue("Services", "${s.services.size} (${s.audioServices.size} audio)")
    KeyValue("Capacity used", "${s.usedCu} of $TOTAL_CU CU (${s.usedCu * 100 / TOTAL_CU}%)")
    Spacer(Modifier.height(10.dp))
    Text("Main service channel (capacity units)", style = MaterialTheme.typography.labelMedium)
    val subchannels = s.services.filter { it.subchId >= 0 && it.lengthCu > 0 }.distinctBy { it.subchId }.sortedBy { it.startCu }
    val outline = MaterialTheme.colorScheme.outlineVariant
    val selectedOutline = MaterialTheme.colorScheme.onSurface
    Canvas(Modifier.fillMaxWidth().height(36.dp).padding(vertical = 4.dp)) {
        drawRect(outline, size = size, style = androidx.compose.ui.graphics.drawscope.Stroke(1f))
        subchannels.forEach { svc ->
            val x = size.width * svc.startCu / TOTAL_CU
            val w = size.width * svc.lengthCu / TOTAL_CU
            drawRect(subchannelColors[svc.subchId % subchannelColors.size], Offset(x, 0f), Size(w, size.height))
            if (svc.sid == s.selectedSid) drawRect(selectedOutline, Offset(x, 0f), Size(w, size.height), style = androidx.compose.ui.graphics.drawscope.Stroke(3f))
        }
    }
    Spacer(Modifier.height(6.dp))
    if (subchannels.isEmpty()) {
        Text("Subchannels appear as the receiver reads them.", style = MaterialTheme.typography.bodySmall)
        return
    }
    Column(Modifier.horizontalScroll(rememberScrollState())) {
        Row { listOf("" to 16, "SubCh" to 48, "Start" to 48, "CUs" to 44, "kbit/s" to 52, "Prot." to 64, "Codec" to 60, "Service" to 160)
            .forEach { (t, w) -> Cell(t, w, bold = true) } }
        HorizontalDivider()
        subchannels.forEach { svc ->
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(vertical = 3.dp)) {
                Box(Modifier.size(10.dp).background(subchannelColors[svc.subchId % subchannelColors.size]))
                Spacer(Modifier.width(6.dp))
                Cell("${svc.subchId}", 48)
                Cell("${svc.startCu}", 48)
                Cell("${svc.lengthCu}", 44)
                Cell("${svc.bitrateKbps}", 52)
                Cell(svc.protection, 64)
                Cell(svc.codecShort, 60)
                Cell(s.services.filter { it.subchId == svc.subchId }.joinToString(", ") { it.label }, 160, bold = svc.sid == s.selectedSid)
            }
        }
    }
}

@Composable
private fun SignalPage(s: DabState) {
    // SNR history (last 60 updates, normalised to 0..30 dB)
    var history by remember { mutableStateOf(listOf<Float>()) }
    LaunchedEffect(s.secondsSinceTune) { if (s.sync) history = (history + (s.snrDb / 30f).coerceIn(0f, 1f)).takeLast(60) }

    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text("SNR", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(if (s.sync) "${s.snrDb} dB" else "-", style = MaterialTheme.typography.headlineSmall,
                color = if (s.sync) snrColor(s.snrDb) else Bad, fontFamily = FontFamily.Monospace)
        }
        Sparkline(history, Modifier.width(160.dp).height(44.dp))
    }
    Spacer(Modifier.height(8.dp))
    FlowRowCompat {
        Metric("Sync", if (s.sync) "yes" else "no")
        Metric("Freq. offset", "${s.frequencyOffsetHz} Hz")
        Metric("FIC quality", "${s.quality.ficPercent} %")
        Metric("Frames ok", "${s.quality.framePercent} %")
        Metric("RS ok", "${s.quality.rsPercent} %")
        Metric("AAC ok", "${s.quality.aacPercent} %")
        Metric("Audio rate", "${s.audioRate} Hz")
        Metric("Audio buffer", "${s.audioBufferedMs} ms")
        Metric("Dropped input", "${s.droppedSamples}")
    }
    Spacer(Modifier.height(10.dp))
    Text("Constellation (DQPSK, after differential demodulation)", style = MaterialTheme.typography.labelMedium)
    Constellation(Modifier.size(220.dp).padding(4.dp))
    Spacer(Modifier.height(10.dp))
    Text("Transmitters (TII)", style = MaterialTheme.typography.labelMedium)
    if (s.transmitters.isEmpty()) Text("None identified yet. TII needs a good signal and takes a while.", style = MaterialTheme.typography.bodySmall)
    s.transmitters.forEach { t -> KeyValue("Main ${t.mainId} / sub ${t.subId}", "strength %.1f".format(t.strength)) }
}

@Composable
private fun Constellation(modifier: Modifier) {
    DisposableEffect(Unit) {
        LocalDab.constellationWanted = true
        onDispose { LocalDab.constellationWanted = false }
    }
    val points by LocalDab.constellation.collectAsState()
    val axis = MaterialTheme.colorScheme.outlineVariant
    val dot = MaterialTheme.colorScheme.primary
    Canvas(modifier.background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(6.dp))) {
        val c = Offset(size.width / 2, size.height / 2)
        val r = minOf(size.width, size.height) * 0.42f
        drawLine(axis, Offset(0f, c.y), Offset(size.width, c.y))
        drawLine(axis, Offset(c.x, 0f), Offset(c.x, size.height))
        var i = 0
        while (i + 1 < points.size) {
            drawCircle(dot.copy(alpha = 0.5f), radius = 1.5f, center = Offset(c.x + points[i] * r, c.y - points[i + 1] * r))
            i += 2
        }
    }
}
