package io.github.ianmarmour.qdp

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Typeface
import android.os.Build
import android.os.Bundle
import android.text.InputType
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.view.WindowInsets
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Spinner
import android.widget.TextView
import android.widget.Toast

/**
 * The connection screen: the hosts on the local network and those paired
 * with, a host to add by its address, and how to stream.
 *
 * Tapping a paired host streams from it; tapping another pairs with it,
 * asking for the code it shows. Holding a paired host forgets it.
 */
class MainActivity : Activity() {
    private lateinit var hostList: LinearLayout
    private lateinit var searching: TextView
    private lateinit var host: EditText
    private lateinit var pin: EditText
    private lateinit var token: EditText
    private lateinit var resolution: Spinner
    private lateinit var decoder: Spinner
    private lateinit var touchMode: Spinner
    private lateinit var showStats: CheckBox
    private lateinit var microphone: CheckBox
    private lateinit var discovery: Discovery

    /** The hosts on the local network. */
    private var discovered: List<DiscoveredHost> = emptyList()

    /** The stream waiting for the microphone permission. */
    private var pending: StreamTarget? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Native.init(applicationContext)
        takeLink(intent)
        val settings = Settings.load(this)
        discovery = Discovery(this) { hosts ->
            discovered = hosts
            showHosts()
        }
        val form = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            val margin = dp(24)
            setPadding(margin, margin, margin, margin)
        }
        form.addView(TextView(this).apply {
            text = getString(R.string.app_name)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 28f)
        })

        form.addView(heading("Hosts"))
        hostList = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        form.addView(hostList, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
        searching = note("Looking for hosts on this network… A host appears here while qdp serve runs on it.")
        form.addView(searching)

        form.addView(heading("Add a host"))
        host = form.field("Address", "192.168.1.20 or desktop.local:4433", settings.host)
        form.addView(Button(this).apply {
            text = "Pair"
            setOnClickListener { pairByAddress() }
        }, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT).apply { topMargin = dp(8) })
        form.addView(note("Or connect with the host's pin, from qdp pin, and a token it accepts:"))
        pin = form.field("Host key pin", "64 hexadecimal digits from `qdp pin`", settings.pin)
        token = form.field("Token", "The token the host accepts", settings.token).apply {
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
        }
        form.addView(Button(this).apply {
            text = "Connect with a token"
            setOnClickListener { connectWithToken() }
        }, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT).apply { topMargin = dp(8) })

        form.addView(heading("Stream"))
        resolution = form.choice("Resolution", Resolution.entries.map { getString(it.label) }, settings.resolution.ordinal)
        decoder = form.choice("Decoder", Decoder.entries.map { getString(it.label) }, settings.decoder.ordinal)
        touchMode = form.choice("Touch", TouchMode.entries.map { getString(it.label) }, settings.touchMode.ordinal)
        showStats = CheckBox(this).apply {
            text = "Show statistics"
            isChecked = settings.showStats
        }
        form.addView(showStats)
        microphone = CheckBox(this).apply {
            text = "Send the microphone"
            isChecked = settings.microphone
        }
        form.addView(microphone)
        Native.clientPin()?.let { devicePin ->
            form.addView(note("This device's pin, for qdp serve --client-key:"))
            form.addView(TextView(this).apply {
                text = devicePin
                setTextIsSelectable(true)
                typeface = Typeface.MONOSPACE
            })
        }

        val scroll = ScrollView(this).apply {
            addView(form)
            // The form stays clear of the system bars and the keyboard.
            setOnApplyWindowInsetsListener { view, insets ->
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.ime())
                    view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
                } else {
                    @Suppress("DEPRECATION")
                    view.setPadding(
                        insets.systemWindowInsetLeft,
                        insets.systemWindowInsetTop,
                        insets.systemWindowInsetRight,
                        insets.systemWindowInsetBottom,
                    )
                }
                insets
            }
        }
        setContentView(scroll)
    }

    override fun onResume() {
        super.onResume()
        // A stream may have paired with a host meanwhile.
        showHosts()
        discovery.start()
    }

    override fun onPause() {
        discovery.stop()
        super.onPause()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (takeLink(intent)) {
            recreate()
        }
    }

    /** Keeps the settings a connection link in [intent] gives, returning whether it had one. */
    private fun takeLink(intent: Intent?): Boolean {
        val link = intent?.data ?: return false
        val settings = ConnectionLink.apply(link, Settings.load(this)) ?: return false
        settings.save(this)
        // The link is used once, not again when the screen is recreated.
        intent.data = null
        Toast.makeText(this, "Filled in from the link", Toast.LENGTH_SHORT).show()
        return true
    }

    /** A host in the list: what it shows, what tapping it opens, and the paired host it is. */
    private data class HostRow(
        val name: String,
        val detail: String,
        val target: StreamTarget?,
        val paired: PairedHost?,
    )

    /**
     * Returns the hosts to list: those found on the network, recognized as
     * paired by their pin, then the paired hosts not found.
     */
    private fun hostRows(): List<HostRow> {
        val paired = PairedHosts.load(this)
        val found = mutableSetOf<String>()
        val rows = discovered.sortedBy { it.name.lowercase() }.map { host ->
            val match = paired.firstOrNull { it.pin == host.pin }
            when {
                match != null -> {
                    found += match.pin
                    HostRow(
                        host.name,
                        "Paired · ${host.address}",
                        StreamTarget(host.name, host.address, Native.AUTH_PAIRED, match.pin),
                        match,
                    )
                }
                // A host taking a paired host's name with another key may
                // pose as it (Section 3.13).
                paired.any { it.name == host.name } -> HostRow(
                    host.name,
                    "Its key is not the one paired with under this name · ${host.address}",
                    if (host.pairing) StreamTarget(host.name, host.address, Native.AUTH_PAIR) else null,
                    null,
                )
                host.pairing -> HostRow(
                    host.name,
                    "Tap to pair · ${host.address}",
                    StreamTarget(host.name, host.address, Native.AUTH_PAIR),
                    null,
                )
                else -> HostRow(host.name, "Pairing is off on this host · ${host.address}", null, null)
            }
        }
        val absent = paired.filter { it.pin !in found }.map { host ->
            HostRow(
                host.name,
                "Paired · ${host.address} · not found on this network",
                StreamTarget(host.name, host.address, Native.AUTH_PAIRED, host.pin),
                host,
            )
        }
        return rows + absent
    }

    /** Lists the hosts. */
    private fun showHosts() {
        hostList.removeAllViews()
        for (row in hostRows()) {
            hostList.addView(hostView(row), LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
        }
        searching.visibility = if (discovered.isEmpty()) View.VISIBLE else View.GONE
    }

    /** Returns the view of a host in the list. */
    private fun hostView(row: HostRow): View = LinearLayout(this).apply {
        orientation = LinearLayout.VERTICAL
        setPadding(dp(12), dp(12), dp(12), dp(12))
        isFocusable = true
        isClickable = true
        val background = TypedValue()
        if (context.theme.resolveAttribute(android.R.attr.selectableItemBackground, background, true)) {
            setBackgroundResource(background.resourceId)
        }
        addView(TextView(context).apply {
            text = row.name
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 18f)
        })
        addView(TextView(context).apply {
            text = row.detail
            alpha = SECONDARY_ALPHA
        })
        setOnClickListener { open(row) }
        if (row.paired != null) {
            setOnLongClickListener {
                forget(row.paired)
                true
            }
        }
    }

    /** Streams from, or pairs with, the host of [row]. */
    private fun open(row: HostRow) {
        val target = row.target
        if (target == null) {
            Toast.makeText(
                this,
                "Start qdp serve on ${row.name} with pairing on, or connect with its pin and a token",
                Toast.LENGTH_LONG,
            ).show()
            return
        }
        // A paired host found at a new address is reached there from now on.
        val paired = row.paired
        if (paired != null && (paired.address != target.address || paired.name != target.name)) {
            PairedHosts.keep(this, paired.copy(name = target.name, address = target.address))
        }
        launch(target)
    }

    /** Asks whether to forget [host], and does if so. */
    private fun forget(host: PairedHost) {
        AlertDialog.Builder(this)
            .setTitle("Forget ${host.name}?")
            .setMessage("Connecting to it again needs pairing with a new code.")
            .setPositiveButton("Forget") { _, _ ->
                PairedHosts.forget(this, host.pin)
                showHosts()
            }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }

    /** Pairs with the host at the address typed. */
    private fun pairByAddress() {
        val address = host.text.toString().trim()
        if (address.isEmpty()) {
            host.error = "Enter the host's address"
            return
        }
        launch(StreamTarget(address, address, Native.AUTH_PAIR))
    }

    /** Checks the address, pin and token typed, and streams with them. */
    private fun connectWithToken() {
        val address = host.text.toString().trim()
        val hostPin = pin.text.toString().trim()
        when {
            address.isEmpty() -> host.error = "Enter the host's address"
            !Native.isPin(hostPin) -> pin.error = "A pin is 64 hexadecimal digits, as `qdp pin` prints"
            token.text.isEmpty() -> token.error = "Enter the token the host was started with"
            else -> launch(StreamTarget(address, address, Native.AUTH_TOKEN, hostPin, token.text.toString()))
        }
    }

    /** Keeps the form and streams [target], asking for the microphone first if it is wanted. */
    private fun launch(target: StreamTarget) {
        val settings = Settings(
            host = host.text.toString().trim(),
            pin = pin.text.toString().trim(),
            token = token.text.toString(),
            resolution = Resolution.entries[resolution.selectedItemPosition],
            decoder = Decoder.entries[decoder.selectedItemPosition],
            touchMode = TouchMode.entries[touchMode.selectedItemPosition],
            showStats = showStats.isChecked,
            microphone = microphone.isChecked,
        )
        settings.save(this)
        val granted = checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        if (settings.microphone && !granted) {
            pending = target
            requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), MICROPHONE_REQUEST)
        } else {
            stream(target)
        }
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        // Without the permission the session streams without the microphone.
        val target = pending
        if (requestCode == MICROPHONE_REQUEST && target != null) {
            pending = null
            stream(target)
        }
    }

    /** Starts streaming [target] with the kept settings. */
    private fun stream(target: StreamTarget) {
        startActivity(target.into(Intent(this, StreamActivity::class.java)))
    }

    /** Returns `value` density-independent pixels in pixels. */
    private fun dp(value: Int): Int =
        TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value.toFloat(), resources.displayMetrics).toInt()

    /** Returns a section's heading. */
    private fun heading(text: String): View = TextView(this).apply {
        this.text = text
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 20f)
        setPadding(0, dp(24), 0, dp(4))
    }

    /** Returns a line of explanation. */
    private fun note(text: String): TextView = TextView(this).apply {
        this.text = text
        alpha = SECONDARY_ALPHA
        setPadding(0, dp(12), 0, 0)
    }

    /** Adds a labelled text field showing [value]. */
    private fun LinearLayout.field(label: String, hint: String, value: String): EditText {
        addView(label(label))
        val field = EditText(context).apply {
            this.hint = hint
            setText(value)
            isSingleLine = true
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        }
        addView(field, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
        return field
    }

    /** Adds a labelled choice of [options], with [selected] chosen. */
    private fun LinearLayout.choice(label: String, options: List<String>, selected: Int): Spinner {
        addView(label(label))
        val spinner = Spinner(context).apply {
            adapter = ArrayAdapter(context, android.R.layout.simple_spinner_dropdown_item, options)
            setSelection(selected)
        }
        addView(spinner, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
        return spinner
    }

    /** Returns a field's label. */
    private fun label(text: String): View = TextView(this).apply {
        this.text = text
        gravity = Gravity.START
        setPadding(0, dp(16), 0, 0)
    }

    private companion object {
        const val MICROPHONE_REQUEST = 1

        /** The opacity of secondary text. */
        const val SECONDARY_ALPHA = 0.7f
    }
}
