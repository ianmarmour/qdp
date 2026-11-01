package io.github.ianmarmour.qdp.ui.stream

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.SessionEvent

/** The opacity of the stream's overlays' backgrounds, which leaves the video visible through them. */
const val OVERLAY_ALPHA = 0.7f

/** Tabular figures, so that changing numbers do not shift their neighbors. */
private const val TABULAR_FIGURES = "tnum"

/**
 * The statistics pill in the stream's corner: "60 fps · 18 ms", and at the
 * [StatsLevel.DETAILED] level where the second's slowest frame spent its
 * time, from capture on the host to decoding here.
 */
@Composable
fun StatsPill(stats: SessionEvent.Stats, level: StatsLevel, modifier: Modifier = Modifier) {
    val tabular = TextStyle(fontFeatureSettings = TABULAR_FIGURES)
    Column(
        modifier = modifier
            .clip(MaterialTheme.shapes.medium)
            .background(MaterialTheme.colorScheme.surfaceContainerLowest.copy(alpha = OVERLAY_ALPHA))
            .padding(horizontal = 12.dp, vertical = 6.dp),
    ) {
        Text(statsSummary(stats), style = MaterialTheme.typography.labelLarge.merge(tabular))
        val slowest = stats.slowest
        if (level == StatsLevel.DETAILED && slowest != null) {
            Text(
                text = stringResource(R.string.stats_slowest, slowest.latencyMillis),
                style = MaterialTheme.typography.labelMedium.merge(tabular),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                text = stringResource(
                    R.string.stats_stages,
                    slowest.toFirstFragmentMillis,
                    slowest.transferMillis,
                    slowest.queuedMillis,
                    slowest.decodeMillis,
                ),
                style = MaterialTheme.typography.labelSmall.merge(tabular),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/** Returns "60 fps · 18 ms" for [stats], with a dash while the latency is unknown. */
@Composable
fun statsSummary(stats: SessionEvent.Stats): String {
    val latency = stats.latencyMillis?.let { stringResource(R.string.stats_latency, it) }
        ?: stringResource(R.string.stats_latency_unknown)
    return stringResource(R.string.stats_summary, stats.fps, latency)
}
