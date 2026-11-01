package io.github.ianmarmour.qdp.ui.home

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.calculateEndPadding
import androidx.compose.foundation.layout.calculateStartPadding
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LargeTopAppBar
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.TopAppBarScrollBehavior
import androidx.compose.material3.adaptive.currentWindowAdaptiveInfo
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.focusRestorer
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isCtrlPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.window.core.layout.WindowSizeClass
import io.github.ianmarmour.qdp.HostCard
import io.github.ianmarmour.qdp.HostStatus
import io.github.ianmarmour.qdp.R
import io.github.ianmarmour.qdp.StreamTarget
import io.github.ianmarmour.qdp.ui.components.GamepadButton
import io.github.ianmarmour.qdp.ui.components.QdpIconButton
import io.github.ianmarmour.qdp.ui.components.gamepadButtons
import io.github.ianmarmour.qdp.ui.components.rememberGamepadConnected
import io.github.ianmarmour.qdp.ui.components.rememberKeyDriven

/** The narrowest a card is laid out; the grid fits as many columns as this allows. */
private val CARD_MIN_WIDTH = 240.dp

/** The widest the grid grows, on tablets. */
private val MAX_GRID_WIDTH = 1200.dp

/** The space between cards, and around the grid. */
private val GRID_SPACING = 16.dp

/** The grid key of the search's progress line. */
private const val SEARCH_KEY = "search"

/** The grid key of the "Add by address" card. */
private const val ADD_KEY = "add"

/**
 * The hosts screen: the computers found on the Wi-Fi and those paired with,
 * as a grid of cards that ends with "Add by address", or a searching or
 * empty state while there are none. It looks for computers while it shows.
 *
 * Tapping a card does what its status says (connect, pair, or explain);
 * a gamepad's A does the same, Y opens the focused card's options, X adds a
 * computer and Start, like Ctrl+comma, opens settings.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(
    viewModel: HomeViewModel,
    onStream: (StreamTarget) -> Unit,
    onPairByAddress: (address: String) -> Unit,
    onConnectWithToken: (address: String, pin: String, token: String) -> Unit,
    onOpenSettings: () -> Unit,
    onOpenWifiSettings: () -> Unit,
) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val dialog by viewModel.dialog.collectAsStateWithLifecycle()
    LifecycleResumeEffect(viewModel) {
        viewModel.startSearching()
        onPauseOrDispose { viewModel.stopSearching() }
    }
    val sizeClass = currentWindowAdaptiveInfo().windowSizeClass
    val compactWidth = !sizeClass.isWidthAtLeastBreakpoint(WindowSizeClass.WIDTH_DP_MEDIUM_LOWER_BOUND)
    val compactHeight = !sizeClass.isHeightAtLeastBreakpoint(WindowSizeClass.HEIGHT_DP_MEDIUM_LOWER_BOUND)
    // A phone held upright has room for a large title that collapses as
    // the grid scrolls; short landscape windows keep a 64dp bar.
    val collapsing = compactWidth && !compactHeight
    val scrollBehavior = if (collapsing) {
        TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    } else {
        TopAppBarDefaults.pinnedScrollBehavior()
    }
    val gamepad = rememberGamepadConnected()
    var focusedKey by remember { mutableStateOf<String?>(null) }
    var menuKey by remember { mutableStateOf<String?>(null) }
    val focused = state.cards.firstOrNull { it.key == focusedKey }
    val addHost = { viewModel.addHost(advanced = false) }

    fun open(card: HostCard) {
        when (card.status) {
            HostStatus.READY, HostStatus.NOT_SEEN -> {
                viewModel.rememberAddress(card)
                card.target?.let(onStream)
            }
            HostStatus.NEW -> card.target?.let(onStream)
            HostStatus.PAIRING_OFF -> viewModel.show(HomeDialog.PairingOff(card))
            HostStatus.KEY_MISMATCH -> viewModel.show(HomeDialog.KeyMismatch(card))
        }
    }

    Scaffold(
        modifier = Modifier
            .nestedScroll(scrollBehavior.nestedScrollConnection)
            .gamepadButtons { button ->
                when (button) {
                    GamepadButton.Y -> focused?.let { menuKey = it.key }
                    GamepadButton.X -> addHost()
                    GamepadButton.START -> onOpenSettings()
                    GamepadButton.SELECT, GamepadButton.L3, GamepadButton.R3 -> Unit
                }
            }
            .onPreviewKeyEvent { event ->
                val shortcut = event.type == KeyEventType.KeyDown && event.isCtrlPressed && event.key == Key.Comma
                if (shortcut) {
                    onOpenSettings()
                }
                shortcut
            },
        topBar = { HomeTopBar(collapsing, scrollBehavior, addHost, onOpenSettings) },
        bottomBar = {
            if (gamepad) {
                GamepadHintBar(stringResource(focused?.let { actionLabelOf(it.status) } ?: R.string.hint_select))
            }
        },
    ) { padding ->
        if (state.cards.isEmpty()) {
            EmptyState(
                phase = state.search,
                onAdd = addHost,
                onSetupHelp = { viewModel.show(HomeDialog.SetupHelp) },
                onWifiSettings = onOpenWifiSettings,
                modifier = Modifier.fillMaxSize().padding(padding),
            )
        } else {
            HostGrid(
                state = state,
                padding = padding,
                menuKey = menuKey,
                onMenuKeyChange = { menuKey = it },
                onFocusedKeyChange = { key, hasFocus ->
                    if (hasFocus) {
                        focusedKey = key
                    } else if (focusedKey == key) {
                        focusedKey = null
                    }
                },
                onOpen = ::open,
                onForget = { viewModel.show(HomeDialog.Forget(it)) },
                onDetails = { viewModel.show(HomeDialog.Details(it)) },
                onAdd = addHost,
                onWifiSettings = onOpenWifiSettings,
            )
        }
    }

    dialog?.let { shown ->
        HomeDialogs(
            dialog = shown,
            onDismiss = { viewModel.show(null) },
            onForget = { card ->
                viewModel.forget(card)
                viewModel.show(null)
            },
            onPair = { card ->
                viewModel.show(null)
                card.target?.let(onStream)
            },
            onAddByAddressPair = { address ->
                viewModel.show(null)
                onPairByAddress(address)
            },
            onConnectWithToken = { address, pin, token ->
                viewModel.show(null)
                onConnectWithToken(address, pin, token)
            },
        )
    }
}

/** The screen's title bar, large and collapsing when [collapsing], with Add by address and Settings. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun HomeTopBar(
    collapsing: Boolean,
    scrollBehavior: TopAppBarScrollBehavior,
    onAdd: () -> Unit,
    onOpenSettings: () -> Unit,
) {
    val title = stringResource(R.string.home_title)
    val actions: @Composable RowScope.() -> Unit = {
        QdpIconButton(
            onClick = onAdd,
            icon = R.drawable.ic_add,
            description = stringResource(R.string.action_add_by_address),
        )
        QdpIconButton(
            onClick = onOpenSettings,
            icon = R.drawable.ic_settings,
            description = stringResource(R.string.action_settings),
        )
    }
    if (collapsing) {
        LargeTopAppBar(title = { Text(title) }, actions = actions, scrollBehavior = scrollBehavior)
    } else {
        TopAppBar(
            title = { Text(title, style = MaterialTheme.typography.headlineSmall) },
            actions = actions,
            scrollBehavior = scrollBehavior,
        )
    }
}

/**
 * The computers' cards, as many columns as fit, then "Add by address".
 * Focus returns to the card focused last, as when coming back from a
 * stream, and starts on the first card when keys drive the screen.
 */
@Composable
private fun HostGrid(
    state: HomeState,
    padding: PaddingValues,
    menuKey: String?,
    onMenuKeyChange: (String?) -> Unit,
    onFocusedKeyChange: (key: String, hasFocus: Boolean) -> Unit,
    onOpen: (HostCard) -> Unit,
    onForget: (HostCard) -> Unit,
    onDetails: (HostCard) -> Unit,
    onAdd: () -> Unit,
    onWifiSettings: () -> Unit,
) {
    val firstCard = remember { FocusRequester() }
    val keyDriven = rememberKeyDriven()
    var anyFocused by remember { mutableStateOf(false) }
    LaunchedEffect(keyDriven) {
        if (keyDriven && !anyFocused) {
            withFrameNanos {}
            firstCard.requestFocus()
        }
    }
    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
        LazyVerticalGrid(
            columns = GridCells.Adaptive(CARD_MIN_WIDTH),
            modifier = Modifier
                .widthIn(max = MAX_GRID_WIDTH)
                .fillMaxWidth()
                .focusRestorer(firstCard)
                .onFocusChanged { anyFocused = it.hasFocus },
            contentPadding = padding.plus(GRID_SPACING),
            horizontalArrangement = Arrangement.spacedBy(GRID_SPACING),
            verticalArrangement = Arrangement.spacedBy(GRID_SPACING),
        ) {
            if (state.search != SearchPhase.FOUND) {
                item(key = SEARCH_KEY, span = { GridItemSpan(maxLineSpan) }) {
                    SearchBanner(state.search, onWifiSettings, Modifier.animateItem())
                }
            }
            items(state.cards, key = { it.key }) { card ->
                HostCardView(
                    card = card,
                    menuExpanded = menuKey == card.key,
                    onMenuExpandedChange = { expanded -> onMenuKeyChange(if (expanded) card.key else null) },
                    onOpen = { onOpen(card) },
                    onForget = { onForget(card) },
                    onDetails = { onDetails(card) },
                    modifier = Modifier
                        .animateItem()
                        .then(if (card.key == state.cards.first().key) Modifier.focusRequester(firstCard) else Modifier)
                        .onFocusChanged { onFocusedKeyChange(card.key, it.hasFocus) },
                )
            }
            item(key = ADD_KEY) {
                AddHostCard(onClick = onAdd, modifier = Modifier.animateItem())
            }
        }
    }
}

/** Returns these padding values with [extra] added on every side. */
@Composable
private fun PaddingValues.plus(extra: Dp): PaddingValues {
    val direction = LocalLayoutDirection.current
    return PaddingValues(
        start = calculateStartPadding(direction) + extra,
        top = calculateTopPadding() + extra,
        end = calculateEndPadding(direction) + extra,
        bottom = calculateBottomPadding() + extra,
    )
}
