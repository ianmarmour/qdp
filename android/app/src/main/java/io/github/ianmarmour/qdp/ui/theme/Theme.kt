package io.github.ianmarmour.qdp.ui.theme

import android.content.Context
import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import io.github.ianmarmour.qdp.ColorSource

/** The color that focused items are outlined with; the theme's primary. */
val LocalFocusRingColor = staticCompositionLocalOf { Color.Unspecified }

/** Rounded shapes: cards at 20dp and dialogs at 28dp; buttons and chips are pills. */
private val QdpShapes = Shapes(
    extraSmall = RoundedCornerShape(8.dp),
    small = RoundedCornerShape(12.dp),
    medium = RoundedCornerShape(16.dp),
    large = RoundedCornerShape(20.dp),
    extraLarge = RoundedCornerShape(28.dp),
)

/**
 * QDP's Material 3 theme: the wallpaper's colors on Android 12 and later
 * when [colorSource] asks for them, neutral ones otherwise, in [dark] or
 * light theme; the system font's type scale; and rounded shapes.
 */
@Composable
fun QdpTheme(colorSource: ColorSource, dark: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    val colors = colorScheme(LocalContext.current, colorSource, dark)
    MaterialTheme(colorScheme = colors, shapes = QdpShapes) {
        CompositionLocalProvider(LocalFocusRingColor provides colors.primary, content = content)
    }
}

/** Returns the color scheme for [source] in [dark] or light theme. */
private fun colorScheme(context: Context, source: ColorSource, dark: Boolean): ColorScheme {
    if (source == ColorSource.WALLPAPER && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
        // The system's dynamic schemes follow its contrast setting themselves.
        return if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
    }
    return neutralColorScheme(dark)
}
